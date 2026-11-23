//! `/agent` 命令表：new / list / use / reset / status / help。
//!
//! 为什么命令层这么薄：真正的推理在 [`crate::host::worker`] 的主链路里（消息 → 网关 →
//! 编辑终稿），命令只做"会话管理"——开话题、看列表、清历史、查状态。把它单独一层有三个好处：
//!
//! - 文本解析与文案渲染都是纯函数，单测里不用构造 `HostApi` 就能覆盖；
//! - 阻塞的 TDLib 调用全部收在 `dispatch` 里，而 `dispatch` 只由 worker 线程调用
//!   （绝不在宿主回调里）；
//! - worker 拦下 `/agent` 之后这里再校验一次命令名，避免误伤别的插件的命令。
//!
//! 这里不触发推理：`/agent new` 只建话题，之后用户在话题里正常说话才走主链路。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::json;

use crate::PluginState;
use crate::core::{AgentResult, ChatKey, IncomingMessage, ReplyMode};
use crate::session::SessionSummary;
use crate::td::TopicInfo;

/// 没给标题时用的话题名。
pub const DEFAULT_TOPIC_TITLE: &str = "AI 会话";
/// `/agent list` 顺带问 TDLib 要多少个话题。
const LIST_TOPICS_LIMIT: i32 = 20;

/// `/agent` 的子命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subcommand {
    /// 用法说明；未知子命令也归到这里。
    Help,
    /// `new [标题]`：在私聊里开一个话题。
    New(String),
    /// `list`：列出当前 chat 的会话。
    List,
    /// `reset`：清空当前会话的历史。
    Reset,
    /// `status`：当前会话 / 模型 / 网关 / 话题开关。
    Status,
    /// `use <id>`：按 id 查一个会话。
    Use(String),
}

/// 解析 `/agent` 后面的原文。
///
/// 只认第一个词，其余整段保留——标题里可以有空格。未知子命令一律当 `help`：
/// 命令是给人用的，给用法比给"未知命令"更有用。
pub fn parse_subcommand(raw_args: &str) -> Subcommand {
    let raw_args = raw_args.trim();
    let (head, rest) = match raw_args.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (raw_args, ""),
    };
    match head.to_ascii_lowercase().as_str() {
        "new" | "open" => Subcommand::New(rest.to_owned()),
        "list" | "ls" => Subcommand::List,
        "reset" | "clear" => Subcommand::Reset,
        "status" | "state" => Subcommand::Status,
        "use" | "switch" => Subcommand::Use(rest.to_owned()),
        _ => Subcommand::Help,
    }
}

/// 用法说明。
pub fn help_text() -> String {
    [
        "AI Agent 会话管理",
        "用法：/agent <子命令>",
        "· /agent new [标题] —— 私聊里开一个新会话（话题）",
        "· /agent list —— 列出当前 chat 的会话（本地 + TDLib 话题）",
        "· /agent use <id> —— 查看某个会话（路由仍按 chat + topic 自动进行）",
        "· /agent reset —— 清空当前会话的历史",
        "· /agent status —— 会话 / 模型 / 网关 / 话题开关",
        "· /agent help —— 这份说明",
        "直接发消息（群里先 @ 我）就会走 Agent；不需要每次 new。",
    ]
    .join("\n")
}

/// 渲染会话列表：本地会话与 TDLib 话题按话题号合并。
///
/// 本地有会话的用本地标题（用户自己起的），TDLib 里多出来的话题单独列出来，
/// 提示"还没建会话"——这样既能看到全部话题，又不会把两种来源算重。
pub fn render_session_list(
    chat_id: i64,
    sessions: &[SessionSummary],
    topics: &[TopicInfo],
) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut covered: Vec<i32> = Vec::new();

    for session in sessions {
        covered.push(session.topic_id.unwrap_or(0));
        lines.push(format!(
            "· {} 「{}」 · {} 条 · {}",
            session.id,
            title_or_default(&session.title),
            session.message_count,
            kind_label(session.topic_id),
        ));
    }

    for topic in topics {
        if covered.contains(&topic.topic_id) {
            continue;
        }
        lines.push(format!(
            "· {chat_id}__{} 「{}」 · TDLib 话题（本插件还没建会话）",
            topic.topic_id,
            title_or_default(&topic.name),
        ));
    }

    if lines.is_empty() {
        return "当前 chat 还没有 AI 会话：私聊里发一条消息，或 /agent new [标题] 开一个。"
            .to_owned();
    }
    lines.join("\n")
}

