//! `updateNewMessage` 的纯解析：没有宿主、没有状态，单测直接喂 JSON。
//!
//! 为什么单列一层：更新是**外部输入**，字段缺失 / 类型不对都不能 panic；
//! 把它折成 [`IncomingMessage`](crate::core::IncomingMessage) 之后，准入、防环、
//! 落盘都只看这个结构，不再碰原始 JSON。

use serde_json::Value;

use crate::core::{ChatKind, IncomingMessage};

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
        reply_to_message_id: reply_to_id(message),
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
fn reply_to_id(message: &Value) -> Option<i64> {
    let reply = message.get("reply_to")?;
    // 收到的更新里是 `messageReplyToMessage`，输入侧才是 `inputMessageReplyToMessage`；
    // 两种写法都认（回放归档 / 夹具可能直接复用请求形状）。
    let kind = reply.get("@type").and_then(Value::as_str)?;
    if !matches!(kind, "messageReplyToMessage" | "inputMessageReplyToMessage") {
        return None;
    }
    reply.get("message_id").and_then(Value::as_i64)
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
