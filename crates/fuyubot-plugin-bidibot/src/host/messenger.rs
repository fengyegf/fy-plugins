//! 宿主消息网关：发送文本、转发消息、建话题、取用户基础信息。
//!
//! 两条路都走宿主已经收口的入口，本插件**不自己拼 TDLib 请求**：
//!
//! - 发送：simplify 的 `send_message` op（simplify_invoke + response 通道等结果）。
//!   话题里发送时额外带 `topic_id`（messageTopicForum）。
//! - 转发：simplify 的 `forward_messages` op；用户 → 群方向保留转发头部，
//!   群 → 用户方向用 `send_copy` 复制内容、不带"转发自"。
//! - 建话题：TDLib 的 createForumTopic 没有专用 op，走 `invoke` 逃生舱。
//! - 取用户信息：simplify 的 `invoke` 逃生舱发 getUser；宿主有缓存服务时先问缓存。
//!
//! 这些调用都是**阻塞**的：只能在插件自己的业务线程（bidibot-task）里调。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt, LogLevel};
use serde_json::{Value, json};

/// 普通文本发送超时。
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
/// 取用户信息的超时：TDLib 本地缓存应答，给短一点。
const USER_TIMEOUT_MS: u64 = 5_000;

/// 宿主缓存服务（ABI 文档里的宿主预置服务）。
const CACHE_SERVICE: &str = "fuyubot.host.cache";

/// 一次发送得到的消息定位信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub chat_id: i64,
    pub message_id: i64,
}

/// 用户的基础信息（名片头用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserInfo {
    pub user_id: i64,
    /// 昵称：first_name + last_name。
    pub name: String,
    /// 主用户名（不带 @）。
    pub username: Option<String>,
    /// 是否是 bot。
    pub is_bot: bool,
    /// 头像（profile_photo.small）的 TDLib file id；没有头像就是 None。
    pub photo_file_id: Option<i64>,
}