/// 会话摘要里"是否话题"的标签。
fn kind_label(topic_id: Option<i32>) -> &'static str {
    if topic_id.is_some() {
        "话题"
    } else {
        "主聊天"
    }
}

/// 标题为空时的占位。
fn title_or_default(title: &str) -> &str {
    let title = title.trim();
    if title.is_empty() { "未命名" } else { title }
}

/// 状态文案（纯函数，便于单测；`status` 子命令只负责取数据）。
pub fn format_status(
    session_id: &str,
    message_count: usize,
    model: &str,
    reply_mode: ReplyMode,
    gateway_available: bool,
    topics_enabled: bool,
) -> String {
    let model = model.trim();
    let model = if model.is_empty() {
        "网关默认"
    } else {
        model
    };
    format!(
        "Agent 状态\n· 当前会话：{}（{} 条消息）\n· 模型：{}\n· 回复形态：{}\n· 网关：{}\n· 话题：{}",
        session_id,
        message_count,
        model,
        reply_mode_name(reply_mode),
        gateway_available_label(gateway_available),
        topic_label(topics_enabled),
    )
}

/// 回复形态的稳定名字（与配置值一致）。
fn reply_mode_name(mode: ReplyMode) -> &'static str {
    match mode {
        ReplyMode::Plain => "plain",
        ReplyMode::Markdown => "markdown",
        ReplyMode::Rich => "rich",
    }
}

/// 网关状态文案。
fn gateway_available_label(available: bool) -> &'static str {
    if available {
        "可用（正常推理）"
    } else {
        "不在场（降级为兜底文案）"
    }
}

/// 话题开关文案。
fn topic_label(enabled: bool) -> &'static str {
    if enabled {
        "开启（私聊首条消息自动开话题）"
    } else {
        "关闭（消息留在主聊天）"
    }
}

/// 标题归一化：压空白、截到 `max_chars`、空则用默认名。
fn normalize_title(raw: &str, max_chars: usize) -> String {
    let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return DEFAULT_TOPIC_TITLE.to_owned();
    }
    let title: String = cleaned.chars().take(max_chars.max(1)).collect();
    title.trim().to_owned()
}

/// 登记命令（宿主 `fuyubot.host.commands` 服务）。
pub fn register(host: &HostApi) {
    let payload = json!({
        "name": "agent",
        "description": "AI Agent 会话管理：new / list / use / reset / status / help",
        "role": "user",
    })
    .to_string();
    match host.call_service("fuyubot.host.commands", "register", &payload) {
        Ok(_) => host.info("agent: /agent 已登记进命令表"),
        Err(error) => host.warn(&format!("agent: 登记 /agent 失败：{error}")),
    }
}

/// 分派一条命令；返回要回给用户的文本（None = 不回复）。
///
/// 由 worker 线程调用（不在宿主回调里），所以这里的 TDLib 调用可以阻塞。
pub fn dispatch(state: &PluginState, message: &IncomingMessage) -> AgentResult<Option<String>> {
    let Some(command) = message.command() else {
        return Ok(None);
    };
    // 防御：worker 已经筛过一遍；这里再确认一次，绝不抢别的插件的命令。
    if command.name != "agent" || command.addressed_elsewhere {
        return Ok(None);
    }

    let reply = match parse_subcommand(&command.raw_args) {
        Subcommand::Help => help_text(),
        Subcommand::New(raw_title) => new_session(state, message, &raw_title)?,
        Subcommand::List => list_sessions(state, message)?,
        Subcommand::Reset => reset_session(state, message)?,
        Subcommand::Status => status_text(state, message)?,
        Subcommand::Use(raw_id) => use_session(state, message, &raw_id)?,
    };
    Ok(Some(reply))
}

