//! 消息网关：把 ABI v6 的两条发送通道包成几个直白的方法。
//!
//! 两条通道分工明确（见 `docs/abi-v6-spec.md` 第 4 条）：
//!
//! - **回复某条命令消息**（`reply_to = Some(message_id)`）：走宿主的
//!   [`fuyubot.host.command`](super::command) `reply`，由宿主决定是「发新消息并挂
//!   `reply_to`」、「编辑原命令消息」，还是「群里的 ephemeral 消息」。插件**不再自己**
//!   把 `chat_id` / `reply_to` 拼进 `sendMessage` 参数。
//! - **主动推送**（`reply_to = None`，比如任务 panic 时的内部错误提示）：没有命令消息
//!   可回，继续走 simplify 的 `send_message`。
//!
//! 本插件没有相册发送；将来若要发 `sendMessageAlbum`，必须留在 simplify
//! `send_message` 这一侧——`fuyubot.host.command.reply` 只收单条
//! `input_message_content`，表达不了相册。
//!
//! 两条通道都是**阻塞**的（simplify 要等 `response` 更新，`call_service` 要等 TDLib
//! 回包），因此只能在自己的业务线程里调用，不能在 `updateNewMessage` 回调里调。
//!
//! 图片上传走 TDLib 的「上传本地照片」分支：`inputPhoto` 里塞 `inputFileLocal`，
//! 宽高交给服务器替换（客户端拿不到真图尺寸，也不必为了它再解一遍图片）。
//!
//! 该网关仅服务于当前插件，不依赖其它插件的运行时代码。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

use super::command;

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

    /// 发送一条纯文本消息；`reply_to` 填命令消息 id 即「回复那条命令」。
    pub fn send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<Sent> {
        let content = json!({
            "@type": "inputMessageText",
            "text": { "@type": "formattedText", "text": text, "entities": [] },
        });
        self.send(chat_id, content, reply_to, SEND_TIMEOUT_MS)
    }

    /// 发送一条纯文本消息，失败只记日志、返回 `None`。
    pub fn try_send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<Sent> {
        match self.send_text(chat_id, text, reply_to) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host.warn(&format!("whois: 发送消息失败: {error}"));
                None
            }
        }
    }

    /// 以照片形式发一条消息：本地文件 + 可选回复。
    pub fn send_photo(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        reply_to: Option<i64>,
    ) -> Result<Sent> {
        // 卡片只发送图片，不带 caption；TDLib 会使用默认空值。
        let content = json!({
            "@type": "inputMessagePhoto",
            "photo": {
                "@type": "inputPhoto",
                "photo": { "@type": "inputFileLocal", "path": path.to_string_lossy() },
                "width": 0,
                "height": 0,
            },
        });
        self.send(chat_id, content, reply_to, UPLOAD_TIMEOUT_MS)
    }

    /// 两条通道的唯一分叉点。
    ///
    /// - 有命令消息可回：交给宿主 `fuyubot.host.command.reply`，内容之外的东西
    ///   （`chat_id` / `reply_to` / 编辑 / ephemeral）全由宿主决定；
    /// - 没有命令消息（主动推送）：simplify `send_message`，此时 `chat_id` 是唯一
    ///   的投递目标，本来就得由插件自己给。
    fn send(
        &self,
        chat_id: i64,
        content: Value,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> Result<Sent> {
        let message = match reply_to {
            Some(message_id) => {
                command::reply(&self.host, chat_id, message_id, &content, timeout_ms)?
            }
            None => self.op(
                "send_message",
                json!({ "chat_id": chat_id, "input_message_content": content }),
                timeout_ms,
            )?,
        };
        Ok(Sent {
            chat_id,
            message_id: message_id_of(&message),
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

/// 最终 `Message` 的 `id`。宿主没给就记 0：只影响日志里的定位信息，不影响发送结果。
fn message_id_of(message: &Value) -> i64 {
    message.get("id").and_then(Value::as_i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_message_id_degrades_to_zero() {
        assert_eq!(message_id_of(&json!({ "@type": "message", "id": 42 })), 42);
        assert_eq!(message_id_of(&json!({ "@type": "message" })), 0);
    }
}
