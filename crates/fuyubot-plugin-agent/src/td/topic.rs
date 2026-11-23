//! 话题：建 / 列 / 删 / 改名。
//!
//! TDLib 没有对应的 simplify op，只能走 `invoke` 逃生舱（`args.request` 是完整请求 JSON）。
//! 应答同样是**外部输入**，缺字段就报错或跳过，绝不 panic。
//!
//! 回放时这些请求会留档，方法返回可用的占位（见各方法注释），让 worker 链路继续。

use serde_json::{Value, json};

use crate::core::{AgentError, AgentResult};

use super::{TdClient, TopicInfo, update};

/// 建话题的 TDLib 请求体。
///
/// `is_name_implicit: false` 表示名字由调用方给（true 才是从首条消息里取）。
/// `forumTopicIcon` 的 `custom_emoji_id: 0` = 不用自定义 emoji。
pub(super) fn create_request(chat_id: i64, name: &str, icon_color: i64) -> Value {
    json!({
        "@type": "createForumTopic",
        "chat_id": chat_id,
        "name": name,
        "is_name_implicit": false,
        "icon": {
            "@type": "forumTopicIcon",
            "color": icon_color,
            "custom_emoji_id": 0,
        },
    })
}

/// 列话题的 TDLib 请求体：第一页，按创建时间倒序。
pub(super) fn list_request(chat_id: i64, limit: i32) -> Value {
    json!({
        "@type": "getForumTopics",
        "chat_id": chat_id,
        "query": "",
        "offset_date": 0,
        "offset_message_id": 0,
        "offset_forum_topic_id": 0,
        "limit": limit,
    })
}

/// 删话题的 TDLib 请求体。
pub(super) fn delete_request(chat_id: i64, topic_id: i32) -> Value {
    json!({
        "@type": "deleteForumTopic",
        "chat_id": chat_id,
        "forum_topic_id": topic_id,
    })
}

/// 改话题名的 TDLib 请求体。
pub(super) fn edit_request(chat_id: i64, topic_id: i32, name: &str) -> Value {
    json!({
        "@type": "editForumTopic",
        "chat_id": chat_id,
        "forum_topic_id": topic_id,
        "name": name,
    })
}

/// 建话题。
pub(super) fn create_topic(
    client: &TdClient,
    chat_id: i64,
    name: &str,
    icon_color: i64,
    timeout_ms: u64,
) -> AgentResult<TopicInfo> {
    let request = create_request(chat_id, name, icon_color);
    match client.run_invoke(request, timeout_ms)? {
        Some(info) => parse_created(&info, name),
        // 回放占位：请求已留档，给一个固定话题号让上层把消息发进这个话题继续跑。
        None => Ok(TopicInfo {
            topic_id: 1,
            name: name.to_owned(),
            last_message_id: None,
            last_message_text: None,
            unread_count: 0,
            is_pinned: false,
        }),
    }
}

/// 列话题；回放时返回空列表（没有 TDLib 就没有话题）。
pub(super) fn list_topics(
    client: &TdClient,
    chat_id: i64,
    limit: i32,
    timeout_ms: u64,
) -> AgentResult<Vec<TopicInfo>> {
    let request = list_request(chat_id, limit);
    match client.run_invoke(request, timeout_ms)? {
        Some(value) => Ok(parse_topics(&value)),
        None => Ok(Vec::new()),
    }
}

/// 删话题。
pub(super) fn delete_topic(
    client: &TdClient,
    chat_id: i64,
    topic_id: i32,
    timeout_ms: u64,
) -> AgentResult<()> {
    client.run_invoke(delete_request(chat_id, topic_id), timeout_ms)?;
    Ok(())
}

/// 改话题名。
pub(super) fn edit_topic(
    client: &TdClient,
    chat_id: i64,
    topic_id: i32,
    name: &str,
    timeout_ms: u64,
) -> AgentResult<()> {
    client.run_invoke(edit_request(chat_id, topic_id, name), timeout_ms)?;
    Ok(())
}

/// `createForumTopic` 的应答是 `forumTopicInfo`。
fn parse_created(info: &Value, fallback_name: &str) -> AgentResult<TopicInfo> {
    let topic_id = topic_id_of(info)?;
    let name = info
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback_name)
        .to_owned();
    Ok(TopicInfo {
        topic_id,
        name,
        last_message_id: None,
        last_message_text: None,
        unread_count: 0,
        is_pinned: false,
    })
}

/// 从 `forumTopicInfo` 取 `forum_topic_id`：缺字段 / 类型不对都报错，不 panic。
fn topic_id_of(info: &Value) -> AgentResult<i32> {
    let raw = info
        .get("forum_topic_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| AgentError::Td("话题应答缺少 forum_topic_id".to_owned()))?;
    i32::try_from(raw).map_err(|_| AgentError::Td(format!("话题号超出范围：{raw}")))
}

/// `getForumTopics` 的应答：`forumTopics.topics[]`，每项是 `forumTopic`。
fn parse_topics(value: &Value) -> Vec<TopicInfo> {
    value
        .get("topics")
        .and_then(Value::as_array)
        .map(|topics| topics.iter().filter_map(parse_topic).collect())
        .unwrap_or_default()
}

/// 一项 `forumTopic`：`info` 拿 id / 名字，`last_message` 拿摘要，另有未读数与置顶。
fn parse_topic(topic: &Value) -> Option<TopicInfo> {
    let info = topic.get("info")?;
    let topic_id = topic_id_of(info).ok()?;
    let last = topic.get("last_message");
    let last_message_id = last
        .and_then(|message| message.get("id"))
        .and_then(Value::as_i64);
    let last_message_text = last
        .and_then(|message| message.get("content"))
        .map(update::content_text)
        .filter(|text| !text.is_empty());
    Some(TopicInfo {
        topic_id,
        name: info
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        last_message_id,
        last_message_text,
        unread_count: topic
            .get("unread_count")
            .and_then(Value::as_i64)
            .and_then(|count| i32::try_from(count).ok())
            .unwrap_or(0),
        is_pinned: topic
            .get("is_pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}
