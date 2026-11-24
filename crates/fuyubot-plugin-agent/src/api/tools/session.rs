//! `fengyegf.agent.session`：会话管理（list / get / reset / delete / create / compress）。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::PluginState;
use crate::core::{AgentError, AgentResult, ChatKey};
use crate::session::{Session, SessionStore};

use super::parse_args;

/// 工具名。
pub const NAME: &str = "fengyegf.agent.session";

/// 工具说明（注册到宿主工具表）。
pub const DESCRIPTION: &str =
    "管理会话：list / get / reset / delete / create / compress（compress = 把历史压成摘要）。";

/// 入参 schema。
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["list", "get", "reset", "delete", "create", "compress"] },
            "chat_id": { "type": "integer" },
            "topic_id": { "type": "integer" },
            "id": { "type": "string" },
            "title": { "type": "string" }
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

/// 入参。
#[derive(Debug, Clone, Default, Deserialize)]
struct SessionArgs {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    chat_id: Option<i64>,
    #[serde(default)]
    topic_id: Option<i32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

/// session 的 action（解析后的枚举，便于单测与 match）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionAction {
    List,
    Get,
    Reset,
    Delete,
    Create,
    Compress,
}

impl SessionArgs {
    /// 解析 action；大小写宽松，未知值直接参数错误。
    fn action(&self) -> AgentResult<SessionAction> {
        let action = self
            .action
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase);
        match action.as_deref() {
            Some("list") => Ok(SessionAction::List),
            Some("get") => Ok(SessionAction::Get),
            Some("reset") => Ok(SessionAction::Reset),
            Some("delete") => Ok(SessionAction::Delete),
            Some("create") => Ok(SessionAction::Create),
            Some("compress") | Some("compact") | Some("summarize") => Ok(SessionAction::Compress),
            Some(other) => Err(AgentError::BadRequest(format!(
                "未知 session action：{other}（只认 list / get / reset / delete / create / compress）"
            ))),
            None => Err(AgentError::BadRequest(
                "缺少 action（list / get / reset / delete / create / compress）".to_owned(),
            )),
        }
    }

    fn title(&self) -> &str {
        self.title.as_deref().map(str::trim).unwrap_or("")
    }
}

/// 处理：list / get / reset / delete / create / compress。
pub fn handle(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let args: SessionArgs = parse_args(input)?;
    let action = args.action()?;
    let store = &state.store;

    match action {
        SessionAction::List => {
            let sessions = match args.chat_id {
                Some(chat_id) => store.list(chat_id)?,
                None => store.list_all()?,
            };
            Ok(json!({ "ok": true, "sessions": sessions }))
        }
        SessionAction::Get => {
            let session = match session_id(&args) {
                Some(id) => store.find(id)?,
                None => store.load(&resolve_key(store, &args)?)?,
            };
            match session {
                Some(session) => Ok(json!({ "ok": true, "session": session_json(&session)? })),
                None => Err(AgentError::BadRequest("会话不存在".to_owned())),
            }
        }
        SessionAction::Reset => {
            let key = resolve_key(store, &args)?;
            // removed = 清掉的消息条数（会话本身保留）。
            let removed = store
                .load(&key)?
                .map(|session| session.messages.len())
                .unwrap_or(0);
            store.reset(&key)?;
            Ok(json!({ "ok": true, "removed": removed }))
        }
        SessionAction::Delete => {
            let key = resolve_key(store, &args)?;
            let removed = usize::from(store.delete(&key)?);
            Ok(json!({ "ok": true, "removed": removed }))
        }
        SessionAction::Create => {
            let key = resolve_key(store, &args)?;
            let title = args.title();
            let title = if title.is_empty() { "AI 会话" } else { title };
            let session = store.ensure(&key, title)?;
            Ok(json!({ "ok": true, "session": session_json(&session)? }))
        }
        SessionAction::Compress => {
            let key = resolve_key(store, &args)?;
            let compression = state.engine.compress(store, &key)?;
            Ok(json!({
                "ok": true,
                "session": compression.session,
                "removed": compression.removed,
                "summary": compression.summary,
            }))
        }
    }
}

/// 会话 id 入参（空串 = 没给）。
fn session_id(args: &SessionArgs) -> Option<&str> {
    args.id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

/// 用 chat_id + topic_id 定位；没给 chat_id 时退而用 id 反查会话的 key。
fn resolve_key(store: &SessionStore, args: &SessionArgs) -> AgentResult<ChatKey> {
    if let Some(chat_id) = args.chat_id {
        return Ok(ChatKey::new(chat_id, args.topic_id));
    }
    if let Some(id) = session_id(args) {
        return store
            .find(id)?
            .map(|session| session.key())
            .ok_or_else(|| AgentError::BadRequest(format!("会话不存在：{id}")));
    }
    Err(AgentError::BadRequest(
        "缺少 chat_id（或用 id 指定会话）".to_owned(),
    ))
}

fn session_json(session: &Session) -> AgentResult<Value> {
    serde_json::to_value(session)
        .map_err(|error| AgentError::Store(format!("序列化会话 {} 失败：{error}", session.id)))
}
