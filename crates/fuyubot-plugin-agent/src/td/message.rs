//! 被引用消息：`getRepliedMessage` → `getMessage` 兜底 → 折成可读文本。
//!
//! 为什么单列一层：`messageReplyToMessage` 里只有 `message_id` 和用户选中的片段，
//! 正文得再问 TDLib 要；而取回的消息形状很多（纯文本 / 媒体 caption / 富消息块树），
//! 拍平逻辑必须是纯函数。这一层只负责「取」和「折」，渲染交给 [`crate::core::quote`]。

use fuyubot_plugin_api::LogLevel;
use serde_json::{Value, json};

use crate::core::{AgentResult, IncomingMessage, QuoteAuthor, QuotedMessage};

use super::{LOOKUP_TIMEOUT_MS, TdClient, update};

/// 取被引用的消息。
///
/// 取不到（已删除 / 回放 / 权限不足）返回 `None`——上层照样会保留「引用了 #id」的标记，
/// 只是没有正文：引用是锦上添花，绝不能因为它失败整轮不回复。
pub(super) fn replied(
    client: &TdClient,
    message: &IncomingMessage,
) -> AgentResult<Option<QuotedMessage>> {
    let Some(reply) = message.reply.as_ref() else {
        return Ok(None);
    };
    // 先问「回复链」：跨话题、跨群、媒体组都归它处理。
    let request = json!({
        "@type": "getRepliedMessage",
        "chat_id": message.chat_id,
        "message_id": message.message_id,
    });
    if let Some(value) = invoke_message(client, &request) {
        return Ok(Some(parse(client, &value)));
    }
    // 兜底：按被引用消息的 chat + id 直接取（老版本 / 回复链被拒时）。
    let request = json!({
        "@type": "getMessage",
        "chat_id": reply.chat_id.unwrap_or(message.chat_id),
        "message_id": reply.message_id,
    });
    Ok(invoke_message(client, &request).map(|value| parse(client, &value)))
}

/// 发请求并只认返回 `message` 的应答；错误 / 回放一律 `None`（只记 debug 日志）。
fn invoke_message(client: &TdClient, request: &Value) -> Option<Value> {
    let op = request
        .get("@type")
        .and_then(Value::as_str)
        .unwrap_or("getMessage");
    let value = match client.run_invoke(request.clone(), LOOKUP_TIMEOUT_MS) {
        Ok(value) => value,
        Err(error) => {
            client
                .host()
                .log(LogLevel::Debug, &format!("agent: {op} 失败：{error}"));
            return None;
        }
    }?;
    if value.get("@type").and_then(Value::as_str) != Some("message") {
        client
            .host()
            .log(LogLevel::Debug, &format!("agent: {op} 没有返回消息对象"));
        return None;
    }
    Some(value)
}

