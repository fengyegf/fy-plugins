//! 跨模块共享的纯数据类型：不依赖宿主，方便单测。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 会被当成「聊天内容」交给 Agent 的 TDLib 内容类型。
///
/// 服务消息（`messageForumTopicCreated` / `messageChatAddMembers` /
/// `messageBasicGroupChatCreate` 等）没有正文、用户也不期待回答：把它们当用户消息会白烧
/// 一次推理，还会占掉应答窗口。
pub const CHAT_CONTENT_TYPES: &[&str] = &[
    "messageText",
    "messagePhoto",
    "messageVideo",
    "messageDocument",
    "messageAudio",
    "messageVoiceNote",
    "messageVideoNote",
    "messageAnimation",
    "messageSticker",
    "messageLocation",
    "messageVenue",
    "messageContact",
    "messagePoll",
    "messageDice",
    "messageGame",
    "messageStory",
    "messageInvoice",
    "messagePaidMedia",
];

/// 这条内容要不要交给模型。
pub fn is_chat_content(content_type: &str) -> bool {
    CHAT_CONTENT_TYPES.contains(&content_type)
}

/// 把 Markdown 粗略压成适合「草稿流式」显示的纯文本。
///
/// 草稿走的是 TDLib 的 `formattedText`（没有 parse_mode），原样塞 Markdown 会把 `**`、
/// `#`、``` 这些标记直接显示出来。这里只做展示用的粗加工：正式的终稿仍然按
/// `reply_mode` 走 markdown / rich，不受影响。
pub fn strip_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        // 代码围栏只去标记，保留代码内容。
        if trimmed.starts_with("```") {
            continue;
        }
        let mut line = trimmed.to_owned();
        // 标题 / 引用 / 列表前缀。
        if line.starts_with('#') {
            line = line.trim_start_matches('#').trim_start().to_owned();
        }
        if let Some(rest) = line.strip_prefix("> ") {
            line = rest.to_owned();
        }
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = line.strip_prefix(marker) {
                line = format!("• {rest}");
                break;
            }
        }
        // 行内标记：先链接再强调，避免 [**a**](u) 漏掉。
        line = replace_links(&line);
        for marker in ["**", "__", "~~"] {
            line = line.replace(marker, "");
        }
        line = line.replace('`', "");
        out.push_str(&line);
        out.push('\n');
    }
    // 收掉多余空行（草稿里空行也是空隙，太多会显得卡住）。
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.trim_end().to_owned()
}

/// `[文本](链接)` → `文本 (链接)`；`![alt](img)` → `alt`。
fn replace_links(line: &str) -> String {
    let mut rest = line;
    let mut out = String::with_capacity(line.len());
    while let Some(start) = rest.find("](") {
        let Some(open) = rest[..start].rfind('[') else {
            break;
        };
        let Some(close) = rest[start + 2..].find(')') else {
            break;
        };
        let label = &rest[open + 1..start];
        let url = &rest[start + 2..start + 2 + close];
        out.push_str(&rest[..open]);
        if url.starts_with("http") && !label.is_empty() {
            out.push_str(&format!("{label} ({url})"));
        } else {
            out.push_str(label);
        }
        rest = &rest[start + 2 + close + 1..];
    }
    out.push_str(rest);
    out
}

/// 按字符切块（流式草稿用）；空文本返回空列表。
pub fn chunk_text(text: &str, chunk_chars: usize) -> Vec<String> {
    let chunk_chars = chunk_chars.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= chunk_chars {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// 会话定位：chat + 可选话题（thread）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChatKey {
    pub chat_id: i64,
    pub topic_id: Option<i32>,
}

impl ChatKey {
    /// 新键。
    pub fn new(chat_id: i64, topic_id: Option<i32>) -> Self {
        Self { chat_id, topic_id }
    }

    /// 话题号；无话题按 0 记（General 话题没有 id）。
    pub fn topic(&self) -> i32 {
        self.topic_id.unwrap_or(0)
    }

    /// 落盘文件名主干：`<chat_id>__<topic>`。
    pub fn file_stem(&self) -> String {
        format!("{}__{}", self.chat_id, self.topic())
    }
}

/// 会话种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    /// 一对一私聊。
    Private,
    /// 群 / 超级群。
    Group,
    /// 频道。
    Channel,
}

impl ChatKind {
    /// 稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Group => "group",
            Self::Channel => "channel",
        }
    }
}

