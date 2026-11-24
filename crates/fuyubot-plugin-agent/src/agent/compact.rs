//! 历史压缩：把会话转录成压缩器输入，以及压缩用的提示词。
//!
//! 压缩 = 一次普通网关调用（不带工具）：模型把「旧摘要 + 本轮之前的历史」压成一份新摘要。
//! 真正的调用编排在 [`super::AgentEngine::compress`]；这里只做纯文本处理，便于单测。

use serde_json::{Value, json};

use crate::core::{ChatMessage, consts};

/// 压缩器的系统提示。
pub const COMPACT_SYSTEM_PROMPT: &str = r#"你是会话压缩器。输入是一段 Telegram 对话（开头可能已经带一份先前摘要）。请把它压缩成一份「后续对话可以直接接着聊」的摘要。

要求：
- 保留：用户的目标 / 偏好 / 已确认的事实 / 关键数字 / 未完成的待办 / 尚未解决的问题 / 重要结论；
- 丢弃：寒暄、重复、已作废的中间过程、工具调用的细枝末节；
- 以第三人称陈述事实，不要角色扮演，不要复述原文对话；
- 用中文，Markdown 小标题 + 要点，控制在 800 字以内；
- 只输出摘要本身，不要任何前后缀说明。"#;

/// 把（旧摘要 +）消息列表拼成压缩器输入。
///
/// 单条消息与总长度都有上限：压缩是「把长的压短」，不是把整本聊天记录塞给模型；
/// 超长时保留**最近**的部分（最近发生了什么比一年前的寒暄更值得留）。
pub fn transcript(summary: &str, messages: &[ChatMessage]) -> String {
    let mut body = String::new();
    for message in messages {
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        let role = if message.role == "assistant" {
            "助手"
        } else {
            "用户"
        };
        body.push_str(role);
        body.push('：');
        body.push_str(&truncate_chars(content, consts::COMPACT_MESSAGE_MAX_CHARS));
        body.push('\n');
    }
    if body.chars().count() > consts::COMPACT_TRANSCRIPT_MAX_CHARS {
        body = format!(
            "（更早的对话过长，已省略）\n{}",
            tail_chars(&body, consts::COMPACT_TRANSCRIPT_MAX_CHARS)
        );
    }
    let summary = summary.trim();
    if summary.is_empty() {
        body
    } else {
        format!("【先前摘要】\n{summary}\n\n【之后的对话记录】\n{body}")
    }
}

/// 压缩请求的 messages：固定 system + 一段带转录的 user。
pub fn request_messages(transcript: &str) -> Vec<Value> {
    vec![
        json!({ "role": "system", "content": COMPACT_SYSTEM_PROMPT }),
        json!({
            "role": "user",
            "content": format!("请压缩下面的对话：\n\n{transcript}"),
        }),
    ]
}

/// 摘要规范化：压掉多余空行，截到 `SUMMARY_MAX_CHARS`。
pub fn normalize_summary(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank = 0usize;
    for line in raw.trim().lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    truncate_chars(out.trim(), consts::SUMMARY_MAX_CHARS)
}

/// 按字符截断（不按字节，避免切坏多字节字符）。
fn truncate_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// 取末尾 `limit` 个字符。
fn tail_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    text.chars().skip(count - limit).collect()
}
