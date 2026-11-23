//! 草稿消息（官方「流式回复」的 TDLib 形态）：`sendTextMessageDraft` / `sendRichMessageDraft`。
//!
//! 为什么不用「发一条消息再 editMessageText」：官方流式是**草稿**——空草稿由客户端显示成
//! 自带的 Thinking… 占位，同一 `draft_id` 反复更新会做增量动画，最后发一条正式消息落地，
//! 草稿自动消失。编辑真实消息既没有 Thinking 占位，也会在聊天里留下"先发后改"的痕迹。
//!
//! 参数来自 TDLib 1.8.70 的 schema：
//! `sendTextMessageDraft(chat_id, forum_topic_id, draft_id, can_stop, keep_on_stop, text)`
//! `sendRichMessageDraft(chat_id, forum_topic_id, draft_id, can_stop, keep_on_stop, message: inputRichMessage)`

use serde_json::{Value, json};

use super::TdClient;
use super::send::rich_message;
use crate::core::{AgentError, AgentResult};

/// 一次草稿发送的定位与选项（把 8 个参数收成一个，调用点也更好读）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftSend {
    pub chat_id: i64,
    /// 论坛话题 / bot 私聊话题；`None` 发到 General。
    pub topic_id: Option<i32>,
    /// 同一个 id 的草稿更新会做增量动画。
    pub draft_id: i64,
    /// 给用户显示「停止生成」按钮。
    pub can_stop: bool,
    /// 用户停止时保留当前草稿（false = 停止即消失）。
    pub keep_on_stop: bool,
    pub timeout_ms: u64,
}

/// 拼 `sendTextMessageDraft` 的请求体（纯函数，单测直接比对）。
pub(super) fn text_draft_request(send: &DraftSend, text: &str) -> Value {
    json!({
        "@type": "sendTextMessageDraft",
        "chat_id": send.chat_id,
        "forum_topic_id": send.topic_id.unwrap_or(0),
        "draft_id": send.draft_id,
        "can_stop": send.can_stop,
        "keep_on_stop": send.keep_on_stop,
        "text": { "@type": "formattedText", "text": text, "entities": [] },
    })
}

/// 拼 `sendRichMessageDraft` 的请求体：`message` 必须是 `inputRichMessage`。
pub(super) fn rich_draft_request(send: &DraftSend, message: &Value) -> AgentResult<Value> {
    if message.get("@type").and_then(Value::as_str) != Some("inputRichMessage") {
        return Err(AgentError::Td("富消息草稿需要 inputRichMessage".to_owned()));
    }
    Ok(json!({
        "@type": "sendRichMessageDraft",
        "chat_id": send.chat_id,
        "forum_topic_id": send.topic_id.unwrap_or(0),
        "draft_id": send.draft_id,
        "can_stop": send.can_stop,
        "keep_on_stop": send.keep_on_stop,
        "message": message,
    }))
}

/// 拼「思考占位」富草稿：一个 `inputPageBlockThinking` 块。
///
/// TDLib 1.8.70 有 `inputPageBlockThinking`（Bot API 的 `InputRichBlockThinking`），
/// 客户端会把它渲染成思考占位；比空文本草稿更接近官方富消息演示。
pub(super) fn thinking_draft_request(send: &DraftSend, text: &str) -> Value {
    json!({
        "@type": "sendRichMessageDraft",
        "chat_id": send.chat_id,
        "forum_topic_id": send.topic_id.unwrap_or(0),
        "draft_id": send.draft_id,
        "can_stop": send.can_stop,
        "keep_on_stop": send.keep_on_stop,
        "message": {
            "@type": "inputRichMessage",
            "source": {
                "@type": "richMessageSourceBlocks",
                "blocks": [{
                    "@type": "inputPageBlockThinking",
                    "text": { "@type": "richTextPlain", "text": text },
                }],
            },
            "is_rtl": false,
            "detect_automatic_blocks": false,
        },
    })
}

/// 发一条文本草稿（`text` 空 = 客户端自带的 Thinking… 占位）。
pub(super) fn send_text_draft(client: &TdClient, send: &DraftSend, text: &str) -> AgentResult<()> {
    let request = text_draft_request(send, text);
    client.run_invoke(request, send.timeout_ms).map(|_| ())
}

/// 发一条富消息草稿。
pub(super) fn send_rich_draft(
    client: &TdClient,
    send: &DraftSend,
    markdown: &str,
) -> AgentResult<()> {
    let message = rich_message(client, markdown)?;
    let request = rich_draft_request(send, &message)?;
    client.run_invoke(request, send.timeout_ms).map(|_| ())
}

/// 发一条「思考占位」富草稿。
pub(super) fn send_thinking_draft(
    client: &TdClient,
    send: &DraftSend,
    text: &str,
) -> AgentResult<()> {
    let request = thinking_draft_request(send, text);
    client.run_invoke(request, send.timeout_ms).map(|_| ())
}