/// 一条入站消息（`updateNewMessage` 解析结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub chat_id: i64,
    pub message_id: i64,
    pub kind: ChatKind,
    /// 发送者 user id（匿名管理员 / 频道身份没有）。
    pub sender_user_id: Option<i64>,
    /// 以群 / 频道身份发送时的 chat id。
    pub sender_chat_id: Option<i64>,
    /// 发送者是不是 bot（由 TDLib 层查询后回填）。
    pub sender_is_bot: bool,
    pub is_outgoing: bool,
    /// 正文：文本取 text，媒体取 caption，其余为空。
    pub text: String,
    /// 内容类型：text / photo / document 等。
    pub content_type: String,
    /// 所在话题（`messageTopicForum`）。
    pub topic_id: Option<i32>,
    /// 引用的消息 id。
    pub reply_to_message_id: Option<i64>,
    /// TDLib 消息时间戳（unix 秒）。
    pub date: u64,
}

impl IncomingMessage {
    /// 这条消息所属会话。
    pub fn key(&self) -> ChatKey {
        ChatKey::new(self.chat_id, self.topic_id)
    }

    /// 是否私聊。
    pub fn is_private(&self) -> bool {
        self.kind == ChatKind::Private
    }

    /// 给模型看的正文：没有文本时退回一个类型占位。
    pub fn display_text(&self) -> String {
        if self.text.trim().is_empty() {
            format!("[{}]", self.content_type)
        } else {
            self.text.clone()
        }
    }

    /// 解析 `/name@bot args`。
    pub fn command(&self) -> Option<CommandCall> {
        parse_command(&self.text)
    }
}

/// 一条命令调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCall {
    /// 命令名（不带 `/`，已小写）。
    pub name: String,
    /// 是否点名给了别的 bot（`/name@other`）。
    pub addressed_elsewhere: bool,
    /// 空格切开的参数。
    pub args: Vec<String>,
    /// 去掉命令名之后的原文。
    pub raw_args: String,
}

/// 解析命令文本；不是命令返回 `None`。
pub fn parse_command(text: &str) -> Option<CommandCall> {
    let trimmed = text.trim_start();
    let rest = trimmed.strip_prefix('/')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let head = parts.next()?.trim();
    if head.is_empty() {
        return None;
    }
    let (name, target) = match head.split_once('@') {
        Some((name, target)) => (name, Some(target)),
        None => (head, None),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let raw_args = parts.next().unwrap_or("").trim().to_owned();
    Some(CommandCall {
        name: name.to_ascii_lowercase(),
        addressed_elsewhere: target.is_some_and(|target| !target.is_empty()),
        args: raw_args.split_whitespace().map(ToOwned::to_owned).collect(),
        raw_args,
    })
}

/// 一条会话消息（落盘与请求组装共用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// 用户消息。
    pub fn user(content: impl Into<String>, ts: u64) -> Self {
        Self {
            role: "user".to_owned(),
            content: content.into(),
            ts,
            name: None,
        }
    }

    /// 助手消息。
    pub fn assistant(content: impl Into<String>, ts: u64) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: content.into(),
            ts,
            name: None,
        }
    }

    /// 系统消息。
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_owned(),
            content: content.into(),
            ts: 0,
            name: None,
        }
    }
}

/// 推理模式：单次 = 一次调用；循环 = 多轮直到收敛 / 达到上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// 单次对话。
    Single,
    /// 循环对话（Agent 迭代）。
    Loop,
}

impl RunMode {
    /// 宽松解析：single / once / loop / agent。
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "single" | "once" | "one" | "turn" => Some(Self::Single),
            "loop" | "agent" | "iter" | "iterate" => Some(Self::Loop),
            _ => None,
        }
    }

    /// 稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Loop => "loop",
        }
    }
}

/// 一次推理请求。
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub key: ChatKey,
    pub user_text: String,
    /// 指定会话 id；空 = 按 ChatKey 落。
    pub session_id: Option<String>,
    /// 覆盖系统提示词。
    pub system_prompt: Option<String>,
    /// 覆盖模型。
    pub model: Option<String>,
    /// 覆盖 max_tokens。
    pub max_tokens: Option<u32>,
}

/// 一次推理结果。
#[derive(Debug, Clone, Serialize)]
pub struct TurnOutcome {
    pub reply: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub turns: u32,
    /// 这一轮实际执行了几次工具调用（0 = 纯文本）。
    #[serde(default)]
    pub tool_calls: u32,
    #[serde(default)]
    pub usage: Value,
}
