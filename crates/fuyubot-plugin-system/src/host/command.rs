//! 命令回复通道：把「回复某条命令消息」交给宿主的 `fuyubot.host.command.reply`。
//!
//! 宿主预置服务 `fuyubot.host.command`（契约名，不是插件名），方法 `reply`，入参
//!
//! ```json
//! {"chat_id":…,"message_id":…,"timeout_ms":…,"payload":{"input_message_content":<sendMessage 参数>}}
//! ```
//!
//! 成功返回 TDLib 的**最终 `Message`**（含 `id`），失败返回
//! `{"@type":"error","code":…,"message":…}`。
//!
//! 为什么不再自己拼 `sendMessage`：发新消息 + `reply_to`、编辑原命令消息、群里的
//! `sendEphemeralMessage` 三种形态由宿主按「账号类型 + 触发者 + `ephemeral_message_id`」
//! 决定。插件把 `chat_id` / `reply_to` 塞进 `sendMessage` 参数只会和宿主打架：
//! user 账户自己触发的命令本该被「编辑原命令」，插件却会发出一条新消息。
//!
//! **同步阻塞**：宿主当场等 TDLib 的最终消息（最多 `timeout_ms`）。只能在插件自己的
//! 工作线程（`system-task`）里调，绝不能在 `updateNewMessage` 回调里调——那里等响应
//! 就是自己等自己。
//!
//! 结构与 kfc / whois 的 `host/command.rs` 一致。

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use crate::host::messenger::{Error, Result, Sent};

/// 宿主预置的命令回复服务名（契约名，不是插件名）。
pub const SERVICE: &str = "fuyubot.host.command";

/// 服务方法名：回复一条命令。
const METHOD: &str = "reply";

/// 拼 `reply` 的入参。抽成纯函数：单测不必造 `HostApi`，也不会真发消息。
///
/// `message_id` 是**命令消息**的 id；`payload` 是 `sendMessage` 的参数，本通道只用
/// `input_message_content`。**不带** `reply_to`——回复形态由宿主决定。
pub fn input(chat_id: i64, message_id: i64, timeout_ms: u64, content: &Value) -> Value {
    json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "timeout_ms": timeout_ms,
        "payload": { "input_message_content": content },
    })
}

/// 调宿主回复一条命令；成功时返回最终消息的定位信息。
pub fn reply(
    host: &HostApi,
    chat_id: i64,
    message_id: i64,
    content: &Value,
    timeout_ms: u64,
) -> Result<Sent> {
    let input = input(chat_id, message_id, timeout_ms, content);
    let raw = host
        .call_service(SERVICE, METHOD, &input.to_string())
        .map_err(|error| Error::Host(error.to_string()))?;
    parse_reply(&raw, chat_id)
}

/// 解析宿主的应答。抽成纯函数，单测直接喂字符串。
fn parse_reply(raw: &str, chat_id: i64) -> Result<Sent> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| Error::Protocol(format!("{SERVICE}.{METHOD} 返回的不是 JSON：{error}")))?;
    // TDLib 的失败是以 `{"@type":"error",…}` 回来的，不是错误码，映射成 `Error::Op`。
    if value.get("@type").and_then(Value::as_str) == Some("error") {
        return Err(Error::Op {
            code: reply_code(&value),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未提供错误信息")
                .to_owned(),
        });
    }
    Ok(Sent {
        // 以宿主返回的 `chat_id` 为准：它决定最终把消息落在哪个聊天。
        chat_id: value
            .get("chat_id")
            .and_then(Value::as_i64)
            .unwrap_or(chat_id),
        message_id: value.get("id").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// TDLib 的 `code` 是整数，宿主服务层偶尔给字符串；两种都吃掉。
fn reply_code(error: &Value) -> String {
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
    fn input_wraps_content_under_payload() {
        let content = json!({ "@type": "inputMessageText" });
        let value = input(42, 7, 30_000, &content);
        assert_eq!(value["chat_id"], 42);
        assert_eq!(value["message_id"], 7);
        assert_eq!(value["timeout_ms"], 30_000u64);
        assert_eq!(value["payload"]["input_message_content"], content);
        // 不再自己塞 reply_to / chat_id 进 sendMessage 参数：回复形态由宿主决定。
        assert!(value.get("reply_to").is_none(), "{value}");
        assert!(value["payload"].get("reply_to").is_none(), "{value}");
        assert!(value["payload"].get("chat_id").is_none(), "{value}");
    }

    #[test]
    fn final_message_id_comes_from_the_reply() {
        let sent = parse_reply(r#"{"@type":"message","id":9001,"chat_id":42}"#, 42)
            .expect("正常应答应当解析成功");
        assert_eq!(sent.chat_id, 42);
        assert_eq!(sent.message_id, 9001);
    }

    #[test]
    fn tdlib_error_maps_to_op_error() {
        let error = parse_reply(
            r#"{"@type":"error","code":400,"message":"Bad Request"}"#,
            42,
        )
        .expect_err("error 应答必须失败");
        match error {
            Error::Op { code, message } => {
                assert_eq!(code, "400");
                assert_eq!(message, "Bad Request");
            }
            other => panic!("应当映射成 Error::Op，实际 {other:?}"),
        }
    }

    #[test]
    fn reply_code_accepts_int_and_string() {
        assert_eq!(reply_code(&json!({"code": 400})), "400");
        // 老宿主 / 服务层可能把 code 写成字符串。
        assert_eq!(
            reply_code(&json!({"code": "SERVICE_UNAVAILABLE"})),
            "SERVICE_UNAVAILABLE"
        );
        assert_eq!(reply_code(&json!({})), "unknown");
    }

    #[test]
    fn non_json_reply_is_a_protocol_error() {
        assert!(matches!(
            parse_reply("not json", 42),
            Err(Error::Protocol(_))
        ));
    }
}
