//! TDLib 交互层：插件里**唯一**碰 TDLib 的地方。
//!
//! 三个原则：
//! 1. 只经宿主 ABI（`HostExt::simplify_invoke_json` / `invoke_value` / `wait_for_update_json`），
//!    不链接 tdjson；
//! 2. 每个方法都是**同步阻塞**的（等 response 通道），只能在插件自己的工作线程 /
//!    工具处理器里调，绝不能在宿主回调里调；
//! 3. 回放（`--replay`）下宿主不接 TDLib，请求只留档、返回**合成请求号**（>= `1 << 48`）。
//!    这种情况下不要等响应，直接返回 `offline = true` 的结果，否则回放会卡满超时。
//!
//! 拆成五个子模块，各管一段：`update`（纯解析）/ `send` / `draft`（流式草稿）/ `topic` / `identity`。
//! 本文件只留共享类型、门面 [`TdClient`]，以及「发 op → 判离线 → 等应答 → 拆信封」
//! 这条所有方法共用的管线——离线短路只写一次，子模块想忘也忘不了。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

use crate::core::{AgentError, AgentResult, ChatKey, IncomingMessage, QuotedMessage};

mod draft;
mod identity;
mod message;
mod send;
mod topic;
mod update;

pub use draft::DraftSend;

/// 宿主的请求号分三档（见宿主 `fuyubot-loader` 的 runtime.rs）：
///
/// - **TDLib 自己的号**：小号（TDLib 的 `@extra`），在线，响应照常回投；
/// - **合成号**：`[1 << 48, 1 << 49)`——没接 TDLib（`--no-tdlib` / `--replay`）时用，
///   请求只留档，**永远不会有响应**，等下去只是白等到超时；
/// - **simplify 操作号**：`>= 1 << 49`——`send_message` / `edit_message_text` /
///   `delete_messages` / `invoke` 这些 op 的回投地址，**在线**，结果走 `response` 通道。
///
/// 之前只判 `>= 1 << 48`，把 simplify 操作号也当成了「离线」：于是每个 op 发出去之后
/// 立刻返回 `Ok(None)`，TDLib 的应答全被丢掉——消息照发，但拿不到消息号、查不到被引用
/// 消息（回复引用就是这个症状）、建话题只拿到占位号、也发现不了发送失败。
const SYNTHETIC_REQUEST_BASE: u64 = 1 << 48;
const SIMPLIFY_REQUEST_BASE: u64 = 1 << 49;

/// 等响应时在操作自己的 `timeout_ms` 之上多给的余量：宿主要先等 TDLib，再回投响应。
const WAIT_MARGIN_MS: u64 = 5_000;

/// 身份 / 会话查询的预算：这类请求打的是 TDLib 本地缓存，不该等太久。
const LOOKUP_TIMEOUT_MS: u64 = 10_000;

/// 一条已发出的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMessage {
    pub chat_id: i64,
    pub message_id: i64,
    /// true = 离线回放，请求只留档没有结果。
    pub offline: bool,
}

/// 一个话题（论坛群或带话题的 bot 私聊）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicInfo {
    pub topic_id: i32,
    pub name: String,
    pub last_message_id: Option<i64>,
    pub last_message_text: Option<String>,
    pub unread_count: i32,
    pub is_pinned: bool,
}

/// 用户 / bot 的基础信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub user_id: i64,
    pub first_name: String,
    pub last_name: String,
    pub username: Option<String>,
    pub is_bot: bool,
    /// bot 是否启用了话题（BotFather 的 threads 开关）。
    pub has_topics: bool,
    /// bot 是否允许用户建 / 删话题。
    pub allows_users_to_create_topics: bool,
}

impl UserInfo {
    /// 展示名：昵称优先，其次 @username，最后 id。
    pub fn display(&self) -> String {
        let name = format!("{} {}", self.first_name.trim(), self.last_name.trim())
            .trim()
            .to_owned();
        if !name.is_empty() {
            return name;
        }
        match self.username.as_deref() {
            Some(username) if !username.is_empty() => format!("@{username}"),
            _ => self.user_id.to_string(),
        }
    }
}

/// TDLib 门面。
#[derive(Clone)]
pub struct TdClient {
    host: HostApi,
    /// 发送者是不是 bot 的进程内缓存：worker 每条消息都要问一次，TDLib 那边太贵。
    ///
    /// 只缓存**查到**的结果；回放 / 未登录时查不到，下一次还得再问。
    bot_cache: Arc<Mutex<HashMap<i64, bool>>>,
}

