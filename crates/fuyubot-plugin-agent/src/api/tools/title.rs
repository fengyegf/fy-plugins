//! `fengyegf.agent.title`：重命名话题标题（AI / 其它插件用）。

use serde::Deserialize;
use serde_json::{Value, json};

use super::{clean, parse_args};
use crate::PluginState;
use crate::core::{AgentError, AgentResult, ChatKey};

/// 工具名。
pub const NAME: &str = "fengyegf.agent.title";

/// 工具说明（注册到宿主工具表）。
pub const DESCRIPTION: &str = "重命名话题标题（AI 想给当前对话起标题时调用）：给 chat_id + topic_id + title，回写会话落盘标题。";

/// 入参 schema。
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "chat_id": { "type": "integer" },
            "topic_id": { "type": "integer" },
            "title": { "type": "string" }
        },
        "required": ["chat_id", "topic_id", "title"],
        "additionalProperties": false
    })
}

/// 入参。
#[derive(Debug, Clone, Default, Deserialize)]
struct TitleArgs {
    #[serde(default)]
    chat_id: Option<i64>,
    #[serde(default)]
    topic_id: Option<i32>,
    #[serde(default)]
    title: Option<String>,
}

impl TitleArgs {
    /// 校验并取出 `(chat_id, topic_id, title)`。
    fn check(&self) -> AgentResult<(i64, i32, String)> {
        let chat_id = self
            .chat_id
            .ok_or_else(|| AgentError::BadRequest("缺少 chat_id".to_owned()))?;
        let topic_id = self
            .topic_id
            .ok_or_else(|| AgentError::BadRequest("缺少 topic_id（话题模式私聊）".to_owned()))?;
        let title = clean(self.title.as_deref())
            .ok_or_else(|| AgentError::BadRequest("缺少 title".to_owned()))?;
        Ok((chat_id, topic_id, title))
    }
}

/// 处理：改话题标题 + 回写会话落盘标题。
pub fn handle(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let args: TitleArgs = parse_args(input)?;
    let (chat_id, topic_id, title) = args.check()?;
    let limited: String = title
        .chars()
        .take(state.config.topic_title_max_chars.max(4))
        .collect();
    state
        .td
        .edit_topic(chat_id, topic_id, &limited, state.config.action_timeout_ms)?;
    let session_updated = state
        .store
        .rename(&ChatKey::new(chat_id, Some(topic_id)), &limited)
        .unwrap_or(false);
    Ok(json!({
        "ok": true,
        "chat_id": chat_id,
        "topic_id": topic_id,
        "title": limited,
        "session_updated": session_updated,
    }))
}