impl UserInfo {
    /// 名片头里的展示名：昵称优先，其次用户名，最后退回 id。
    pub fn display(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_owned();
        }
        match self.username.as_deref() {
            Some(username) if !username.is_empty() => format!("@{username}"),
            _ => self.user_id.to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("调用宿主失败: {0}")]
    Host(String),
    #[error("等待 TDLib 响应超时")]
    Timeout,
    #[error("操作被拒绝（{code}）: {message}")]
    Op { code: String, message: String },
    #[error("TDLib 应答缺少字段: {0}")]
    Missing(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 解析 getUser 的应答。抽成纯函数：单测不必造 HostApi。
/// 从 TDLib file 对象里取本地路径（没下载完就是空串）。
fn local_path(file: &Value) -> Option<std::path::PathBuf> {
    file.get("local")
        .and_then(|local| local.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
}

pub fn parse_user(value: &Value) -> Option<UserInfo> {
    let user_id = value.get("id").and_then(Value::as_i64)?;
    let first = value
        .get("first_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let last = value
        .get("last_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = format!("{first}{last}").trim().to_owned();
    let username = value
        .get("usernames")
        .and_then(|usernames| usernames.get("active_usernames"))
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_str)
        .map(str::to_owned);
    let is_bot = value
        .get("type")
        .and_then(|kind| kind.get("@type"))
        .and_then(Value::as_str)
        == Some("userTypeBot");
    // 头像优先小图；小图缺失时退大图。有些来源（宿主缓存）根本不含 profile_photo。
    let photo_file_id = value
        .get("profile_photo")
        .and_then(|photo| {
            photo
                .get("small")
                .and_then(|small| small.get("id"))
                .or_else(|| photo.get("big").and_then(|big| big.get("id")))
        })
        .and_then(Value::as_i64);
    Some(UserInfo {
        user_id,
        name,
        username,
        is_bot,
        photo_file_id,
    })
}

/// 拼一条纯文本的 input_message_content。
///
/// TDLib 的 inputMessageText.text 是 formattedText 对象，不是字符串——塞字符串会被拒。
pub fn text_content(text: &str) -> Value {
    json!({
        "@type": "inputMessageText",
        "text": { "@type": "formattedText", "text": text, "entities": [] },
    })
}

/// 引用消息的两种写法。
///
/// 真 TDLib 的 sendMessage 参数是 `reply_to: InputMessageReplyTo`（对象），
/// 而 fuyubot-simplify 的 SendMessageParams 里是扁平的 `reply_to_message_id`。
/// 宿主把 args 透传给 TDLib，所以默认发原生对象形状；被拒（bad_args / 400）时
/// 再换扁平键重发一次，兼容按 SendMessageParams 解析的宿主版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKey {
    /// {"reply_to_message_id": 123}
    MessageId,
    /// {"reply_to": {"@type":"inputMessageReplyToMessage","message_id":123}}
    ReplyObject,
}

/// 一张本地图片的 input_message_content（照 td_api.tl 的 inputMessagePhoto）。
///
/// 宽高交给服务器替换（0 = 让 TDLib 自己读）；caption 为空就不带。
pub fn photo_content(path: &std::path::Path, caption: Option<&str>) -> Value {
    let mut content = json!({
        "@type": "inputMessagePhoto",
        "photo": {
            "@type": "inputPhoto",
            "photo": { "@type": "inputFileLocal", "path": path.to_string_lossy() },
            "width": 0,
            "height": 0,
        },
    });
    if let Some(caption) = caption.filter(|text| !text.trim().is_empty()) {
        content["caption"] = json!({
            "@type": "formattedText",
            "text": caption,
            "entities": [],
        });
    }
    content
}

/// 拼 send_message op 的 args；reply_to 为空时不带引用键。
///
/// 旧签名只发普通聊天，保留给 relay 与现有单测；带话题的发送用
/// [`send_args_with_topic`]。
pub fn send_args_with(
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
    key: ReplyKey,
    timeout_ms: u64,
) -> Value {
    send_args_with_topic(chat_id, text, None, reply_to, key, timeout_ms)
}

/// 拼 send_message op 的 args，可带话题（论坛群）。
///
/// `topic` 为空时不带 topic_id；reply_to 为空时不带引用键。
pub fn send_args_with_topic(
    chat_id: i64,
    text: &str,
    topic: Option<i64>,
    reply_to: Option<i64>,
    key: ReplyKey,
    timeout_ms: u64,
) -> Value {
    let mut args = json!({
        "chat_id": chat_id,
        "input_message_content": text_content(text),
        "timeout_ms": timeout_ms,
    });
    if let Some(topic_id) = topic {
        args["topic_id"] = topic_arg(topic_id);
    }
    if let Some(message_id) = reply_to {
        match key {
            ReplyKey::MessageId => args["reply_to_message_id"] = json!(message_id),
            ReplyKey::ReplyObject => {
                args["reply_to"] = json!({
                    "@type": "inputMessageReplyToMessage",
                    "message_id": message_id,
                });
            }
        }
    }
    args
}

/// 缺省（SendMessageParams 形状）的 args，单测与普通发送用。
pub fn send_args(chat_id: i64, text: &str, reply_to: Option<i64>, timeout_ms: u64) -> Value {
    send_args_with(chat_id, text, reply_to, ReplyKey::MessageId, timeout_ms)
}

/// TDLib 的话题定位对象（MessageTopic）。
fn topic_arg(topic_id: i64) -> Value {
    json!({
        "@type": "messageTopicForum",
        "forum_topic_id": topic_id,
    })
}

/// 拼 createForumTopic 的 invoke 请求体。
///
/// TDLib 没有单独的 simplify op，只能走 `invoke` 逃生舱。
pub fn create_topic_request(chat_id: i64, name: &str, icon_color: i64) -> Value {
    json!({
        "request": {
            "@type": "createForumTopic",
            "chat_id": chat_id,
            "name": name,
            "is_name_implicit": false,
            "icon": {
                "@type": "forumTopicIcon",
                "color": icon_color,
                "custom_emoji_id": 0,
            },
        },
    })
}

/// 从 createForumTopic 的应答（forumTopicInfo）里取 forum_topic_id。
///
/// 缺字段或类型不对都返回错误，绝不 panic：应答是外部输入。
pub fn parse_topic_id(value: &Value) -> Result<i64> {
    value
        .get("forum_topic_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Missing("createForumTopic 应答缺少 forum_topic_id".to_owned()))
}

/// 拼 forward_messages 的 args；message_ids 为空时返回 None（不必发请求）。
pub fn forward_args(
    to_chat: i64,
    from_chat: i64,
    message_ids: &[i64],
    topic: Option<i64>,
    send_copy: bool,
) -> Option<Value> {
    if message_ids.is_empty() {
        return None;
    }
    let mut args = json!({
        "chat_id": to_chat,
        "from_chat_id": from_chat,
        "message_ids": message_ids,
        "send_copy": send_copy,
    });
    if let Some(topic_id) = topic {
        args["topic_id"] = topic_arg(topic_id);
    }
    Some(args)
}

/// 这个错误是不是"认得这个 op、但不认这个参数"——只有这种才值得换写法重试。
fn is_argument_error(error: &Error) -> bool {
    match error {
        Error::Op { code, .. } => {
            matches!(code.as_str(), "bad_args" | "missing_field" | "tdlib") || code == "400"
        }
        _ => false,
    }
}

/// 消息网关。HostApi 是 Copy，这里只是给它加几个名字。
#[derive(Debug, Clone)]
pub struct Messenger {
    host: HostApi,
}

impl Messenger {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 发一条纯文本；reply_to 是"引用"的消息 id。
    ///
    /// 引用键先按 TDLib 原生 reply_to 对象发；被拒时换 SendMessageParams 的扁平键再发一次
    /// （第一次失败 = 消息没发出去，重发不会造成重复）。
    pub fn send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<Sent> {
        self.send_text_timeout(chat_id, text, reply_to, SEND_TIMEOUT)
    }

    /// 同 [`Messenger::send_text`]，但等多久由调用方决定（relay 传 [`Config::send_timeout`]）。
    pub fn send_text_timeout(
        &self,
        chat_id: i64,
        text: &str,
        reply_to: Option<i64>,
        timeout: Duration,
    ) -> Result<Sent> {
        self.send_text_with_topic(chat_id, text, None, reply_to, timeout)
    }

    /// 在（可选的）话题里发一条纯文本。
    ///
    /// 引用键回退逻辑与 [`Messenger::send_text_timeout`] 完全共用，不另写一套。
    pub fn send_text_in_topic(
        &self,
        chat_id: i64,
        text: &str,
        topic: Option<i64>,
        reply_to: Option<i64>,
        timeout: Duration,
    ) -> Result<Sent> {
        self.send_text_with_topic(chat_id, text, topic, reply_to, timeout)
    }

    /// 发送文本的公共实现：先发 TDLib 原生引用形状，被拒再换扁平键。
    fn send_text_with_topic(
        &self,
        chat_id: i64,
        text: &str,
        topic: Option<i64>,
        reply_to: Option<i64>,
        timeout: Duration,
    ) -> Result<Sent> {
        let timeout_ms = timeout.as_millis() as u64;
        // 先发 TDLib 原生形状（reply_to 对象）：宿主把 args 透传给 sendMessage，
        // 真库只认这个键名（reply_to_message_id 会被 schema 校验警告）。
        match self.send_with(
            chat_id,
            text,
            topic,
            reply_to,
            ReplyKey::ReplyObject,
            timeout_ms,
        ) {
            // 万一遇到按 SendMessageParams 解析 args 的宿主/版本，换回扁平键再发一次。
            Err(error) if reply_to.is_some() && is_argument_error(&error) => self.send_with(
                chat_id,
                text,
                topic,
                reply_to,
                ReplyKey::MessageId,
                timeout_ms,
            ),
            other => other,
        }
    }

    fn send_with(
        &self,
        chat_id: i64,
        text: &str,
        topic: Option<i64>,
        reply_to: Option<i64>,
        key: ReplyKey,
        timeout_ms: u64,
    ) -> Result<Sent> {
        let args = send_args_with_topic(chat_id, text, topic, reply_to, key, timeout_ms);
        let message = self.op("send_message", args, Duration::from_millis(timeout_ms))?;
        Ok(Sent {
            chat_id: message
                .get("chat_id")
                .and_then(Value::as_i64)
                .unwrap_or(chat_id),
            message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
        })
    }

    /// 发一条纯文本，失败只记日志、返回 None。
    pub fn try_send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<Sent> {
        self.try_send_text_timeout(chat_id, text, reply_to, SEND_TIMEOUT)
    }

    /// 同 [`Messenger::try_send_text`]，超时由调用方给。
    pub fn try_send_text_timeout(
        &self,
        chat_id: i64,
        text: &str,
        reply_to: Option<i64>,
        timeout: Duration,
    ) -> Option<Sent> {
        match self.send_text_timeout(chat_id, text, reply_to, timeout) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host
                    .warn(&format!("bidibot: 发送消息到 {chat_id} 失败: {error}"));
                None
            }
        }
    }

    /// 同 [`Messenger::send_text_in_topic`]，失败只记日志、返回 None。
    pub fn try_send_text_in_topic(
        &self,
        chat_id: i64,
        text: &str,
        topic: Option<i64>,
        reply_to: Option<i64>,
        timeout: Duration,
    ) -> Option<Sent> {
        match self.send_text_in_topic(chat_id, text, topic, reply_to, timeout) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host.warn(&format!(
                    "bidibot: 在话题 {topic:?} 里发消息到 {chat_id} 失败: {error}"
                ));
                None
            }
        }
    }

