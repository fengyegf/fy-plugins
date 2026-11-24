//! `fengyegf.agent.run`：跑 Agent 循环（mode=single 只跑一轮，loop 多轮迭代）。

use serde_json::{Value, json};

use crate::PluginState;
use crate::core::{AgentError, AgentResult, RunMode, consts};

use super::{TurnArgs, direct_messages, parse_args, turn_value};

/// 工具名。
pub const NAME: &str = "fengyegf.agent.run";

/// 工具说明（注册到宿主工具表）。
pub const DESCRIPTION: &str =
    "跑 Agent 循环：mode=loop 时最多 max_turns 轮（默认 3 轮），mode=single 只跑一轮。";

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
            "max_tokens": { "type": "integer" },
            "mode": { "type": "string", "enum": ["single", "loop"] },
            "max_turns": { "type": "integer" }
        },
        "required": ["prompt"],
        "additionalProperties": false
    })
}

/// 处理：有 chat_id 走循环；没有时退化成一次直通（没有历史可迭代）。
pub fn handle(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let args: TurnArgs = parse_args(input)?;
    let prompt = args.prompt()?;
    let mode = match args
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        None => RunMode::Loop,
        Some(text) => RunMode::parse(text).ok_or_else(|| {
            AgentError::BadRequest(format!("未知 mode：{text}（只认 single / loop）"))
        })?,
    };

    let Some(key) = args.key() else {
        let completion = state.engine.complete_messages(
            direct_messages(state, &args, prompt),
            args.model.as_deref(),
            args.max_tokens,
        )?;
        return Ok(json!({
            "ok": true,
            "session": Value::Null,
            "reply": completion.content,
            "turns": 1,
            "usage": completion.usage,
        }));
    };

    let max_turns = match args.max_turns {
        Some(turns) => turns.max(1),
        None if mode == RunMode::Loop => consts::LOOP_MAX_TURNS,
        None => 1,
    };
    let request = args.request(key, prompt);
    let outcome = state.engine.run(&state.store, &request, mode, max_turns)?;
    Ok(turn_value(outcome, true))
}
