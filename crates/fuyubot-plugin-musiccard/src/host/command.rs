//! 命令回复通道：宿主预置服务 `fuyubot.host.command` 的 `reply`。
//!
//! 回复某条命令消息时**不要**自己拼 `sendMessage`：把 `sendMessage` 的参数
//! （`input_message_content`，可带 `reply_markup` / `topic_id`）连同命令消息的
//! `chat_id` / `message_id` 交给宿主，由它按账号类型与触发者决定：
//!
//! - bot / user 账户白名单用户 → 发新消息 + `reply_to` 那条命令；
//! - user 账户自己触发 → 编辑原命令消息；
//! - 群里的 ephemeral 命令 → `sendEphemeralMessage`（只对执行者可见）。
//!
//! 契约（宿主服务表）：
//!
//! - 入参 `{"chat_id":…,"message_id":…,"timeout_ms":…,"payload":{…}}`，
//!   `message_id` 指向**命令消息**；
//! - 成功回 TDLib 的最终 `Message`（带 `"id"`）；失败回 `{"@type":"error",…}`。
//!
//! 这是**同步阻塞**调用：宿主当场等 TDLib 的最终消息，所以只能在本插件自己的
//! 业务线程里调（`cmd::run` 起的线程），绝不能在 `on_update` 回调里调——那会把
//! 宿主的接收循环堵死。

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use crate::host::messenger::{Error, Result};

/// 宿主预置的命令回复服务（方法 `reply`）。跨仓契约名，不是插件名。
pub(crate) const SERVICE: &str = "fuyubot.host.command";

/// 拼 `reply` 的入参。抽成纯函数，单测不必造 `HostApi`。
pub(crate) fn input(chat_id: i64, message_id: i64, timeout_ms: u64, content: &Value) -> Value {
    json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "timeout_ms": timeout_ms,
        "payload": { "input_message_content": content },
    })
}

/// 把一条消息内容当作「对命令的回复」发出去，返回 TDLib 的最终 `Message`。
///
/// **同步阻塞**：只能在工作线程上调用。
pub(crate) fn reply(
    host: &HostApi,
    chat_id: i64,
    message_id: i64,
    timeout_ms: u64,
    content: &Value,
) -> Result<Value> {
    let input = input(chat_id, message_id, timeout_ms, content).to_string();
    let raw = host
        .call_service(SERVICE, "reply", &input)
        .map_err(|error| Error::Host(format!("{SERVICE}.reply: {error}")))?;
    parse_reply(&raw)
}

/// 解析宿主的应答：`{"@type":"error",…}` 视为失败并映射成 [`Error::Op`]，
/// 其余当 TDLib 的最终 `Message` 原样返回。抽成纯函数，单测直接喂字符串。
fn parse_reply(raw: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(raw).map_err(|error| Error::Json(error.to_string()))?;
    if value.get("@type").and_then(Value::as_str) == Some("error") {
        // TDLib 的失败是对象而不是错误码：{`@type`:"error", code, message}。
        let code = match value.get("code") {
            Some(Value::String(code)) => code.clone(),
            Some(code) if !code.is_null() => code.to_string(),
            _ => "unknown".to_owned(),
        };
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("未提供错误信息")
            .to_owned();
        return Err(Error::Op { code, message });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_content() -> Value {
        json!({
            "@type": "inputMessageText",
            "text": { "@type": "formattedText", "text": "hi", "entities": [] },
        })
    }

    #[test]
    fn input_carries_the_command_context_and_payload() {
        let content = text_content();
        let value = input(-100, 7, 30_000, &content);
        assert_eq!(value["chat_id"], -100);
        assert_eq!(value["message_id"], 7);
        assert_eq!(value["timeout_ms"], 30_000);
        assert_eq!(value["payload"]["input_message_content"], content);
        // 回复形态由宿主决定：插件不许自己塞 reply_to / chat_id 进 sendMessage 参数。
        assert!(value["payload"].get("reply_to").is_none());
        assert!(value["payload"].get("chat_id").is_none());
    }

    #[test]
    fn final_message_is_returned_as_is() {
        let message = parse_reply(r#"{"@type":"message","id":99,"chat_id":-100}"#).unwrap();
        assert_eq!(message["id"], 99);
        assert_eq!(message["chat_id"], -100);
    }

    #[test]
    fn tdlib_error_becomes_op_error() {
        let error = parse_reply(r#"{"@type":"error","code":400,"message":"Bad Request"}"#)
            .expect_err("TDLib 的 error 必须当失败");
        assert!(
            matches!(error, Error::Op { ref code, ref message } if code == "400" && message == "Bad Request")
        );
    }

    #[test]
    fn non_json_reply_is_a_json_error() {
        assert!(matches!(parse_reply("not json"), Err(Error::Json(_))));
    }
}