    /// 建一个论坛话题；返回 TDLib 的 forum_topic_id。
    ///
    /// createForumTopic 没有专门的 simplify op，走 `invoke` 逃生舱。
    /// 应答缺 forum_topic_id 时返回 [`Error::Missing`]，不 panic。
    pub fn create_topic(
        &self,
        chat_id: i64,
        name: &str,
        icon_color: i64,
        timeout: Duration,
    ) -> Result<i64> {
        let args = create_topic_request(chat_id, name, icon_color);
        let value = self.op("invoke", args, timeout)?;
        parse_topic_id(&value)
    }

    /// 把 from_chat 的一批消息转发到 to_chat 的（可选）话题里。
    ///
    /// `send_copy = true` 时不带"转发自"头部（群 → 用户方向的安全要求）；
    /// 用户 → 群方向用 false，保留媒体与来源。message_ids 为空时直接当作完成。
    pub fn forward_to(
        &self,
        to_chat: i64,
        from_chat: i64,
        message_ids: &[i64],
        topic: Option<i64>,
        send_copy: bool,
        timeout: Duration,
    ) -> Result<()> {
        let Some(args) = forward_args(to_chat, from_chat, message_ids, topic, send_copy) else {
            // 没消息可转：不是错误，也不需要打扰宿主。
            return Ok(());
        };
        self.op("forward_messages", args, timeout)?;
        Ok(())
    }