/// `new [标题]`：只允许私聊，建话题并确保本地会话存在。
fn new_session(
    state: &PluginState,
    message: &IncomingMessage,
    raw_title: &str,
) -> AgentResult<String> {
    if !message.is_private() {
        return Ok(
            "新会话只能在私聊里开；群里直接在话题里 @ 我，消息会按 chat + topic 自动归档。"
                .to_owned(),
        );
    }

    let title = normalize_title(raw_title, state.config.topic_title_max_chars);
    let topic = state.td.create_topic(
        message.chat_id,
        &title,
        state.config.topic_icon_color,
        state.config.action_timeout_ms,
    )?;
    let key = ChatKey::new(message.chat_id, Some(topic.topic_id));
    state.store.ensure(&key, &title)?;
    Ok(format!(
        "已开新会话 #{}「{}」\n接下来在这个话题里直接说话就行。",
        topic.topic_id, title
    ))
}

/// `list`：本地会话 + TDLib 话题；TDLib 失败就只列本地。
fn list_sessions(state: &PluginState, message: &IncomingMessage) -> AgentResult<String> {
    let sessions = state.store.list(message.chat_id)?;
    let topics = match state.td.list_topics(
        message.chat_id,
        LIST_TOPICS_LIMIT,
        state.config.action_timeout_ms,
    ) {
        Ok(topics) => topics,
        Err(error) => {
            state
                .host
                .info(&format!("agent: 列 TDLib 话题失败，只列本地会话：{error}"));
            Vec::new()
        }
    };
    Ok(render_session_list(message.chat_id, &sessions, &topics))
}

/// `reset`：清空当前会话的消息（保留会话本身）。
fn reset_session(state: &PluginState, message: &IncomingMessage) -> AgentResult<String> {
    let key = ChatKey::new(message.chat_id, message.topic_id);
    let count = state
        .store
        .load(&key)?
        .map(|session| session.messages.len())
        .unwrap_or(0);
    state.store.reset(&key)?;
    if count == 0 {
        Ok(format!("会话 {} 本来就没有消息。", key.file_stem()))
    } else {
        Ok(format!(
            "已清空会话 {} 的 {} 条消息。",
            key.file_stem(),
            count
        ))
    }
}

/// `status`：取数据，文案交给纯函数。
fn status_text(state: &PluginState, message: &IncomingMessage) -> AgentResult<String> {
    let key = ChatKey::new(message.chat_id, message.topic_id);
    let count = state
        .store
        .load(&key)?
        .map(|session| session.messages.len())
        .unwrap_or(0);
    Ok(format_status(
        &key.file_stem(),
        count,
        &state.config.model,
        state.config.reply_mode,
        state.engine.gateway().available(),
        state.config.topic_auto_create,
    ))
}

/// `use <id>`：只做查看；路由仍按 chat + topic 自动进行。
fn use_session(
    state: &PluginState,
    message: &IncomingMessage,
    raw_id: &str,
) -> AgentResult<String> {
    let id = raw_id.split_whitespace().next().unwrap_or("").trim();
    if id.is_empty() {
        let sessions = state.store.list(message.chat_id)?;
        let ids: Vec<&str> = sessions.iter().map(|session| session.id.as_str()).collect();
        return Ok(if ids.is_empty() {
            "用法：/agent use <会话 id>。当前 chat 还没有会话。".to_owned()
        } else {
            format!(
                "用法：/agent use <会话 id>。当前 chat 的会话：{}",
                ids.join("、")
            )
        });
    }

    match state.store.find(id)? {
        Some(session) => {
            let count = session.messages.len();
            Ok(format!(
                "会话 {}「{}」：{} 条消息。\n本插件按 chat + topic 自动路由，继续聊直接在对应私聊 / 话题里发消息即可；use 只用于查看和确认。",
                session.id,
                title_or_default(&session.title),
                count
            ))
        }
        None => Ok(format!(
            "找不到会话 {id}；用 /agent list 看当前 chat 的会话。"
        )),
    }
}