impl TdClient {
    /// 包一层宿主。
    pub fn new(host: HostApi) -> Self {
        Self {
            host,
            bot_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 宿主能力（同模块子模块用）。
    pub fn host(&self) -> HostApi {
        self.host
    }

    /// 这个请求号是不是「离线合成号」（没接 TDLib，不会有响应）。
    ///
    /// simplify 的操作号比合成号更高一档，是在线请求，不能当离线短路掉。
    pub fn is_offline_request(request_id: u64) -> bool {
        (SYNTHETIC_REQUEST_BASE..SIMPLIFY_REQUEST_BASE).contains(&request_id)
    }

    /// 发文本（`parse_mode` = None / markdown / html）。
    pub fn send_text(
        &self,
        chat_id: i64,
        topic_id: Option<i32>,
        text: &str,
        parse_mode: Option<&str>,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> AgentResult<SentMessage> {
        send::send_text(
            self, chat_id, topic_id, text, parse_mode, reply_to, timeout_ms,
        )
    }

    /// 发富消息（Markdown → inputMessageRichMessage，失败由调用方降级）。
    pub fn send_rich(
        &self,
        chat_id: i64,
        topic_id: Option<i32>,
        markdown: &str,
        reply_to: Option<i64>,
        timeout_ms: u64,
    ) -> AgentResult<SentMessage> {
        send::send_rich(self, chat_id, topic_id, markdown, reply_to, timeout_ms)
    }

    /// 发一条**文本草稿**（官方流式回复的 TDLib 形态）：`text` 为空 = 客户端自带的
    /// Thinking… 占位；同一个 `draft_id` 反复更新会做增量动画，最后用正式消息落地。
    pub fn send_text_draft(&self, draft: &DraftSend, text: &str) -> AgentResult<()> {
        draft::send_text_draft(self, draft, text)
    }

    /// 发一条**富消息草稿**（Markdown 先本地转块树，再走 `sendRichMessageDraft`）。
    pub fn send_rich_draft(&self, draft: &DraftSend, markdown: &str) -> AgentResult<()> {
        draft::send_rich_draft(self, draft, markdown)
    }

    /// 发一条**思考占位**富草稿（`inputPageBlockThinking` 块）。
    pub fn send_thinking_draft(&self, draft: &DraftSend, text: &str) -> AgentResult<()> {
        draft::send_thinking_draft(self, draft, text)
    }

    /// 编辑自己的消息为文本。
    pub fn edit_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        parse_mode: Option<&str>,
        timeout_ms: u64,
    ) -> AgentResult<Option<SentMessage>> {
        send::edit_text(self, chat_id, message_id, text, parse_mode, timeout_ms)
    }

    /// 编辑自己的消息为富消息。
    pub fn edit_rich(
        &self,
        chat_id: i64,
        message_id: i64,
        markdown: &str,
        timeout_ms: u64,
    ) -> AgentResult<Option<SentMessage>> {
        send::edit_rich(self, chat_id, message_id, markdown, timeout_ms)
    }

    /// 删除消息。
    pub fn delete_messages(
        &self,
        chat_id: i64,
        message_ids: &[i64],
        revoke: bool,
        timeout_ms: u64,
    ) -> AgentResult<()> {
        send::delete_messages(self, chat_id, message_ids, revoke, timeout_ms)
    }

    /// 建话题（论坛超级群或"带话题的 bot 私聊"）。
    pub fn create_topic(
        &self,
        chat_id: i64,
        name: &str,
        icon_color: i64,
        timeout_ms: u64,
    ) -> AgentResult<TopicInfo> {
        topic::create_topic(self, chat_id, name, icon_color, timeout_ms)
    }

    /// 列话题（旧对话列表）。
    pub fn list_topics(
        &self,
        chat_id: i64,
        limit: i32,
        timeout_ms: u64,
    ) -> AgentResult<Vec<TopicInfo>> {
        topic::list_topics(self, chat_id, limit, timeout_ms)
    }

    /// 删话题（连同其中所有消息）。
    pub fn delete_topic(&self, chat_id: i64, topic_id: i32, timeout_ms: u64) -> AgentResult<()> {
        topic::delete_topic(self, chat_id, topic_id, timeout_ms)
    }

    /// 改话题名。
    pub fn edit_topic(
        &self,
        chat_id: i64,
        topic_id: i32,
        name: &str,
        timeout_ms: u64,
    ) -> AgentResult<()> {
        topic::edit_topic(self, chat_id, topic_id, name, timeout_ms)
    }

    /// 取自己（getMe）。
    pub fn me(&self) -> AgentResult<UserInfo> {
        identity::me(self)
    }

    /// 取用户（先问宿主缓存服务，再退回 getUser）。
    pub fn user(&self, user_id: i64) -> AgentResult<Option<UserInfo>> {
        identity::user(self, user_id)
    }

    /// 按 @username 找 bot / 用户（searchPublicChat）。
    pub fn resolve_username(&self, username: &str) -> AgentResult<Option<i64>> {
        identity::resolve_username(self, username)
    }

    /// 发送者是不是 bot。
    pub fn is_bot(&self, user_id: i64) -> AgentResult<bool> {
        identity::is_bot(self, user_id)
    }

    /// 解析 `updateNewMessage`（纯函数，可单测）。
    pub fn parse_incoming(update: &Value) -> Option<IncomingMessage> {
        update::parse_incoming(update)
    }

    /// 取被引用的消息（用户点了「回复」时才有）：正文 + 作者 + 内容类型。
    ///
    /// 只在 worker 线程调用（同步等应答）；取不到返回 `Ok(None)`，由上层只留引用标记。
    pub fn quoted_message(&self, message: &IncomingMessage) -> AgentResult<Option<QuotedMessage>> {
        message::replied(self, message)
    }

    /// 这个会话当前是否已有话题（bot 私聊 topics）。
    pub fn has_topics(&self, chat_id: i64) -> AgentResult<bool> {
        identity::has_topics(self, chat_id)
    }

    /// 给聊天 / 话题取一个稳定 key（API 与 worker 共用）。
    pub fn key_of(chat_id: i64, topic_id: Option<i32>) -> ChatKey {
        ChatKey::new(chat_id, topic_id)
    }

    /// 发送者是否 bot 的进程内缓存（identity 层读写）。
    pub(super) fn bot_cache(&self) -> &Mutex<HashMap<i64, bool>> {
        &self.bot_cache
    }

    /// 跑一个**异步** simplify op，并等它的 `response`。
    ///
    /// - `Ok(Some(result))`：在线，拿到 op 的结果；
    /// - `Ok(None)`：**回放**（合成请求号），请求已留档但没有结果，调用方给离线占位；
    /// - `Err`：op 被拒 / 等待超时 / 宿主调用失败。
    pub(super) fn run_op(
        &self,
        op: &str,
        args: Value,
        timeout_ms: u64,
    ) -> AgentResult<Option<Value>> {
        let request_id = self
            .host
            .simplify_invoke_json(json!({ "op": op, "args": args }))
            .map_err(|error| AgentError::Host(format!("simplify {op} 调用失败：{error}")))?;
        if !request_id.is_valid() {
            return Err(AgentError::Host(format!("simplify {op} 没有拿到请求号")));
        }
        // 回放：请求只留档，等下去必然是超时。
        if Self::is_offline_request(request_id.id) {
            return Ok(None);
        }
        self.wait_result(op, request_id.id, timeout_ms).map(Some)
    }

    /// 走 `invoke` 逃生舱发一条**完整** TDLib 请求（话题相关的方法没有专用 op）。
    pub(super) fn run_invoke(&self, request: Value, timeout_ms: u64) -> AgentResult<Option<Value>> {
        let args = json!({ "request": request, "timeout_ms": timeout_ms });
        self.run_op("invoke", args, timeout_ms)
    }

    /// 跑一个**同步** simplify op（不碰网络，回调里也安全）：`rich_from_markdown` 等。
    pub(super) fn run_sync_op(&self, op: &str, args: Value) -> AgentResult<Value> {
        let value = self
            .host
            .simplify_invoke_result_json(json!({ "op": op, "args": args }))
            .map_err(|error| AgentError::Host(format!("simplify_result {op} 调用失败：{error}")))?;
        if value.is_null() {
            return Err(AgentError::Td(format!("{op} 没有返回结果")));
        }
        simplify_result(&value)
    }

    /// 等某个请求号的 `response`，再拆信封。
    fn wait_result(&self, op: &str, request_id: u64, timeout_ms: u64) -> AgentResult<Value> {
        let filter = json!({ "event_type": "response", "request_id": request_id });
        let response = self
            .host
            .wait_for_update_json(filter, timeout_ms.saturating_add(WAIT_MARGIN_MS))
            .ok_or_else(|| AgentError::Td(format!("{op} 等待响应超时（{timeout_ms} ms）")))?;
        simplify_result(&response)
    }
}

/// 拆 simplify 应答信封：`{"@type":"simplifyResult","ok":…,"result"/"error":…}`。
///
/// 有的宿主会把信封再包一层（`{"extra": …}`），两处都认；不带 `ok` 的值当成
/// 「结果本身」原样返回（兼容只回结果的宿主版本，不会把正常结果误判成错误）。
fn simplify_result(value: &Value) -> AgentResult<Value> {
    let envelope = match value.get("ok") {
        Some(_) => value,
        None => value
            .get("extra")
            .filter(|extra| extra.get("ok").is_some())
            .unwrap_or(value),
    };
    match envelope.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(envelope.get("result").cloned().unwrap_or(Value::Null)),
        Some(false) => {
            let error = envelope.get("error").cloned().unwrap_or(Value::Null);
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("未提供错误信息");
            Err(AgentError::Td(format!("{code}：{message}")))
        }
        None => Ok(value.clone()),
    }
}
