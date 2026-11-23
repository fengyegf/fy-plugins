//! 发送 / 编辑 / 删除：全部经 simplify 的 `send_message` / `edit_message_text` /
//! `delete_messages`，插件不自己拼 `sendMessage` 的嵌套参数。
//!
//! 两条约定：
//! - 纯文本必须给完整的 `inputMessageText`（TDLib 只认 `formattedText`，塞字符串会被拒）；
//! - 带 `parse_mode` 时把「文本字段」写成宿主认的 `{"text":…,"parse_mode":…}`，
//!   由宿主走 `parseTextEntities`，插件不自己实现 Markdown 解析。
//!
//! 富消息先跑**同步**的 `rich_from_markdown` 拿内容对象，再塞进 `send_message`；
//! 任何一步失败都原样冒泡，由调用方逐级降级（rich → markdown → plain）。

use serde_json::{Value, json};

use crate::core::{AgentError, AgentResult};

use super::{SentMessage, TdClient};

/// 文本内容：`parse_mode` 有值时交给宿主解析，否则自己拼 `formattedText`。
pub(super) fn text_content(text: &str, parse_mode: Option<&str>) -> Value {
    match parse_mode.map(str::trim).filter(|mode| !mode.is_empty()) {
        Some(mode) => json!({
            "@type": "inputMessageText",
            "text": { "text": text, "parse_mode": mode },
        }),
        None => json!({
            "@type": "inputMessageText",
            "text": { "@type": "formattedText", "text": text, "entities": [] },
        }),
    }
}

/// 拼 `send_message` 的 args：话题 / 引用只在有值时出现（不是发 `null`）。
pub(super) fn send_args(
    chat_id: i64,
    content: Value,
    topic_id: Option<i32>,
    reply_to: Option<i64>,
    timeout_ms: u64,
) -> Value {
    let mut args = json!({
        "chat_id": chat_id,
        "input_message_content": content,
        "timeout_ms": timeout_ms,
    });
    if let Some(topic_id) = topic_id {
        args["topic_id"] = topic_arg(topic_id);
    }
    if let Some(message_id) = reply_to {
        args["reply_to"] = reply_arg(message_id);
    }
    args
}

/// 拼 `edit_message_text` 的 args。
pub(super) fn edit_args(chat_id: i64, message_id: i64, content: Value, timeout_ms: u64) -> Value {
    json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "input_message_content": content,
        "timeout_ms": timeout_ms,
    })
}

/// TDLib 的 `MessageTopic`：论坛话题（「与 bot 的私聊话题」也用它）。
pub(super) fn topic_arg(topic_id: i32) -> Value {
    json!({ "@type": "messageTopicForum", "forum_topic_id": topic_id })
}

/// TDLib 的 `InputMessageReplyTo`：引用一条消息。
pub(super) fn reply_arg(message_id: i64) -> Value {
    json!({ "@type": "inputMessageReplyToMessage", "message_id": message_id })
}

/// 发文本。
pub(super) fn send_text(
    client: &TdClient,
    chat_id: i64,
    topic_id: Option<i32>,
    text: &str,
    parse_mode: Option<&str>,
    reply_to: Option<i64>,
    timeout_ms: u64,
) -> AgentResult<SentMessage> {
    let args = send_args(
        chat_id,
        text_content(text, parse_mode),
        topic_id,
        reply_to,
        timeout_ms,
    );
    match client.run_op("send_message", args, timeout_ms)? {
        Some(message) => Ok(sent_from(&message, chat_id, 0)),
        // 回放：请求已留档，没有真实消息号；offline 让上层别拿它去编辑。
        None => Ok(SentMessage {
            chat_id,
            message_id: 0,
            offline: true,
        }),
    }
}

/// 发富消息：Markdown → 本地块树（同步 op）→ `send_message`。
pub(super) fn send_rich(
    client: &TdClient,
    chat_id: i64,
    topic_id: Option<i32>,
    markdown: &str,
    reply_to: Option<i64>,
    timeout_ms: u64,
) -> AgentResult<SentMessage> {
    let content = rich_content(client, markdown)?;
    let args = send_args(chat_id, content, topic_id, reply_to, timeout_ms);
    match client.run_op("send_message", args, timeout_ms)? {
        Some(message) => Ok(sent_from(&message, chat_id, 0)),
        None => Ok(SentMessage {
            chat_id,
            message_id: 0,
            offline: true,
        }),
    }
}

