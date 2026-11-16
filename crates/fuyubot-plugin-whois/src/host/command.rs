//! `fuyubot.host.command` 客户端：把「回复某条命令消息」交给宿主决定形态。
//!
//! ABI v6 契约（见仓库 `docs/abi-v6-spec.md` 第 4 条）：
//!
//! - 服务名 `fuyubot.host.command`，方法 `reply`；
//! - 入参 `{"chat_id":…,"message_id":…,"timeout_ms":…,"payload":{"input_message_content":…}}`；
//! - 成功回 TDLib 最终 `Message`（含 `id`），失败回 `{"@type":"error","code":…,"message":…}`。
//!
//! 插件**不再自己拼** `sendMessage` 的 `chat_id` / `reply_to`：宿主拿到「命令消息 id」后
//! 自己决定发什么——bot / 白名单发新消息并挂 `reply_to`，用户号自己触发的命令编辑原消息，
//! 群里的 ephemeral 命令走 `sendEphemeralMessage`。插件只给内容。
//!
//! 这是**同步阻塞**调用：只能在插件自己的业务线程里调（本插件是 `whois-task` 线程），
//! 禁止在 `updateNewMessage` 回调里调——回调跑在 TDLib 接收线程上，阻塞它会把自己等死。

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use super::messenger::Error;

/// 宿主命令服务名（契约名，不是插件名）。
pub const SERVICE: &str = "fuyubot.host.command";
/// 回复命令消息的方法名。
const METHOD: &str = "reply";

/// 拼 `reply` 的入参。抽成纯函数，单测不必造 `HostApi`。
pub fn input(chat_id: i64, message_id: i64, timeout_ms: u64, content: &Value) -> Value {
    json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "timeout_ms": timeout_ms,
        "payload": { "input_message_content": content },
    })
}

/// 调 `fuyubot.host.command.reply`，成功时返回 TDLib 最终 `Message`。
///
/// 阻塞：宿主要等 TDLib 回包，只能在工作线程上调。
pub fn reply(
    host: &HostApi,
    chat_id: i64,
    message_id: i64,
    content: &Value,
    timeout_ms: u64,
) -> Result<Value, Error> {
    let input = input(chat_id, message_id, timeout_ms, content).to_string();
    let raw = host
        .call_service(SERVICE, METHOD, &input)
        .map_err(|error| Error::Host(error.to_string()))?;
    parse_reply(&raw)
}

/// 解析应答：TDLib `Message` 原样返回，`{"@type":"error",…}` 转成 [`Error::Op`]。
pub fn parse_reply(raw: &str) -> Result<Value, Error> {
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
    Ok(value)
}

/// TDLib 的 `code` 是整数，服务层失败有时给字符串——两种都收。
fn reply_code(value: &Value) -> String {
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
    fn input_carries_message_id_and_nested_content() {
        let content = json!({ "@type": "inputMessageText", "text": { "text": "hi" } });
        let value = input(-100123, 42, 30_000, &content);
        assert_eq!(value["chat_id"], -100123);
        assert_eq!(value["message_id"], 42);
        assert_eq!(value["timeout_ms"], 30_000);
        // 内容只能挂在 payload.input_message_content 下，宿主不认顶层同名字段。
        assert_eq!(value["payload"]["input_message_content"], content);
        assert!(value.get("reply_to").is_none(), "reply_to 归宿主填");
    }

    #[test]
    fn final_message_passes_through() {
        let message = parse_reply(r#"{"@type":"message","id":77,"chat_id":-1}"#).unwrap();
        assert_eq!(message["id"], 77);
    }

    #[test]
    fn tdlib_error_becomes_op_error() {
        let error = parse_reply(r#"{"@type":"error","code":400,"message":"message not found"}"#)
            .unwrap_err();
        match error {
            Error::Op { code, message } => {
                assert_eq!(code, "400");
                assert_eq!(message, "message not found");
            }
            other => panic!("应当是 Op 错误，实际 {other:?}"),
        }
    }

    #[test]
    fn non_json_reply_is_a_host_error() {
        assert!(matches!(parse_reply("boom"), Err(Error::Host(_))));
    }
}
