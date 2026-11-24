//! `updateNewMessage` 的纯解析：没有宿主、没有状态，单测直接喂 JSON。
//!
//! 为什么单列一层：更新是**外部输入**，字段缺失 / 类型不对都不能 panic；
//! 把它折成 [`IncomingMessage`](crate::core::IncomingMessage) 之后，准入、防环、
//! 落盘都只看这个结构，不再碰原始 JSON。

use serde_json::Value;

use crate::core::{ChatKind, IncomingMessage, ReplyRef};

/// 一条 `updateNewMessage` → [`IncomingMessage`]；其它更新 / 缺关键字段返回 `None`。
pub(super) fn parse_incoming(update: &Value) -> Option<IncomingMessage> {
    if update.get("@type").and_then(Value::as_str) != Some("updateNewMessage") {
        return None;
    }
    let message = update.get("message")?;
    let chat_id = message.get("chat_id").and_then(Value::as_i64)?;
    let message_id = message.get("id").and_then(Value::as_i64)?;
    let content = message.get("content");
    let content_type = content
        .and_then(|content| content.get("@type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    Some(IncomingMessage {
        chat_id,
        message_id,
        // 裸消息里没有 chat 类型：私聊 id 恒为正，群 / 频道为负。
        // 群与频道要 getChat 才分得开，这里统一按群处理（频道由 worker 忽略）。
        kind: if chat_id > 0 {
            ChatKind::Private
        } else {
            ChatKind::Group
        },
        sender_user_id: sender_user_id(message),
        sender_chat_id: sender_chat_id(message),
        // 由调用方（worker）查一次 is_bot 后回填：解析层不碰 TDLib。
        sender_is_bot: false,
        is_outgoing: message
            .get("is_outgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        text: content.map(content_text).unwrap_or_default(),
        content_type,
        topic_id: topic_id(message),
        reply: reply_ref(message),
        date: message.get("date").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// 发送者 user id：只认 `messageSenderUser`（匿名管理员 / 频道身份没有 user id）。
fn sender_user_id(message: &Value) -> Option<i64> {
    let sender = message.get("sender_id")?;
    if sender.get("@type").and_then(Value::as_str) != Some("messageSenderUser") {
        return None;
    }
    sender.get("user_id").and_then(Value::as_i64)
}

/// 以群 / 频道身份发送时的 chat id（匿名管理员）；普通用户消息返回 `None`。
fn sender_chat_id(message: &Value) -> Option<i64> {
    let sender = message.get("sender_id")?;
    if sender.get("@type").and_then(Value::as_str) != Some("messageSenderChat") {
        return None;
    }
    sender.get("chat_id").and_then(Value::as_i64)
}

/// 话题：只认论坛话题（`messageTopicForum`）；General 话题没有 topic_id。
///
/// forum_topic_id 是 `int32`，超出范围就当没有（外部输入，不能 panic 也不能截断）。
fn topic_id(message: &Value) -> Option<i32> {
    let topic = message.get("topic_id")?;
    if topic.get("@type").and_then(Value::as_str) != Some("messageTopicForum") {
        return None;
    }
    let raw = topic.get("forum_topic_id").and_then(Value::as_i64)?;
    i32::try_from(raw).ok()
}

/// 引用关系：只认「回复某条消息」；故事 / 外部回复不参与本插件。
///
/// 更新里能拿到的只有 id（+ 跨群引用时的 chat_id）和用户手动选中的片段；
/// 被引用消息的正文由 `td::message` 另外取（`getRepliedMessage` / `getMessage`）。
fn reply_ref(message: &Value) -> Option<ReplyRef> {
    if let Some(reply) = message.get("reply_to") {
        match reply.get("@type").and_then(Value::as_str) {
            // 收到的更新里是 `messageReplyToMessage`，输入侧才是 `inputMessageReplyToMessage`；
            // 两种写法都认（回放归档 / 夹具可能直接复用请求形状）。
            Some("messageReplyToMessage") | Some("inputMessageReplyToMessage") => {
                // message_id = 0 表示「不知道回复的是哪条」（TDLib 文档允许），当没有引用处理。
                let message_id = reply
                    .get("message_id")
                    .and_then(Value::as_i64)
                    .filter(|id| *id > 0)?;
                let quote = reply.get("quote");
                return Some(ReplyRef {
                    message_id,
                    // chat_id = 0 表示被引用消息所在 chat 未知：调用方退回当前 chat。
                    chat_id: reply
                        .get("chat_id")
                        .and_then(Value::as_i64)
                        .filter(|id| *id != 0),
                    quote: quote.and_then(quote_text),
                    is_manual: quote
                        .and_then(|quote| quote.get("is_manual"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
            // 故事回复不参与本插件（没有可读正文，也不该进模型上下文）。
            Some("messageReplyToStory") => return None,
            // 未知形状：可能是老版本 / 新版本新增的引用类型，继续看扁平字段。
            _ => {}
        }
    }
    // 老 TDLib：消息上直接带一个扁平的 `reply_to_message_id`。
    let message_id = message
        .get("reply_to_message_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)?;
    Some(ReplyRef {
        message_id,
        chat_id: None,
        quote: None,
        is_manual: false,
    })
}

/// `TextQuote` 的文本。
///
/// 1.8.70 的 `textQuote` 同时有 `text: formattedText` 和 `rich_texts: vector<RichText>`：
/// 手动选中**带格式**的片段时 TDLib 只填 `rich_texts`，`text` 是空串——只看 `text`
/// 会把用户选的片段整段丢掉（真机上就是这样：引用块只剩一个 id）。
fn quote_text(quote: &Value) -> Option<String> {
    let plain = quote.get("text").and_then(|text| {
        text.as_str()
            .map(str::to_owned)
            .or_else(|| text.get("text").and_then(Value::as_str).map(str::to_owned))
    });
    if let Some(plain) = plain.filter(|text| !text.trim().is_empty()) {
        return Some(plain);
    }
    // 富文本形态：把 RichText 树拍平（粗体 / 斜体这些壳子直接穿透）。
    let text: String = quote
        .get("rich_texts")
        .and_then(Value::as_array)?
        .iter()
        .map(super::message::rich_text)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// 正文：文本消息取 `text`，媒体取 `caption`，其余为空。
///
/// 话题列表也要从 `Message.content` 里取一行摘要，所以这里公开给同层复用。
pub(super) fn content_text(content: &Value) -> String {
    let field = match content.get("@type").and_then(Value::as_str) {
        Some("messageText") => "text",
        _ => "caption",
    };
    content
        .get(field)
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