    /// 在一个话题里发一张本地图片（可选 caption）。
    ///
    /// 名片卡片 / 头像都走这条路：photo 用 inputFileLocal，上传交给 TDLib。
    pub fn send_photo_in_topic(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        topic: Option<i64>,
        caption: Option<&str>,
        reply_to: Option<i64>,
        timeout: Duration,
    ) -> Result<Sent> {
        let content = photo_content(path, caption);
        let timeout_ms = timeout.as_millis() as u64;
        let mut args = json!({
            "chat_id": chat_id,
            "input_message_content": content,
            "timeout_ms": timeout_ms,
        });
        if let Some(topic_id) = topic {
            args["topic_id"] = topic_arg(topic_id);
        }
        if let Some(message_id) = reply_to {
            args["reply_to"] = json!({
                "@type": "inputMessageReplyToMessage",
                "message_id": message_id,
            });
        }
        let message = self.op("send_message", args, timeout)?;
        Ok(Sent {
            chat_id: message
                .get("chat_id")
                .and_then(Value::as_i64)
                .unwrap_or(chat_id),
            message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
        })
    }

    /// 下载一个 TDLib 文件，返回它在本机的路径。
    ///
    /// 头像就是这样拿到的：先 download_file 等到 updateFile 报下载完成，再读本地文件。
    pub fn download_file(&self, file_id: i64, timeout: Duration) -> Option<std::path::PathBuf> {
        let args = json!({
            "file_id": file_id,
            "priority": 1,
            "timeout_ms": timeout.as_millis() as u64,
        });
        // 第一选择：simplify 的 download_file op（会等 updateFile 报完成）。
        match self.op("download_file", args, timeout) {
            Ok(file) => {
                if let Some(path) = local_path(&file) {
                    return Some(path);
                }
                // 有的宿主实现只回 file 对象、不等下载：local.path 还是空的。
                self.host.log(
                    LogLevel::Debug,
                    &format!("bidibot: download_file 返回但 local.path 为空: {file}"),
                );
            }
            Err(error) => self.host.log(
                LogLevel::Debug,
                &format!("bidibot: download_file op 失败: {error}"),
            ),
        }
        // 第二选择：直接 invoke TDLib downloadFile，synchronous=true 让它在返回前落盘。
        let args = json!({
            "request": {
                "@type": "downloadFile",
                "file_id": file_id,
                "priority": 1,
                "offset": 0,
                "limit": 0,
                "synchronous": true,
            },
            "timeout_ms": timeout.as_millis() as u64,
        });
        match self.op("invoke", args, timeout) {
            Ok(file) => match local_path(&file) {
                Some(path) => Some(path),
                None => {
                    self.host.log(
                        LogLevel::Debug,
                        &format!("bidibot: downloadFile(synchronous) 仍没有 local.path: {file}"),
                    );
                    None
                }
            },
            Err(error) => {
                self.host.log(
                    LogLevel::Debug,
                    &format!("bidibot: downloadFile(synchronous) 失败: {error}"),
                );
                None
            }
        }
    }

