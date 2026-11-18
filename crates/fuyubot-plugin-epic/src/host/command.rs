//! `fuyubot.host.command` 客户端：把「回复某条命令消息」交给宿主决定形态。
//!
//! ABI v6 契约（见 FuyuBot 文档 · 插件开发 · 调用 TDLib 第 4 条）：
//!
//! - 服务名 `fuyubot.host.command`，方法 `reply`；
//! - 入参 `{"chat_id":…,"message_id":…,"timeout_ms":…,"payload":{"input_message_content":…}}`；
//! - 成功回 TDLib 最终 `Message`（含 `id`），失败回 `{"@type":"error","code":…,"message":…}`。
//!
//! 插件**不再自己拼** `sendMessage` 的 `chat_id` / `reply_to`：宿主拿到「命令消息 id」后
//! 自己决定发什么——bot / 白名单发新消息并挂 `reply_to`，用户号自己触发的命令编辑原消息，
//! 群里的 ephemeral 命令走 `sendEphemeralMessage`。插件只给内容。
//!
//! 这是**同步阻塞**调用：只能在插件自己的业务线程里调（本插件是 `epic-task` 线程），
//! 禁止在 `updateNewMessage` 回调里调——回调跑在 TDLib 接收线程上，阻塞它会把自己等死。

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use super::messenger::{Error, Sent};

/// 宿主命令服务名（契约名，不是插件名）。
pub const SERVICE: &str = "fuyubot.host.command";
/// 回复命令消息的方法名。
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

/// 调 `fuyubot.host.command.reply`；成功时返回最终消息的定位信息。
pub fn reply(
    host: &HostApi,
    chat_id: i64,
    message_id: i64,
    content: &Value,
    timeout_ms: u64,
) -> Result<Sent, Error> {
    let input = input(chat_id, message_id, timeout_ms, content).to_string();
    let raw = host
        .call_service(SERVICE, METHOD, &input)
        .map_err(|error| Error::Host(error.to_string()))?;
    // NULL = 失败（已经进了 AbiError）；空串 = 成功但提供方没有输出。
    if raw.is_empty() {
        return Err(Error::Host(format!("{SERVICE}.{METHOD} 没有返回结果")));
    }
    parse_reply(&raw, chat_id)
}

/// 解析应答：TDLib `Message` 取 `id` / `chat_id`；`{"@type":"error",…}` 转成 [`Error::Op`]。
fn parse_reply(raw: &str, chat_id: i64) -> Result<Sent, Error> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| Error::Host(format!("{SERVICE}.{METHOD} 返回的不是 JSON（{error}）")))?;
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
        // 以宿主返回的 chat_id 为准：它决定最终把消息落在哪个聊天。
        chat_id: value
            .get("chat_id")
            .and_then(Value::as_i64)
            .unwrap_or(chat_id),
        message_id: value.get("id").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// TDLib 的 `code` 是整数，宿主服务层偶尔给字符串——两种都吃掉。
pub fn reply_code(value: &Value) -> String {
    match value.get("code") {
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(code)) => code.clone(),
        _ => "unknown".to_owned(),
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
        // 不再自己塞 reply_to 进 sendMessage 参数：回复形态由宿主决定。
        assert!(value.get("reply_to").is_none(), "{value}");
        assert!(value["payload"].get("reply_to").is_none(), "{value}");
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
    fn non_json_reply_is_a_host_error() {
        assert!(matches!(parse_reply("not json", 42), Err(Error::Host(_))));
    }
}
