//! 回复通道：命令回复走宿主的 `fuyubot.host.command.reply`，编辑走 simplify。
//!
//! 为什么不自己拼 `sendMessage`：发新消息 / 编辑原命令 / ephemeral 三种形态由宿主按
//! 「账号类型 + 触发者」决定，插件插手只会和宿主打架（本仓 kfc 的注释记过这个坑）。
//!
//! **同步阻塞**：只能在业务线程里调，绝不能在 `updateNewMessage` 回调里调。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

/// 宿主预置的命令回复服务（契约名，不是插件名）。
const REPLY_SERVICE: &str = "fuyubot.host.command";
/// 服务方法名。
const REPLY_METHOD: &str = "reply";

/// 一条已发出的消息定位。
#[derive(Debug, Clone, Copy)]
pub struct Sent {
    /// 最终落在哪个会话。
    pub chat_id: i64,
    /// 消息 id（编辑占位消息要用它）。
    pub message_id: i64,
}

/// 拼 `inputMessageText`（TDLib 只认 formattedText，给纯字符串会 400）。
pub fn text_content(text: &str) -> Value {
    json!({
        "@type": "inputMessageText",
        "text": { "@type": "formattedText", "text": text, "entities": [] },
    })
}

/// 回复一条命令消息。
pub fn reply(
    host: &HostApi,
    chat_id: i64,
    message_id: i64,
    text: &str,
    timeout: Duration,
) -> Result<Sent, String> {
    let input = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "timeout_ms": timeout.as_millis() as u64,
        "payload": { "input_message_content": text_content(text) },
    });
    let raw = host
        .call_service(REPLY_SERVICE, REPLY_METHOD, &input.to_string())
        .map_err(|error| format!("{REPLY_SERVICE}.{REPLY_METHOD} 调用失败：{error}"))?;
    parse_sent(&raw, chat_id)
}

/// 解析宿主的应答：最终 `Message`，失败是 `{"@type":"error"}`。
fn parse_sent(raw: &str, fallback_chat_id: i64) -> Result<Sent, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("回复结果不是 JSON：{error}"))?;
    if value.get("@type").and_then(Value::as_str) == Some("error") {
        return Err(format!(
            "宿主回复失败：{} {}",
            value.get("code").and_then(Value::as_i64).unwrap_or(0),
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未提供原因")
        ));
    }
    Ok(Sent {
        chat_id: value
            .get("chat_id")
            .and_then(Value::as_i64)
            .unwrap_or(fallback_chat_id),
        message_id: value.get("id").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// 编辑一条已发消息（「识别中…」→ 结果）。
///
/// 失败只返回错误字符串：编辑失败不该让整条命令失败（结果已经识别出来了）。
pub fn edit(
    host: &HostApi,
    chat_id: i64,
    message_id: i64,
    text: &str,
    timeout: Duration,
) -> Result<(), String> {
    let op = json!({
        "op": "edit_message_text",
        "args": {
            "chat_id": chat_id,
            "message_id": message_id,
            "input_message_content": text_content(text),
        },
    });
    let request_id = host
        .simplify_invoke_json(op)
        .map_err(|error| format!("edit_message_text 发送失败：{error}"))?;
    let response = host
        .wait_for_update_json(
            json!({ "event_type": "response", "request_id": request_id.id }),
            timeout.as_millis() as u64,
        )
        .ok_or_else(|| "edit_message_text 等待响应超时".to_owned())?;
    let result = response.get("extra").cloned().unwrap_or(response);
    if result.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let code = result
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if code == "MESSAGE_NOT_MODIFIED" {
        // 文本没变（例如两次结果相同）—— 不是错误。
        return Ok(());
    }
    Err(format!(
        "edit_message_text 失败：{code} {}",
        result
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    ))
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
    }

    #[test]
    fn parses_sent_message() {
        let sent = parse_sent(r#"{"id":42,"chat_id":7}"#, 0).unwrap();
        assert_eq!(sent.chat_id, 7);
        assert_eq!(sent.message_id, 42);
    }

    #[test]
    fn error_response_is_reported() {
        let error = parse_sent(r#"{"@type":"error","code":400,"message":"bad"}"#, 0).unwrap_err();
        assert!(error.contains("400"));
    }
}
