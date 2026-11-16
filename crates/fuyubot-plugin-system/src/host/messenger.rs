//! 消息网关：把 ABI v6 的发送能力包成几个直白的方法。
//!
//! 所有方法都是**阻塞**的，只能在自己的业务线程里调用，绝不能在宿主回调里调。
//! `args` 按 TDLib 原生字段名书写（`td_api.tl` 为准）。
//!
//! 两条发送通道，别混（ABI v6 硬契约第 4 条）：
//! - **命令回复**（有命令消息可回）走宿主的 `fuyubot.host.command` 服务，实现在
//!   [`command`](crate::host::command)：插件只交 `input_message_content`，发新消息 +
//!   `reply_to`、编辑原命令消息还是发 ephemeral，全由宿主按账户类型与触发者决定，
//!   插件**不拼** `chat_id` / `reply_to`；
//! - **主动发送**（没有命令消息可回，例如 panic 兜底）走 simplify 的 `send_message`。
//!   本插件不发相册（`sendMessageAlbum`）；真发的话它同样没有「单条命令消息」可回，
//!   也留在这一条通道里，原因一样：宿主只对「某条命令消息」做回复形态决策。
//!
//! 图片上传走 TDLib 的「上传本地照片」分支：`inputPhoto` 里塞 `inputFileLocal`，
//! 宽高交给服务器替换（客户端拿不到真图尺寸，也不必为了它再解一遍图片）。
//!
//! 两条通道的分法与本仓已对齐的 kfc / whois / epic 一致。

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
    #[error("宿主返回的结果不是 JSON: {0}")]
    Protocol(String),
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

    /// 发送一条纯文本消息。`reply_to` 填命令消息 id 即「回复这条命令」，
    /// 走宿主的命令回复通道；`None` 是主动推送。
    pub fn send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<Sent> {
        let content = json!({
            "@type": "inputMessageText",
            "text": { "@type": "formattedText", "text": text, "entities": [] },
        });
        self.send_content(chat_id, content, reply_to, SEND_TIMEOUT_MS)
    }

    /// 发送一条纯文本消息，失败只记日志、返回 `None`。
    pub fn try_send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<Sent> {
        match self.send_text(chat_id, text, reply_to) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host.warn(&format!("system: 发送消息失败: {error}"));
                None
            }
        }
    }

    /// 以照片形式发一条消息：本地文件 + 可选回复命令消息。
    pub fn send_photo(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        reply_to: Option<i64>,
    ) -> Result<Sent> {
        // 不带 caption：TS 版也只是一张图。TDLib 的 `caption` 有默认值，可以整段省略。
        let content = json!({
            "@type": "inputMessagePhoto",
            "photo": {
                "@type": "inputPhoto",
                "photo": { "@type": "inputFileLocal", "path": path.to_string_lossy() },
                "width": 0,
                "height": 0,
            },
        });
        self.send_content(chat_id, content, reply_to, UPLOAD_TIMEOUT_MS)
    }

    fn send_content(
        &self,
        chat_id: i64,
        content: Value,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> Result<Sent> {
        match reply_to {
            // 回复某条命令消息：形态交给宿主，插件不拼 reply_to（见 host::command）。
            Some(message_id) => {
                command::reply(&self.host, chat_id, message_id, &content, timeout_ms)
            }
            // 主动发送：没有命令消息可回，仍然走 simplify 的 sendMessage。
            None => self.send_new(chat_id, content, timeout_ms),
        }
    }

    /// 主动发一条消息：没有命令消息可回时用，走 simplify 的 `sendMessage`。
    fn send_new(&self, chat_id: i64, content: Value, timeout_ms: u64) -> Result<Sent> {
        let args = json!({
            "chat_id": chat_id,
            "input_message_content": content,
        });
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
            code: error_code(&error),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未提供错误信息")
                .to_owned(),
        })
    }
}

/// simplify 信封里的 `code` 是整数，宿主服务层偶尔给字符串；两种都吃掉。
fn error_code(error: &Value) -> String {
    match error.get("code") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_error_payload_becomes_op_error() {
        // 与 `op` 里的映射同形：取 code（int / string 都收）与 message。
        let error = json!({ "code": 400, "message": "bad request" });
        assert_eq!(error_code(&error), "400");
        assert_eq!(error_code(&json!({ "code": "FLOOD_WAIT" })), "FLOOD_WAIT");
        assert_eq!(error_code(&json!({})), "unknown");
    }
}
