//! 工具实现：一个工具一个文件，schema / 校验 / 处理都放一起。
//!
//! 公共部分（入参解析、对话入参、引擎结果 → 出参）也放这里：这些契约被多个工具共用，
//! 散到各文件迟早会漂移。

pub mod chat;
pub mod run;
pub mod send;
pub mod session;
pub mod title;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::PluginState;
use crate::core::{AgentError, AgentResult, ChatKey, TurnOutcome, TurnRequest};

/// 解析 JSON 入参（非对象 / 类型错都归到 bad_request）。
pub(crate) fn parse_args<T: DeserializeOwned>(input: &Value) -> AgentResult<T> {
    let value = if input.is_null() {
        json!({})
    } else {
        input.clone()
    };
    if !value.is_object() {
        return Err(AgentError::BadRequest("入参必须是 JSON 对象".to_owned()));
    }
    serde_json::from_value(value)
        .map_err(|error| AgentError::BadRequest(format!("入参不合法：{error}")))
}

/// 去空白；空串当缺省，避免把 `""` 当成显式参数传下去。
pub(crate) fn clean(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

/// `chat` / `run` 的入参（两个对话工具共用同一份形状）。
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct TurnArgs {
    #[serde(default)]
    pub(crate) prompt: Option<String>,
    #[serde(default)]
    pub(crate) session: Option<String>,
    #[serde(default)]
    pub(crate) chat_id: Option<i64>,
    #[serde(default)]
    pub(crate) topic_id: Option<i32>,
    #[serde(default)]
    pub(crate) system: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) max_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) mode: Option<String>,
    #[serde(default)]
    pub(crate) max_turns: Option<u32>,
}

impl TurnArgs {
    /// 必填的 prompt（去空白后不能为空）。
    fn prompt(&self) -> AgentResult<&str> {
        match self.prompt.as_deref().map(str::trim) {
            Some(text) if !text.is_empty() => Ok(text),
            _ => Err(AgentError::BadRequest(
                "缺少 prompt（非空字符串）".to_owned(),
            )),
        }
    }

    /// 有 chat_id 才有会话 key；否则走无历史直通。
    fn key(&self) -> Option<ChatKey> {
        self.chat_id
            .map(|chat_id| ChatKey::new(chat_id, self.topic_id))
    }

    /// 组一轮推理请求。
    fn request(&self, key: ChatKey, prompt: &str) -> TurnRequest {
        TurnRequest {
            key,
            user_text: prompt.to_owned(),
            session_id: clean(self.session.as_deref()),
            system_prompt: clean(self.system.as_deref()),
            model: clean(self.model.as_deref()),
            max_tokens: self.max_tokens,
        }
    }
}

/// 无会话直通的 messages：系统提示（入参优先，其次配置）+ 本轮用户输入。
pub(crate) fn direct_messages(state: &PluginState, args: &TurnArgs, prompt: &str) -> Vec<Value> {
    let system = clean(args.system.as_deref())
        .unwrap_or_else(|| state.config.system_prompt.trim().to_owned());
    let mut messages = Vec::with_capacity(2);
    if !system.is_empty() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));
    messages
}

/// 引擎结果 → 出参。
pub(crate) fn turn_value(outcome: TurnOutcome, with_turns: bool) -> Value {
    let mut value = json!({
        "ok": true,
        "session": outcome.session,
        "reply": outcome.reply,
        "usage": outcome.usage,
    });
    if outcome.tool_calls > 0 {
        value["tool_calls"] = json!(outcome.tool_calls);
    }
    if with_turns {
        value["turns"] = json!(outcome.turns);
    } else if let Some(model) = outcome.model {
        value["model"] = Value::String(model);
    }
    value
}
