//! 引用回复的上下文块（纯函数，可单测）。
//!
//! 用户在 Telegram 里点「回复」时，`updateNewMessage` 只带被引用消息的 id（外加可能的
//! 手动选中片段）——模型看到一句「这个呢？」根本不知道指的是哪条。这里把 TDLib 取回的
//! 被引用消息折成一段带标记的上下文，拼在本轮用户输入前面。

use super::types::{QuotedMessage, ReplyRef};

/// 引用块结束标记。
const CLOSE: &str = "[引用结束]";

/// 渲染引用块。
///
/// - 有正文：`[引用消息 #id｜发送者：xxx]` + 正文（或选中片段 + 原文）+ `[引用结束]`；
/// - 取不到正文（已删除 / 回放 / 权限不足）：只留 `[引用消息 #id]` 与说明，
///   让模型知道「用户在回复某条消息」，好过让它凭空猜；
/// - `max_chars` 只截正文与片段，标记本身不截。
pub fn render_quote_block(
    reply: &ReplyRef,
    quoted: Option<&QuotedMessage>,
    max_chars: usize,
) -> String {
    let mut out = String::new();
    match quoted {
        Some(quoted) => out.push_str(&format!(
            "[引用消息 #{}｜发送者：{}]\n",
            reply.message_id,
            quoted.author.label()
        )),
        None => out.push_str(&format!("[引用消息 #{}]\n", reply.message_id)),
    }

    // 用户手动选中的片段比整条消息更贴近「他到底在说什么」，放最前面。
    let fragment = reply
        .quote
        .as_deref()
        .map(str::trim)
        .filter(|quote| !quote.is_empty());
    let fragment_shown = match (fragment, quoted) {
        (Some(fragment), Some(quoted)) => fragment != quoted.text.trim(),
        (Some(_), None) => true,
        (None, _) => false,
    };
    if let (true, Some(fragment)) = (fragment_shown, fragment) {
        out.push_str(&format!(
            "用户选中的片段：{}\n",
            truncate_chars(fragment, max_chars)
        ));
    }

    match quoted {
        Some(quoted) => {
            let text = truncate_chars(quoted.text.trim(), max_chars);
            if text.is_empty() {
                out.push_str("（这条消息没有可读文本）\n");
            } else if fragment_shown {
                out.push_str(&format!("原文：{text}\n"));
            } else {
                out.push_str(&text);
                out.push('\n');
            }
        }
        // 有选中片段时就不用再说「取不到正文」了：片段本身就是线索。
        None if !fragment_shown => {
            out.push_str("（取不到被引用的消息：可能已删除或读取不到）\n");
        }
        None => {}
    }

    out.push_str(CLOSE);
    out.push_str("\n\n");
    out
}

/// 按字符截断（不按字节，避免切坏多字节字符）；`limit = 0` 视为不限。
fn truncate_chars(text: &str, limit: usize) -> String {
    if limit == 0 || text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// 去掉开头那段引用块（会话标题这类「只要用户原话」的地方用）。
///
/// 块没闭合（被截断 / 手工拼接）时原样返回：宁可标题难看一点，也不要吞掉用户的话。
pub fn strip_quote_block(text: &str) -> &str {
    const OPEN: &str = "[引用消息 ";
    const CLOSE: &str = "[引用结束]";
    let Some(rest) = text.strip_prefix(OPEN) else {
        return text;
    };
    match rest.find(CLOSE) {
        // 结束标记后面固定跟两个换行，去掉它们再从用户原话开始。
        Some(index) => rest[index + CLOSE.len()..].trim_start(),
        None => text,
    }
}
