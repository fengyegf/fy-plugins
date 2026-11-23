//! 工具调用的协议适配：从上游响应认出「模型要调工具」，再把调用与结果
//! 翻译成各协议能接受的后续输入。
//!
//! 三个协议形状不同，但抽象成同一件事：
//!
//! - 认出调用：OpenAI chat 的 `message.tool_calls`、Anthropic `content[]` 里
//!   `type == "tool_use"` 的块、Responses `output[]` 里 `type == "function_call"` 的条目；
//! - 回填上下文：assistant 消息（带调用）→ 工具结果消息；
//! - 入参一律存成 JSON 字符串：chat / responses 原生就是字符串，Anthropic 给的是对象，
//!   这里统一 `to_string`，执行前再解析。
//!
//! 纯函数、纯单测：不碰宿主、不碰网络。

use serde_json::{Value, json};

use super::completion::ProtocolKind;

/// 模型发起的一次工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// 协议侧的调用 id（chat / responses 的 `call_id` / Anthropic 的 `tool_use.id`）。
    pub id: String,
    /// 工具名（宿主注册的全名，如 `fengyegf.agent.session`）。
    pub name: String,
    /// 入参 JSON 字符串。
    pub arguments: String,
}

impl ToolCall {
    /// 入参解析成对象；不是 JSON 对象当空对象（工具自己会回 `bad_request`）。
    pub fn arguments_value(&self) -> Value {
        match serde_json::from_str::<Value>(&self.arguments) {
            Ok(value) if value.is_object() => value,
            _ => json!({}),
        }
    }
}

/// 从上游响应里取出工具调用；没有就是空列表。
pub(crate) fn parse_tool_calls(protocol: ProtocolKind, data: &Value) -> Vec<ToolCall> {
    match protocol {
        ProtocolKind::OpenAiChat => data
            .pointer("/choices/0/message/tool_calls")
            .and_then(Value::as_array)
            .map(|calls| calls.iter().filter_map(chat_tool_call).collect())
            .unwrap_or_default(),
        ProtocolKind::OpenAiResponses => data
            .get("output")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(responses_tool_call).collect())
            .unwrap_or_default(),
        ProtocolKind::AnthropicMessages => data
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| blocks.iter().filter_map(messages_tool_call).collect())
            .unwrap_or_default(),
    }
}

/// chat：`{id, type:"function", function:{name, arguments}}`。
fn chat_tool_call(call: &Value) -> Option<ToolCall> {
    let function = call.get("function")?;
    let name = function.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    Some(ToolCall {
        id: call
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        name: name.to_owned(),
        arguments: arguments_string(function.get("arguments")),
    })
}

/// responses：`{type:"function_call", call_id, name, arguments}`。
fn responses_tool_call(item: &Value) -> Option<ToolCall> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let name = item.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    Some(ToolCall {
        id: item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        name: name.to_owned(),
        arguments: arguments_string(item.get("arguments")),
    })
}

/// messages：`{type:"tool_use", id, name, input}`。
fn messages_tool_call(block: &Value) -> Option<ToolCall> {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return None;
    }
    let name = block.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    Some(ToolCall {
        id: block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        name: name.to_owned(),
        arguments: match block.get("input") {
            Some(input) if input.is_object() => input.to_string(),
            _ => "{}".to_owned(),
        },
    })
}

/// 入参字段：字符串原样用，对象 / 数组序列化，缺失当空对象。
fn arguments_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => text.clone(),
        Some(value) if !value.is_null() => value.to_string(),
        _ => "{}".to_owned(),
    }
}

/// assistant 消息（含工具调用）。返回待追加的输入条目（Responses 可能拆成多条）。
pub(crate) fn assistant_entries(
    protocol: ProtocolKind,
    text: &str,
    calls: &[ToolCall],
) -> Vec<Value> {
    match protocol {
        ProtocolKind::OpenAiChat => {
            let tool_calls: Vec<Value> = calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments }
                    })
                })
                .collect();
            let mut message = json!({ "role": "assistant", "tool_calls": tool_calls });
            // content 允许缺省（只调工具时上游就是 null）；有文本才带上。
            if !text.trim().is_empty() {
                message["content"] = Value::String(text.to_owned());
            }
            vec![message]
        }
        ProtocolKind::OpenAiResponses => {
            let mut items = Vec::with_capacity(calls.len() + 1);
            if !text.trim().is_empty() {
                items.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text }]
                }));
            }
            for call in calls {
                items.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.arguments
                }));
            }
            items
        }
        ProtocolKind::AnthropicMessages => {
            let mut blocks = Vec::with_capacity(calls.len() + 1);
            if !text.trim().is_empty() {
                blocks.push(json!({ "type": "text", "text": text }));
            }
            for call in calls {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments_value()
                }));
            }
            vec![json!({ "role": "assistant", "content": blocks })]
        }
    }
}

/// 工具结果消息：
/// - chat：一条 `role:"tool"`；
/// - responses：一条 `function_call_output`；
/// - messages：合并进**同一条** user 消息的 `tool_result` 块（Anthropic 要求角色交替）。
pub(crate) fn result_entries(
    protocol: ProtocolKind,
    calls: &[ToolCall],
    outputs: &[String],
) -> Vec<Value> {
    match protocol {
        ProtocolKind::OpenAiChat => calls
            .iter()
            .zip(outputs)
            .map(|(call, output)| {
                json!({ "role": "tool", "tool_call_id": call.id, "content": output })
            })
            .collect(),
        ProtocolKind::OpenAiResponses => calls
            .iter()
            .zip(outputs)
            .map(|(call, output)| {
                json!({ "type": "function_call_output", "call_id": call.id, "output": output })
            })
            .collect(),
        ProtocolKind::AnthropicMessages => {
            let blocks: Vec<Value> = calls
                .iter()
                .zip(outputs)
                .map(|(call, output)| {
                    json!({
                        "type": "tool_result",
                        "tool_use_id": call.id,
                        "content": output
                    })
                })
                .collect();
            vec![json!({ "role": "user", "content": blocks })]
        }
    }
}
