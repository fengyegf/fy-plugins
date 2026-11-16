//! 命令回复通道：宿主的 `fuyubot.host.command` 服务（方法 `reply`）。
//!
//! ABI v6 的硬规矩：**回复某条命令消息**不要自己拼 `sendMessage`。插件只交出
//! `chat_id` / 命令消息 `message_id` / `input_message_content`，剩下的形态由宿主决定：
//!
//! - bot 账户（或白名单账户）触发 → 发一条新消息，`reply_to` 由宿主自己带；
//! - user 账户自己触发 → 直接编辑原命令消息；
//! - 群里的 ephemeral 命令 → `sendEphemeralMessage`。
//!
//! 协议（对照插件文档「命令表与 /help」）：
//!
//! ```text
//! reply   入参 {"chat_id": i64, "message_id": i64, "timeout_ms": u64,
//!               "payload": {"input_message_content": <sendMessage 参数>}}
//! reply   成功 TDLib 最终的 Message（含 "id"）
//! reply   失败 {"@type": "error", "code": …, "message": …}
//! ```
//!
//! 该服务**同步阻塞**（宿主内部等 TDLib 应答），只能在工作线程调用，绝不能在
//! `updateNewMessage` 回调里调。入参构造与响应解析抽成纯函数，单测不碰宿主。

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use super::messenger::Error;

/// 宿主预置的命令回复服务（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.host.command";

/// 服务方法名：回一条命令消息。
const METHOD: &str = "reply";

/// `reply` 的入参。
pub(crate) fn payload(chat_id: i64, message_id: i64, content: &Value, timeout_ms: u64) -> Value {
    json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "timeout_ms": timeout_ms,
        "payload": { "input_message_content": content },
    })
}

/// 把一条回复发出去，返回 TDLib 最终的 `Message`。
pub(crate) fn reply(
    host: &HostApi,
    chat_id: i64,
    message_id: i64,
    content: Value,
    timeout_ms: u64,
) -> Result<Value, Error> {
    let input = payload(chat_id, message_id, &content, timeout_ms);
    let raw = host
        .call_service(SERVICE, METHOD, &input.to_string())
        .map_err(|error| Error::Host(format!("{SERVICE}.{METHOD}: {error}")))?;
    parse_message(&raw)
}

/// 解析 `reply` 的返回值。
///
/// 空串 = 宿主成功但提供方没有输出（ABI 契约：NULL 已经进了 `AbiError`），对「必须拿到
/// 消息 id」的调用方来说仍是失败。
pub(crate) fn parse_message(raw: &str) -> Result<Value, Error> {
    if raw.trim().is_empty() {
        return Err(Error::Host(format!("{SERVICE}.{METHOD} 没有返回结果")));
    }
    let message: Value = serde_json::from_str(raw).map_err(|error| {
        Error::Host(format!(
            "{SERVICE}.{METHOD} 响应不是 JSON: {error}（原文：{raw}）"
        ))
    })?;
    if message.get("@type").and_then(Value::as_str) == Some("error") {
        return Err(Error::Op {
            code: error_code(&message),
            message: message
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未提供错误信息")
                .to_owned(),
        });
    }
    Ok(message)
}

/// TDLib 的 `code` 是整数，宿主服务层偶尔给字符串；两种都吃掉。
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
    fn payload_wraps_content_under_payload() {
        let content = json!({ "@type": "inputMessageText", "text": { "@type": "formattedText", "text": "hi", "entities": [] } });
        let value = payload(42, 7, &content, 30_000);
        assert_eq!(value["chat_id"], 42);
        assert_eq!(value["message_id"], 7);
        assert_eq!(value["timeout_ms"], 30_000);
        // 插件只交内容：chat_id / reply_to 由宿主自己拼。
        assert_eq!(value["payload"]["input_message_content"], content);
        assert!(value["payload"].get("reply_to").is_none());
    }

    #[test]
    fn final_message_is_returned_as_is() {
        let message = parse_message(r#"{"@type":"message","id":99,"chat_id":42}"#).unwrap();
        assert_eq!(message["id"], 99);
    }

    #[test]
    fn tdlib_error_becomes_op_error() {
        let error = parse_message(r#"{"@type":"error","code":400,"message":"MESSAGE_ID_INVALID"}"#)
            .unwrap_err();
        match error {
            Error::Op { code, message } => {
                assert_eq!(code, "400");
                assert_eq!(message, "MESSAGE_ID_INVALID");
            }
            other => panic!("期望 Error::Op，实得 {other:?}"),
        }
    }

    #[test]
    fn empty_or_garbage_reply_is_a_host_error() {
        assert!(matches!(parse_message(""), Err(Error::Host(_))));
        assert!(matches!(parse_message("   "), Err(Error::Host(_))));
        assert!(matches!(parse_message("not json"), Err(Error::Host(_))));
    }
}
