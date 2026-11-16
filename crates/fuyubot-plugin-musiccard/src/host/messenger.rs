//! 消息收发：把 ABI v6 的 `simplify` 操作表与宿主的命令回复服务包成几个直白的方法。
//!
//! 发送分两条路，区别只在**有没有命令消息可回**：
//!
//! - `reply_to = Some(message_id)`（回复触发命令的那条消息）→ 走宿主的
//!   `fuyubot.host.command.reply`（见 [`crate::host::command`]）：插件只交
//!   `input_message_content`，发新消息 + `reply_to`、编辑原命令消息、还是群里发
//!   ephemeral，全由宿主决定。插件**不再自己拼** `chat_id` / `reply_to` / `sendMessage`。
//! - `reply_to = None`（主动推送）→ 继续用 simplify 的 `send_message`：没有命令
//!   消息可回，宿主没有上下文可以编辑或回复，只能发新消息。相册
//!   （`sendMessageAlbum`）同理走这条路——一条消息组没有单一命令可回；本插件目前
//!   没有相册，将来加也走这里。
//!
//! 所有方法都是**阻塞**的：simplify 要等 `response` 通道的响应，命令回复服务自己
//! 同步等 TDLib。因此只能在自己的业务线程里调用，绝不能在宿主回调里调。
//!
//! `args` 一律按 TDLib 原生字段名书写（`td_api.tl` 为准）。

use std::path::Path;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};
use thiserror::Error;

use crate::host::command;

/// 一次发送得到的消息定位信息。
#[derive(Debug, Clone, Copy)]
pub struct Sent {
    pub message_id: i64,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("调用宿主失败: {0}")]
    Host(String),
    #[error("等待 TDLib 响应超时")]
    Timeout,
    #[error("命令回复结果不是 JSON: {0}")]
    Json(String),
    #[error("操作被拒绝（{code}）: {message}")]
    Op { code: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// 上传超时：照片动辄几百 KB，比普通文本消息放宽得多。
const UPLOAD_TIMEOUT_MS: u64 = 300_000;
/// 普通消息（含删除 / 编辑）的超时。
const MESSAGE_TIMEOUT_MS: u64 = 30_000;

/// 拼出纯文本 `formattedText`（不挂实体）。
fn text_content(text: &str) -> Value {
    json!({
        "@type": "inputMessageText",
        "text": { "@type": "formattedText", "text": text, "entities": [] },
    })
}

/// 本地文件 -> `inputMessagePhoto`，用于发卡片。
fn photo_content(path: &Path, caption: Option<&str>, spoiler: bool) -> Value {
    let mut content = json!({
        "@type": "inputMessagePhoto",
        "photo": {
            "@type": "inputPhoto",
            "photo": { "@type": "inputFileLocal", "path": path.to_string_lossy() },
            // 宽高交给服务器替换：客户端不必为了它把图解一遍。
            "width": 0,
            "height": 0,
        },
        "has_spoiler": spoiler,
    });
    if let Some(caption) = caption {
        content["caption"] = json!({
            "@type": "formattedText",
            "text": caption,
            "entities": [],
        });
    }
    content
}

/// 从 TDLib 的最终 `Message` 里取消息 id（缺失按 0，调用方只把它当「发出去的那条」）。
fn sent_from(message: &Value) -> Sent {
    Sent {
        message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
    }
}

/// 消息网关。`HostApi` 是 `Copy`，这里直接持有一份。
#[derive(Debug, Clone)]
pub struct Messenger {
    host: HostApi,
}

impl Messenger {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 发送一条纯文本消息，失败只记日志、返回 `None`。
    ///
    /// `reply_to` 指向触发命令的那条消息时走宿主命令回复通道；`None` 是主动推送，
    /// 走 simplify 的 `send_message`。
    pub fn try_send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<Sent> {
        self.try_send(chat_id, text_content(text), reply_to, MESSAGE_TIMEOUT_MS)
    }

    fn try_send(
        &self,
        chat_id: i64,
        content: Value,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> Option<Sent> {
        match self.send_content(chat_id, content, reply_to, timeout_ms) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host.warn(&format!("musiccard: 发送消息失败: {error}"));
                None
            }
        }
    }

    /// 以照片形式发一条消息：本地文件 + 可选回复 + 可选说明 + 可选剧透遮罩。
    ///
    /// `reply_to` 指向需要回复的消息；卡片会回复触发命令的那条消息，
    /// 那时发新消息 / 编辑原命令由宿主决定（本插件不再拼 `reply_to`）。
    pub fn send_photo(
        &self,
        chat_id: i64,
        path: &Path,
        reply_to: Option<i64>,
        caption: Option<&str>,
        spoiler: bool,
    ) -> Result<Sent> {
        let content = photo_content(path, caption, spoiler);
        self.send_content(chat_id, content, reply_to, UPLOAD_TIMEOUT_MS)
    }

    /// 按「有没有命令消息可回」选通道。
    fn send_content(
        &self,
        chat_id: i64,
        content: Value,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> Result<Sent> {
        match reply_to {
            // 回复某条命令：交给宿主决定发新消息 / 编辑原命令 / ephemeral。
            Some(message_id) => {
                let message =
                    command::reply(&self.host, chat_id, message_id, timeout_ms, &content)?;
                Ok(sent_from(&message))
            }
            // 主动推送（相册同理）：没有命令消息可回，宿主没有上下文可编辑，只能发新消息。
            None => {
                let args = json!({
                    "chat_id": chat_id,
                    "input_message_content": content,
                });
                let message = self.op("send_message", args, timeout_ms)?;
                Ok(sent_from(&message))
            }
        }
    }

    /// 删除消息（清理进度消息 / 失败现场都靠它）。
    pub fn delete_messages(&self, chat_id: i64, message_ids: &[i64]) {
        if message_ids.is_empty() {
            return;
        }
        let args = json!({
            "chat_id": chat_id,
            "message_ids": message_ids,
            "revoke": true,
        });
        if let Err(error) = self.op("delete_messages", args, MESSAGE_TIMEOUT_MS) {
            self.host.warn(&format!("musiccard: 删除消息失败: {error}"));
        }
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
