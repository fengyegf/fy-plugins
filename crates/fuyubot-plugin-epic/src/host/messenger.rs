//! 消息收发：把 ABI 的 `simplify` 操作表包成几个直白的方法。
//!
//! 所有方法都是**阻塞**的，只能在自己的业务线程里调用，绝不能在宿主回调里调。
//! `args` 按 TDLib 原生字段名书写（`td_api.tl` 为准）。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};
use thiserror::Error;

/// 一次发送得到的消息定位信息。
///
/// `chat_id` 目前只用于日志/调试时定位，保留字段是为了和 TDLib 返回形状一致。
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct Sent {
    pub chat_id: i64,
    pub message_id: i64,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("调用宿主失败: {0}")]
    Host(String),
    #[error("等待 TDLib 响应超时")]
    Timeout,
    #[error("操作被拒绝（{code}）: {message}")]
    Op { code: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// 普通消息 / 上传图片的发送超时。
const SEND_TIMEOUT_MS: u64 = 30_000;
/// 上传超时：购图体积不大，但网络差时放宽。
const UPLOAD_TIMEOUT_MS: u64 = 120_000;

/// 消息网关。`HostApi` 是 `Copy`，可以直接放进结构体。
#[derive(Debug, Clone)]
pub struct Messenger {
    host: HostApi,
}

impl Messenger {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 发一条纯文本消息，`reply_to` 填消息 id 即「回复」。
    pub fn send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<Sent> {
        let content = json!({
            "@type": "inputMessageText",
            "text": { "@type": "formattedText", "text": text, "entities": [] },
        });
        self.send_content(chat_id, content, reply_to, SEND_TIMEOUT_MS)
    }

    /// 发一条纯文本消息，失败只记日志、返回 `None`。
    pub fn try_send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<Sent> {
        match self.send_text(chat_id, text, reply_to) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host.warn(&format!("epic: 发送消息失败: {error}"));
                None
            }
        }
    }

    /// 以照片形式发一条消息：本地文件 + 可选说明。
    pub fn send_photo(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        caption: Option<&str>,
        reply_to: Option<i64>,
    ) -> Result<Sent> {
        let mut content = json!({
            "@type": "inputMessagePhoto",
            "photo": {
                "@type": "inputPhoto",
                "photo": { "@type": "inputFileLocal", "path": path.to_string_lossy() },
                "width": 0,
                "height": 0,
            },
        });
        if let Some(caption) = caption.filter(|text| !text.is_empty()) {
            content["caption"] = json!({
                "@type": "formattedText",
                "text": caption,
                "entities": [],
            });
        }
        self.send_content(chat_id, content, reply_to, UPLOAD_TIMEOUT_MS)
    }

    fn send_content(
        &self,
        chat_id: i64,
        content: Value,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> Result<Sent> {
        let mut args = json!({
            "chat_id": chat_id,
            "input_message_content": content,
        });
        if let Some(message_id) = reply_to {
            args["reply_to"] = json!({
                "@type": "inputMessageReplyToMessage",
                "message_id": message_id,
            });
        }
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
