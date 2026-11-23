//! `fengyegf.agent.chat`：聊一句（有 chat_id 带会话历史，没有就直通网关）。

use serde_json::{Value, json};

use crate::PluginState;
use crate::core::{AgentResult, RunMode};

use super::{TurnArgs, direct_messages, parse_args, turn_value};

/// 工具名。
pub const NAME: &str = "fengyegf.agent.chat";

/// 工具说明（注册到宿主工具表）。
pub const DESCRIPTION: &str =
    "和 Agent 聊一句：带 chat_id 时按该会话（chat + topic）取历史并落盘，否则直通网关不落历史。";

/// 入参 schema。
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "prompt": { "type": "string" },
            "session": { "type": "string" },
            "chat_id": { "type": "integer" },
            "topic_id": { "type": "integer" },
            "system": { "type": "string" },
            "model": { "type": "string" },
            "max_tokens": { "type": "integer" }
        },
        "required": ["prompt"],
        "additionalProperties": false
    })
}

/// 处理：带 chat_id 走引擎（取历史 + 落盘），否则直通网关（不读也不写历史）。
pub fn handle(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let args: TurnArgs = parse_args(input)?;
    let prompt = args.prompt()?;

    let Some(key) = args.key() else {
        // 没有 chat_id = 没有可归属的会话：直通，ai 自己管上下文。
        let completion = state.engine.complete_messages(
            direct_messages(state, &args, prompt),
            args.model.as_deref(),
            args.max_tokens,
        )?;
        let mut value = json!({
            "ok": true,
            "session": Value::Null,
            "reply": completion.content,
            "usage": completion.usage,
        });
        if let Some(model) = completion.model {
            value["model"] = Value::String(model);
        }
        return Ok(value);
    };

    let request = args.request(key, prompt);
    let outcome = state
        .engine
        .run(&state.store, &request, RunMode::Single, 1)?;
    Ok(turn_value(outcome, false))
}