    /// 取用户基础信息：宿主缓存 + getUser 合并。
    ///
    /// 缓存里的用户对象可能被宿主编简过（常常没有 profile_photo），只拿它会丢头像；
    /// 所以缓存缺头像时再问一次 getUser，把两边的字段合并成一份。
    pub fn user_info(&self, user_id: i64) -> Option<UserInfo> {
        let cached = self
            .cached_user(user_id)
            .and_then(|value| parse_user(&value));
        if let Some(info) = &cached
            && info.photo_file_id.is_some()
        {
            return Some(info.clone());
        }
        if let Some(info) = &cached {
            self.host.log(
                LogLevel::Debug,
                &format!(
                    "bidibot: 缓存里的用户 {} 没有头像（photo_file_id=None），再问一次 getUser",
                    info.user_id
                ),
            );
        }
        let full = self.get_user(user_id);
        match (cached, full) {
            (Some(cached), Some(full)) => Some(UserInfo {
                // 昵称 / 用户名优先用 getUser 的（更新），头像两边取有的那个。
                name: if full.name.trim().is_empty() {
                    cached.name
                } else {
                    full.name
                },
                username: full.username.or(cached.username),
                photo_file_id: full.photo_file_id.or(cached.photo_file_id),
                ..full
            }),
            (Some(cached), None) => Some(cached),
            (None, full) => full,
        }
    }

    /// 宿主缓存服务里的 user 对象（纯内存，不碰 TDLib）。
    fn cached_user(&self, user_id: i64) -> Option<Value> {
        let input = json!({ "kind": "user", "id": user_id }).to_string();
        let raw = self.host.call_service(CACHE_SERVICE, "get", &input).ok()?;
        let value: Value = serde_json::from_str(&raw).ok()?;
        value.get("value").cloned().filter(|value| !value.is_null())
    }

    /// getUser：走 simplify 的 invoke 逃生舱，同步拿用户对象。
    fn get_user(&self, user_id: i64) -> Option<UserInfo> {
        let args = json!({
            "request": { "@type": "getUser", "user_id": user_id },
            "timeout_ms": USER_TIMEOUT_MS,
        });
        match self.op(
            "invoke",
            args,
            Duration::from_millis(USER_TIMEOUT_MS + 2_000),
        ) {
            Ok(value) => parse_user(&value),
            Err(error) => {
                self.host.log(
                    LogLevel::Debug,
                    &format!("bidibot: 取用户 {user_id} 信息失败: {error}"),
                );
                None
            }
        }
    }