/// 编辑成文本。
pub(super) fn edit_text(
    client: &TdClient,
    chat_id: i64,
    message_id: i64,
    text: &str,
    parse_mode: Option<&str>,
    timeout_ms: u64,
) -> AgentResult<Option<SentMessage>> {
    let args = edit_args(
        chat_id,
        message_id,
        text_content(text, parse_mode),
        timeout_ms,
    );
    match client.run_op("edit_message_text", args, timeout_ms)? {
        Some(message) => Ok(Some(sent_from(&message, chat_id, message_id))),
        None => Ok(None),
    }
}

/// 编辑成富消息：Markdown → 本地块树（同步 op）→ `edit_message_text`。
pub(super) fn edit_rich(
    client: &TdClient,
    chat_id: i64,
    message_id: i64,
    markdown: &str,
    timeout_ms: u64,
) -> AgentResult<Option<SentMessage>> {
    let content = rich_content(client, markdown)?;
    let args = edit_args(chat_id, message_id, content, timeout_ms);
    match client.run_op("edit_message_text", args, timeout_ms)? {
        Some(message) => Ok(Some(sent_from(&message, chat_id, message_id))),
        None => Ok(None),
    }
}

/// 删除一批消息；空列表不发请求。
pub(super) fn delete_messages(
    client: &TdClient,
    chat_id: i64,
    message_ids: &[i64],
    revoke: bool,
    timeout_ms: u64,
) -> AgentResult<()> {
    if message_ids.is_empty() {
        return Ok(());
    }
    let args = json!({
        "chat_id": chat_id,
        "message_ids": message_ids,
        "revoke": revoke,
        "timeout_ms": timeout_ms,
    });
    client.run_op("delete_messages", args, timeout_ms)?;
    Ok(())
}

/// Markdown → `inputRichMessage`（同步 op，不碰网络，回放里也能跑）。
///
/// 宿主 `rich_from_markdown` 回的是内容包装（`inputMessageRichText`，里面带
/// `rich_message`）；TDLib 1.8.70 真正认的是 `inputMessageRichMessage { message, clear_draft }`，
/// 所以这里只取 `inputRichMessage`，包装交给调用方按场景拼（发消息 / 草稿）。
pub(super) fn rich_message(client: &TdClient, markdown: &str) -> AgentResult<Value> {
    let content = client.run_sync_op("rich_from_markdown", json!({ "markdown": markdown }))?;
    rich_message_from_content(&content)
}

/// 从宿主返回的内容里取出 `inputRichMessage`（两种形状都认）。
fn rich_message_from_content(content: &Value) -> AgentResult<Value> {
    if content.is_null() {
        return Err(AgentError::Td("rich_from_markdown 没有返回内容".to_owned()));
    }
    let message = content
        .get("rich_message")
        .cloned()
        .unwrap_or_else(|| content.clone());
    if message.get("@type").and_then(Value::as_str) != Some("inputRichMessage") {
        return Err(AgentError::Td(format!(
            "rich_from_markdown 返回了非 inputRichMessage：{}",
            message
                .get("@type")
                .and_then(Value::as_str)
                .unwrap_or("缺少 @type")
        )));
    }
    Ok(message)
}

/// Markdown → `inputMessageRichMessage`（sendMessage / editMessageText 的内容）。
fn rich_content(client: &TdClient, markdown: &str) -> AgentResult<Value> {
    Ok(json!({
        "@type": "inputMessageRichMessage",
        "message": rich_message(client, markdown)?,
        "clear_draft": true,
    }))
}

/// 把应答（最终 `Message`）折成 [`SentMessage`]；缺字段时退回入参给的定位。
fn sent_from(message: &Value, chat_id: i64, fallback_message_id: i64) -> SentMessage {
    SentMessage {
        chat_id: message
            .get("chat_id")
            .and_then(Value::as_i64)
            .unwrap_or(chat_id),
        message_id: message
            .get("id")
            .and_then(Value::as_i64)
            .unwrap_or(fallback_message_id),
        offline: false,
    }
}