/// 把 TDLib 的 `message` 折成可读快照。
fn parse(client: &TdClient, message: &Value) -> QuotedMessage {
    let message_id = message.get("id").and_then(Value::as_i64).unwrap_or(0);
    let content = message.get("content");
    let content_type = content
        .and_then(|content| content.get("@type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let mut text = content.map(readable_text).unwrap_or_default();
    if text.trim().is_empty() {
        // 纯媒体（图片 / 视频 / 贴纸…）：给个类型占位，别让模型以为引用是空的。
        text = format!("[{content_type}]");
    }
    QuotedMessage {
        message_id,
        author: author(client, message),
        text,
        content_type,
    }
}

/// 内容 → 可读正文：文本 / caption 直接取，富消息把块树拍平。
pub(super) fn readable_text(content: &Value) -> String {
    let plain = update::content_text(content);
    if !plain.trim().is_empty() {
        return plain;
    }
    if content.get("@type").and_then(Value::as_str) == Some("messageRichMessage") {
        return rich_message_text(content.get("message"));
    }
    String::new()
}

/// 富消息 → 纯文本：块级一行一条，行内富文本拍成纯字符串。
fn rich_message_text(message: Option<&Value>) -> String {
    let blocks = message
        .and_then(|message| message.get("blocks"))
        .and_then(Value::as_array);
    let Some(blocks) = blocks else {
        return String::new();
    };
    let mut lines = Vec::new();
    for block in blocks {
        push_block_lines(block, &mut lines);
    }
    lines.join("\n")
}

/// 一个页块 → 若干行（容器块递归展开，自己再补一行行内文字）。
///
/// 顺序是「子块 → 本块文字」：标题类块没有子块，不受影响；引用块（blocks + credit）
/// 正好把出处放在正文后面。
fn push_block_lines(block: &Value, lines: &mut Vec<String>) {
    if let Some(nested) = block.get("blocks").and_then(Value::as_array) {
        for child in nested {
            push_block_lines(child, lines);
        }
    }
    // 列表项：`{label, label_type, blocks}`；没有 blocks 的按行内文本处理。
    if let Some(items) = block.get("items").and_then(Value::as_array) {
        for item in items {
            match item.get("blocks").and_then(Value::as_array) {
                Some(nested) => {
                    for child in nested {
                        push_block_lines(child, lines);
                    }
                }
                None => push_line(lines, rich_text(item)),
            }
        }
    }
    // 封面包装（pageBlockCover）。
    if let Some(cover) = block.get("cover").filter(|cover| cover.is_object()) {
        push_block_lines(cover, lines);
    }
    // 表格：一行一条，单元格用 ` | ` 分隔。
    if let Some(rows) = block.get("cells").and_then(Value::as_array) {
        for row in rows {
            let cells: Vec<String> = row
                .as_array()
                .map(|cells| {
                    cells
                        .iter()
                        .map(rich_text)
                        .filter(|cell| !cell.trim().is_empty())
                        .collect()
                })
                .unwrap_or_default();
            push_line(lines, cells.join(" | "));
        }
    }
    // 本块自己的文字：段落 / 标题 / 引用 / 代码 / 公式…
    push_line(lines, block_text(block));
}

/// 块自己的行内文字：按常见字段名取第一个非空的。
fn block_text(block: &Value) -> String {
    for key in [
        "text",
        "title",
        "subtitle",
        "header",
        "subheader",
        "kicker",
        "footer",
        "author",
        "credit",
        "expression",
        "alternative_text",
        "caption",
    ] {
        if let Some(value) = block.get(key) {
            // 块级公式（pageBlockMathematicalExpression）直接带表达式字符串，用 $ 包一下。
            if key == "expression"
                && let Some(expression) = value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            {
                return format!("${expression}$");
            }
            let text = rich_text(value);
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    String::new()
}

/// 追加一行（空行丢掉，避免富消息里的大量空块把上下文撑出空行）。
fn push_line(lines: &mut Vec<String>, text: String) {
    let text = text.trim();
    if !text.is_empty() {
        lines.push(text.to_owned());
    }
}

/// 行内富文本（`richText*` / `formattedText`）→ 纯字符串。
pub(super) fn rich_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(rich_text).collect(),
        Value::Object(map) => {
            // 叶子：`text` 是字符串（richTextPlain / formattedText）。
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return text.to_owned();
            }
            // 自定义表情 / 公式没有普通文本，用它们的替代写法。
            if let Some(alternative) = map.get("alternative_text").and_then(Value::as_str) {
                return alternative.to_owned();
            }
            if let Some(expression) = map.get("expression").and_then(Value::as_str) {
                return format!("${expression}$");
            }
            // 容器：按固定顺序拼（粗体 / 斜体包装的 text、textConcat 的 texts、表格的 cells…）。
            let mut out = String::new();
            for key in ["texts", "text", "cells", "blocks"] {
                if let Some(child) = map.get(key) {
                    out.push_str(&rich_text(child));
                }
            }
            if out.is_empty() {
                // 未知形状的兜底：把所有非元信息字段拼起来（serde_json 的 Map 是字典序，
                // 只用于兜底，常见容器都走上一条路径）。
                for (key, child) in map {
                    if matches!(key.as_str(), "@type" | "entities" | "type") {
                        continue;
                    }
                    out.push_str(&rich_text(child));
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// 作者：outgoing = 自己发的；用户消息查一次展示名（宿主缓存优先）；匿名管理员按群身份记。
fn author(client: &TdClient, message: &Value) -> QuoteAuthor {
    if message
        .get("is_outgoing")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return QuoteAuthor::Me;
    }
    let sender = message.get("sender_id");
    match sender
        .and_then(|sender| sender.get("@type"))
        .and_then(Value::as_str)
    {
        Some("messageSenderUser") => {
            let Some(user_id) = sender
                .and_then(|sender| sender.get("user_id"))
                .and_then(Value::as_i64)
            else {
                return QuoteAuthor::Unknown;
            };
            match client.user(user_id) {
                Ok(Some(user)) => QuoteAuthor::User(user.display()),
                // 查不到（回放 / 未登录 / 已注销）：至少把 id 给模型。
                _ => QuoteAuthor::User(user_id.to_string()),
            }
        }
        Some("messageSenderChat") => QuoteAuthor::Chat,
        _ => QuoteAuthor::Unknown,
    }
}