    /// 跑一个 simplify 操作并等它的结果。
    fn op(&self, name: &str, args: Value, timeout: Duration) -> Result<Value> {
        let mut args = args;
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(timeout.as_millis() as u64);
        args["timeout_ms"] = json!(timeout_ms);

        let request_id = self
            .host
            .simplify_invoke_json(json!({ "op": name, "args": args }))
            .map_err(|error| Error::Host(error.to_string()))?;

        // 多给 5 秒余量：宿主要先等 TDLib，再回投响应。
        let filter = json!({ "event_type": "response", "request_id": request_id.id });
        let response = self
            .host
            .wait_for_update_json(filter, timeout_ms + 5_000)
            .ok_or(Error::Timeout)?;

        if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
        let error = response.get("error").cloned().unwrap_or(Value::Null);
        Err(Error::Op {
            code: error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未提供错误信息")
                .to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_args_carries_reply_only_when_present() {
        let own = send_args(42, "hi", None, 1000);
        assert_eq!(own["chat_id"], 42);
        assert!(own.get("reply_to_message_id").is_none(), "{own}");
        assert_eq!(own["input_message_content"]["@type"], "inputMessageText");

        let quoted = send_args(42, "hi", Some(7), 1000);
        assert_eq!(quoted["reply_to_message_id"], 7);
    }

    #[test]
    fn reply_object_shape_is_tdlib_native() {
        let args = send_args_with(42, "hi", Some(7), ReplyKey::ReplyObject, 1000);
        assert_eq!(args["reply_to"]["@type"], "inputMessageReplyToMessage");
        assert_eq!(args["reply_to"]["message_id"], 7);
        assert!(args.get("reply_to_message_id").is_none());
    }

    #[test]
    fn only_argument_errors_trigger_the_retry() {
        assert!(is_argument_error(&Error::Op {
            code: "bad_args".to_owned(),
            message: String::new(),
        }));
        assert!(is_argument_error(&Error::Op {
            code: "400".to_owned(),
            message: String::new(),
        }));
        assert!(!is_argument_error(&Error::Timeout));
        assert!(!is_argument_error(&Error::Op {
            code: "timeout".to_owned(),
            message: String::new(),
        }));
    }

    #[test]
    fn photo_content_carries_caption_only_when_given() {
        let path = std::path::Path::new("C:/tmp/card.jpg");
        let bare = photo_content(path, None);
        assert_eq!(bare["@type"], "inputMessagePhoto");
        assert_eq!(bare["photo"]["photo"]["@type"], "inputFileLocal");
        assert!(bare.get("caption").is_none(), "{bare}");

        let with_caption = photo_content(path, Some("你好"));
        assert_eq!(with_caption["caption"]["@type"], "formattedText");
        assert_eq!(with_caption["caption"]["text"], "你好");
    }

    #[test]
    fn user_parsing_handles_names_and_usernames() {
        let value = json!({
            "@type": "user",
            "id": 123,
            "first_name": "张",
            "last_name": "三",
            "usernames": { "active_usernames": ["zhangsan"] },
            "type": { "@type": "userTypeRegular" },
        });
        let info = parse_user(&value).unwrap();
        assert_eq!(info.user_id, 123);
        assert_eq!(info.name, "张三");
        assert_eq!(info.username.as_deref(), Some("zhangsan"));
        assert!(!info.is_bot);
        assert_eq!(info.display(), "张三");
    }

    #[test]
    fn missing_name_falls_back_to_username_then_id() {
        let mut info = UserInfo {
            user_id: 9,
            name: String::new(),
            username: Some("someone".to_owned()),
            is_bot: false,
            photo_file_id: None,
        };
        assert_eq!(info.display(), "@someone");
        info.username = None;
        assert_eq!(info.display(), "9");
    }

    #[test]
    fn create_topic_request_matches_tdlib_schema() {
        let args = create_topic_request(-1_001_234_567_890, "张三", 7_322_096);
        let request = &args["request"];
        assert_eq!(request["@type"], "createForumTopic");
        assert_eq!(request["chat_id"], json!(-1_001_234_567_890i64));
        assert_eq!(request["name"], "张三");
        assert_eq!(request["is_name_implicit"], json!(false));
        assert_eq!(request["icon"]["@type"], "forumTopicIcon");
        assert_eq!(request["icon"]["color"], json!(7_322_096));
        assert_eq!(request["icon"]["custom_emoji_id"], json!(0));
        // timeout_ms 由 op() 统一注入，不在这里拼。
        assert!(args.get("timeout_ms").is_none(), "{args}");
    }

    #[test]
    fn topic_id_parsing_rejects_missing_or_wrong_field() {
        let ok = json!({
            "@type": "forumTopicInfo",
            "chat_id": -1_001_234_567_890i64,
            "forum_topic_id": 42,
            "name": "张三",
        });
        assert_eq!(parse_topic_id(&ok).unwrap(), 42);

        // 缺字段：返回错误而不是 panic。
        let missing = json!({"@type": "forumTopicInfo", "chat_id": -1_001i64});
        assert!(matches!(parse_topic_id(&missing), Err(Error::Missing(_))));

        // 类型不对（字符串）也算缺字段。
        let wrong_type = json!({"forum_topic_id": "42"});
        assert!(matches!(
            parse_topic_id(&wrong_type),
            Err(Error::Missing(_))
        ));
    }

    #[test]
    fn forward_args_without_topic_has_no_topic_id() {
        let args = forward_args(-1_001i64, 42, &[9], None, false).expect("非空列表应有 args");
        assert_eq!(args["chat_id"], json!(-1_001i64));
        assert_eq!(args["from_chat_id"], json!(42));
        assert_eq!(args["message_ids"], json!([9]));
        assert_eq!(args["send_copy"], json!(false));
        assert!(args.get("topic_id").is_none(), "{args}");
    }

    #[test]
    fn forward_args_with_topic_carries_forum_topic() {
        let args = forward_args(-1_001i64, 42, &[9, 10], Some(7), true).expect("非空列表应有 args");
        assert_eq!(args["topic_id"]["@type"], "messageTopicForum");
        assert_eq!(args["topic_id"]["forum_topic_id"], json!(7));
        assert_eq!(args["message_ids"], json!([9, 10]));
        assert_eq!(args["send_copy"], json!(true));
    }

    #[test]
    fn empty_forward_list_needs_no_request() {
        assert!(forward_args(-1_001i64, 42, &[], Some(7), false).is_none());
        assert!(forward_args(-1_001i64, 42, &[], None, true).is_none());
    }

    #[test]
    fn send_args_with_topic_keeps_topic_and_reply_shapes() {
        let args = send_args_with_topic(
            -1_001i64,
            "hi",
            Some(7),
            Some(9),
            ReplyKey::ReplyObject,
            1000,
        );
        assert_eq!(args["chat_id"], json!(-1_001i64));
        assert_eq!(args["topic_id"]["@type"], "messageTopicForum");
        assert_eq!(args["topic_id"]["forum_topic_id"], json!(7));
        assert_eq!(args["reply_to"]["message_id"], json!(9));
        assert!(args.get("reply_to_message_id").is_none(), "{args}");
    }

    #[test]
    fn send_args_without_topic_stays_backward_compatible() {
        let args = send_args_with_topic(-1_001i64, "hi", None, None, ReplyKey::MessageId, 1000);
        assert!(args.get("topic_id").is_none(), "{args}");
        assert!(args.get("reply_to_message_id").is_none(), "{args}");

        // 旧签名（relay 与旧单测在用）行为不变。
        let legacy = send_args_with(42, "hi", Some(7), ReplyKey::MessageId, 1000);
        assert_eq!(legacy["reply_to_message_id"], json!(7));
        assert!(legacy.get("topic_id").is_none(), "{legacy}");
    }
}
