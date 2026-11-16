//! 消息网关：把「回复命令」与 simplify 发送包成几个直白的方法。
//!
//! 两条通道，按「有没有命令消息可回」分开：
//!
//! - **命令回复**（`reply_to = Some(message_id)`）：走宿主预置的
//!   `fuyubot.host.command.reply`（见 [`crate::host::command`]）。插件只交
//!   `input_message_content`；发新消息 + `reply_to`、编辑原命令消息、群里的
//!   `sendEphemeralMessage` 全由宿主按账号类型与触发者决定。
//! - **主动推送**（`reply_to = None`）：走 simplify 的 `send_message`。没有命令消息可回，
//!   也就没有 `message_id` 交给命令回复服务，只能自己发（这里也不带 `reply_to`）。
//!
//! 相册（`sendMessageAlbum`）同理留在 simplify：它是一次发多条消息，没有单条
//! 「命令回复」语义；本 crate 目前不用相册，将来要用也只能走 simplify 的发送系。
//!
//! 两条通道都是**阻塞**的：命令回复由宿主当场等 TDLib 的最终消息，simplify 先用
//! `simplify_invoke` 拿请求号、再在 `response` 通道上等结果。因此只能在插件自己的
//! 业务线程（`kfc-task`）里调，绝不能在 `updateNewMessage` 回调里调。
//!
//! 图片上传走 TDLib 的「上传本地照片」分支：`inputPhoto` 里塞 `inputFileLocal`，
//! 宽高交给服务器替换（客户端拿不到真图尺寸，也不必为了它再解一遍图片）。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

use crate::host::command;

/// 普通消息超时。
const SEND_TIMEOUT_MS: u64 = 30_000;
/// 上传超时：图片虽然不大，但弱网下多给一点余量。
const UPLOAD_TIMEOUT_MS: u64 = 120_000;

/// 一次发送得到的消息定位信息。
#[derive(Debug, Clone, Copy)]
pub struct Sent {
    pub chat_id: i64,
    pub message_id: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("调用宿主失败: {0}")]
    Host(String),
    #[error("等待 TDLib 响应超时")]
    Timeout,
    #[error("操作被拒绝（{code}）: {message}")]
    Op { code: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// 消息网关。`HostApi` 是 `Copy`，这里只是给它加几个名字。
#[derive(Debug, Clone)]
pub struct Messenger {
    host: HostApi,
}

impl Messenger {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 发送一条纯文本消息。
    ///
    /// `reply_to = Some(id)` 是**命令消息**的 id：走宿主命令回复通道；
    /// `None` 是主动推送，走 simplify。
    pub fn send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<Sent> {
        let content = text_content(text);
        self.send(chat_id, content, reply_to, SEND_TIMEOUT_MS)
    }

    /// 发送一条纯文本消息，失败只记日志、返回 `None`。
    pub fn try_send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<Sent> {
        match self.send_text(chat_id, text, reply_to) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host.warn(&format!("kfc: 发送消息失败: {error}"));
                None
            }
        }
    }

    /// 以照片形式发一条消息：本地文件 + 可选「回复某条命令」。
    ///
    /// 不带 caption：TS 版也只是一张图。TDLib 的 `caption` 有默认值，可以整段省略。
    pub fn send_photo(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        reply_to: Option<i64>,
    ) -> Result<Sent> {
        let content = photo_content(path);
        self.send(chat_id, content, reply_to, UPLOAD_TIMEOUT_MS)
    }

    /// 两条通道的唯一分叉点：有命令消息可回就交给宿主，否则自己发。
    fn send(
        &self,
        chat_id: i64,
        content: Value,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> Result<Sent> {
        match reply_to {
            // 回复命令：宿主管「发新消息 + reply_to / 编辑原命令 / ephemeral」。
            Some(message_id) => {
                command::reply(&self.host, chat_id, message_id, &content, timeout_ms)
            }
            // 主动推送：没有命令消息，宿主那套回复形态不适用，只能自己拼 sendMessage；
            // 这里**不带** `reply_to`，也不往参数里塞 `chat_id` 之外的会话信息。
            None => self.send_message(chat_id, content, timeout_ms),
        }
    }

    /// simplify 的 `send_message`：只用于主动推送（相册 / 没有命令消息可回的场合）。
    fn send_message(&self, chat_id: i64, content: Value, timeout_ms: u64) -> Result<Sent> {
        let args = json!({ "chat_id": chat_id, "input_message_content": content });
        let message = self.op("send_message", args, timeout_ms)?;
        Ok(Sent {
            chat_id,
            message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
        })
    }

    /// 跑一个 simplify 操作并等它的结果。
    ///
    /// 结果信封形如
    /// `{"@type":"simplifyResult","op":…,"ok":true,"result":…,"request_id":N}`，
    /// 成功失败同形状，只看 `ok`。
    fn op(&self, name: &str, args: Value, timeout_ms: u64) -> Result<Value> {
        let mut args = args;
        args["timeout_ms"] = json!(timeout_ms);

        let request_id = self
            .host
            .simplify_invoke_json(json!({ "op": name, "args": args }))
            .map_err(|error| Error::Host(error.to_string()))?;

        // 多给 5 秒余量：宿主要先等 TDLib，再回投响应。
        let filter = json!({ "event_type": "response", "request_id": request_id.id });
        let response = self
            .host
            .wait_for_update_json(filter, timeout_ms + 5_000)
            .ok_or(Error::Timeout)?;

        if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
        let error = response.get("error").cloned().unwrap_or(Value::Null);
        Err(Error::Op {
            code: error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未提供错误信息")
                .to_owned(),
        })
    }
}

/// 一条纯文本的 `input_message_content`。
///
/// `inputMessageText.text` 在 TDLib 里是 **`formattedText` 对象**，不是字符串；
/// 直接塞字符串会被 TDLib 以 400 拒绝。
fn text_content(text: &str) -> Value {
    json!({
        "@type": "inputMessageText",
        "text": { "@type": "formattedText", "text": text, "entities": [] },
    })
}

/// 一张本地图片的 `input_message_content`（形状照 `td_api.tl` 的 `inputMessagePhoto`）。
///
/// 宽高交给服务器替换（`width` / `height` 为 0 表示让 TDLib 自己取）。
fn photo_content(path: &std::path::Path) -> Value {
    json!({
        "@type": "inputMessagePhoto",
        "photo": {
            "@type": "inputPhoto",
            "photo": { "@type": "inputFileLocal", "path": path.to_string_lossy() },
            "width": 0,
            "height": 0,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_content_is_formatted_text() {
        let content = text_content("你好");
        assert_eq!(content["@type"], "inputMessageText");
        assert_eq!(content["text"]["@type"], "formattedText");
        assert_eq!(content["text"]["text"], "你好");
        assert_eq!(content["text"]["entities"], json!([]));
    }

    #[test]
    fn photo_content_uploads_a_local_file() {
        let content = photo_content(std::path::Path::new("C:/tmp/card.jpg"));
        assert_eq!(content["@type"], "inputMessagePhoto");
        assert_eq!(content["photo"]["@type"], "inputPhoto");
        assert_eq!(content["photo"]["photo"]["@type"], "inputFileLocal");
        assert_eq!(content["photo"]["photo"]["path"], json!("C:/tmp/card.jpg"));
    }
}
