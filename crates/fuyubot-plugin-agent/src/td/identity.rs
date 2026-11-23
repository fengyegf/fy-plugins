//! 身份与用户：`me` / `user` / `resolve_username` / `is_bot` / `has_topics`。
//!
//! 为什么这么拆：`is_bot` 是 worker 每条消息都要问的（Bot-to-Bot 防环），
//! 所以它有自己的进程内缓存；`user` 先问宿主的对象缓存服务，未命中才发 `getUser`。
//! 回放里没有 TDLib：这些查询一律返回「没查到」而不是错误，链路照样能跑。

use serde_json::{Value, json};

use crate::core::{AgentError, AgentResult};

use super::{LOOKUP_TIMEOUT_MS, TdClient, UserInfo};

/// 宿主预置的对象缓存服务（ABI 文档里的宿主服务，不是插件）。
const CACHE_SERVICE: &str = "fuyubot.host.cache";

/// 取自己：simplify 的 `me` op（宿主内部就是 `getMe`）。
pub(super) fn me(client: &TdClient) -> AgentResult<UserInfo> {
    match client.run_op("me", json!({}), LOOKUP_TIMEOUT_MS)? {
        Some(value) => parse_user(&value)
            .ok_or_else(|| AgentError::Td("getMe 应答里没有用户对象（未登录？）".to_owned())),
        // 回放：没有 TDLib，给一份空身份让上层继续（不是错误）。
        None => Ok(offline_me()),
    }
}

/// 取用户：先问宿主缓存（纯内存），未命中再发 `getUser`。
pub(super) fn user(client: &TdClient, user_id: i64) -> AgentResult<Option<UserInfo>> {
    if user_id <= 0 {
        return Ok(None);
    }
    match cached_user(client, user_id) {
        Cached::Hit(info) => return Ok(Some(info)),
        // 宿主说离线且缓存未命中：再去问 TDLib 只会白等一轮超时。
        Cached::Offline => return Ok(None),
        Cached::Miss => {}
    }
    let request = json!({ "@type": "getUser", "user_id": user_id });
    match client.run_invoke(request, LOOKUP_TIMEOUT_MS)? {
        Some(value) => Ok(parse_user(&value)),
        None => Ok(None),
    }
}

/// 按 @username 找用户：`searchPublicChat`，私聊 chat 的 id 就是 user id。
pub(super) fn resolve_username(client: &TdClient, username: &str) -> AgentResult<Option<i64>> {
    let username = username.trim().trim_start_matches('@');
    if username.is_empty() {
        return Ok(None);
    }
    let request = json!({ "@type": "searchPublicChat", "username": username });
    match client.run_invoke(request, LOOKUP_TIMEOUT_MS)? {
        Some(chat) => Ok(private_chat_user_id(&chat)),
        None => Ok(None),
    }
}

/// 发送者是不是 bot：`user` + 进程内缓存（worker 每条消息都会问）。
pub(super) fn is_bot(client: &TdClient, user_id: i64) -> AgentResult<bool> {
    if let Some(cached) = cached_is_bot(client, user_id) {
        return Ok(cached);
    }
    let info = user(client, user_id)?;
    let is_bot = info.as_ref().is_some_and(|user| user.is_bot);
    // 只缓存**查到**的结果：回放 / 未登录时查不到，下次还要再问。
    if info.is_some() {
        remember_is_bot(client, user_id, is_bot);
    }
    Ok(is_bot)
}

/// 这个会话是否已经有话题：只有 bot 私聊才有这个话题开关。
///
/// 私聊（`chat_id > 0`）就是「与 bot 自己的会话」，开关在 `getMe` 的
/// `userTypeBot.has_topics` 上；群 / 频道没有这个概念，直接 false。
pub(super) fn has_topics(client: &TdClient, chat_id: i64) -> AgentResult<bool> {
    if chat_id <= 0 {
        return Ok(false);
    }
    Ok(me(client)?.has_topics)
}

/// 解析 TDLib 的 `user` 对象；缺 id 返回 `None`（外部输入，绝不 panic）。
pub(super) fn parse_user(value: &Value) -> Option<UserInfo> {
    let user_id = value.get("id").and_then(Value::as_i64)?;
    let user_type = value.get("type");
    let type_name = user_type
        .and_then(|kind| kind.get("@type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let is_bot = type_name == "userTypeBot";
    Some(UserInfo {
        user_id,
        first_name: text_field(value, "first_name"),
        last_name: text_field(value, "last_name"),
        username: username_of(value),
        is_bot,
        // 这两个开关只在 userTypeBot 里存在；不是 bot 就没有话题能力。
        has_topics: is_bot && flag_of(user_type, "has_topics"),
        allows_users_to_create_topics: is_bot
            && flag_of(user_type, "allows_users_to_create_topics"),
    })
}

/// 主用户名：新 TDLib 在 `usernames.active_usernames[0]`，老版本是顶层 `username`。
fn username_of(value: &Value) -> Option<String> {
    let active = value
        .get("usernames")
        .and_then(|usernames| usernames.get("active_usernames"))
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_str);
    let legacy = value.get("username").and_then(Value::as_str);
    active
        .or(legacy)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// 取一个字符串字段；缺失 / 类型不对退化成空串。
fn text_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 取一个布尔字段；缺失 / 类型不对退化成 false。
fn flag_of(value: Option<&Value>, key: &str) -> bool {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 宿主缓存查询的三种结果。
enum Cached {
    /// 命中：直接用，不必碰 TDLib。
    Hit(UserInfo),
    /// 未命中但宿主在线：值得再问一次 `getUser`。
    Miss,
    /// 宿主明确说离线（`online: false`）：未命中就别再等 TDLib 超时。
    Offline,
}

/// 问宿主对象缓存：`{value, online}`。
fn cached_user(client: &TdClient, user_id: i64) -> Cached {
    let input = json!({ "kind": "user", "id": user_id }).to_string();
    let Ok(raw) = client.host().call_service(CACHE_SERVICE, "get", &input) else {
        return Cached::Miss;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
        return Cached::Miss;
    };
    if let Some(info) = payload
        .get("value")
        .filter(|value| !value.is_null())
        .and_then(parse_user)
    {
        return Cached::Hit(info);
    }
    if payload.get("online").and_then(Value::as_bool) == Some(false) {
        Cached::Offline
    } else {
        Cached::Miss
    }
}

/// 私聊 chat 的 id 就是对方 user id；群 / 频道不算「按用户名找到用户」。
fn private_chat_user_id(chat: &Value) -> Option<i64> {
    let kind = chat
        .get("type")
        .and_then(|kind| kind.get("@type"))
        .and_then(Value::as_str);
    if kind != Some("chatTypePrivate") {
        return None;
    }
    chat.get("id").and_then(Value::as_i64)
}

/// 读缓存里的 bot 判定。
fn cached_is_bot(client: &TdClient, user_id: i64) -> Option<bool> {
    let cache = client
        .bot_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    cache.get(&user_id).copied()
}

/// 记下 bot 判定（缓存只进不出；用户改名 / 改类型由 updateUser 驱动的那一侧刷新）。
fn remember_is_bot(client: &TdClient, user_id: i64, is_bot: bool) {
    let mut cache = client
        .bot_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    cache.insert(user_id, is_bot);
}

/// 回放 / 未登录时的占位身份：不是错误，让上层链路继续（bot 相关分支都退化成 false）。
fn offline_me() -> UserInfo {
    UserInfo {
        user_id: 0,
        first_name: String::new(),
        last_name: String::new(),
        username: None,
        is_bot: false,
        has_topics: false,
        allows_users_to_create_topics: false,
    }
}
