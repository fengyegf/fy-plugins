//! 网关响应解析：三种协议各一套形状，统一成 `Completion`。
//!
//! 网关（`fengyegf.llm`）把上游响应原样放进 `data`：
//! `{"ok":true,"protocol":"openai-chat","data":{…上游原始响应…}}`。
//! 所以这里只回答"这种协议从哪几个键取文本 / 结束原因 / 用量"，不碰网络与宿主，
//! 可以纯单测。
//!
//! 协议名做宽松识别：只有 responses 与 messages（或 anthropic）是特例，其余按
//! OpenAI Chat 处理——网关配置里写 `chat`、响应里写 `openai-chat`，两种都能对上。

use serde_json::Value;

use super::toolcall::{ToolCall, parse_tool_calls};

/// 解析时认的三种协议形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolKind {
    /// OpenAI Chat Completions（`choices[0].message`）。
    OpenAiChat,
    /// OpenAI Responses（`output_text` / `output[].content[]`）。
    OpenAiResponses,
    /// Anthropic Messages（`content[].text`）。
    AnthropicMessages,
}

/// 宽松识别协议名：`chat` / `openai-chat` / `responses` / `messages` /
/// `anthropic-messages` 都能落到三类里。
pub(crate) fn protocol_kind(name: &str) -> ProtocolKind {
    let lower = name.trim().to_ascii_lowercase();
    if lower.contains("responses") {
        ProtocolKind::OpenAiResponses
    } else if lower.contains("messages") || lower.contains("anthropic") {
        ProtocolKind::AnthropicMessages
    } else {
        ProtocolKind::OpenAiChat
    }
}

/// 一次完成请求的结果。
#[derive(Debug, Clone, Default)]
pub struct Completion {
    /// 助手文本（多块已按顺序拼接）。
    pub content: String,
    /// 模型要调用的工具（空 = 纯文本回复）。
    pub tool_calls: Vec<ToolCall>,
    /// 上游回报的模型；网关没回就是 `None`。
    pub model: Option<String>,
    /// 结束原因：chat 的 `finish_reason`、messages 的 `stop_reason`、responses 的 `status`。
    pub finish_reason: Option<String>,
    /// 原样保留的 `usage` 对象（各协议字段不同，不在插件侧归一）。
    pub usage: Value,
}

impl Completion {
    /// 按协议名解析网关照给的那段 `data`。
    pub fn from_data(data: &Value, protocol: &str) -> Self {
        match protocol_kind(protocol) {
            ProtocolKind::OpenAiChat => Self::from_openai_chat(data),
            ProtocolKind::OpenAiResponses => Self::from_openai_responses(data),
            ProtocolKind::AnthropicMessages => Self::from_anthropic_messages(data),
        }
    }

    /// OpenAI Chat Completions：`choices[0].message.content`。
    ///
    /// `content` 可能是字符串，也可能是 `[{type:"text",text:"…"}]` 这样的分块数组
    /// （多模态 / 部分兼容网关会这么回），两种都按顺序拼成一段文本。
    pub fn from_openai_chat(data: &Value) -> Self {
        let choice = data
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first());
        Self {
            content: choice
                .and_then(|choice| choice.get("message"))
                .map(chat_message_text)
                .unwrap_or_default(),
            tool_calls: parse_tool_calls(ProtocolKind::OpenAiChat, data),
            model: string_field(data, "model"),
            finish_reason: choice.and_then(|choice| string_field(choice, "finish_reason")),
            usage: usage_of(data),
        }
    }

    /// OpenAI Responses：优先 `output_text`，否则遍历 `output[].content[].text`。
    pub fn from_openai_responses(data: &Value) -> Self {
        let mut content = String::new();
        if let Some(text) = data.get("output_text").and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            content.push_str(text);
        }
        if content.is_empty()
            && let Some(output) = data.get("output").and_then(Value::as_array)
        {
            for item in output {
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    content.push_str(&text_parts(parts));
                }
            }
        }
        Self {
            content,
            tool_calls: parse_tool_calls(ProtocolKind::OpenAiResponses, data),
            model: string_field(data, "model"),
            // Responses 没有 finish_reason：用 status（completed / incomplete）代替，
            // 未完成时再看 incomplete_details.reason。
            finish_reason: string_field(data, "status").or_else(|| {
                data.get("incomplete_details")
                    .and_then(|details| string_field(details, "reason"))
            }),
            usage: usage_of(data),
        }
    }

    /// Anthropic Messages：`content[]` 里 `type == "text"` 的块按顺序拼接。
    pub fn from_anthropic_messages(data: &Value) -> Self {
        let mut content = String::new();
        if let Some(blocks) = data.get("content").and_then(Value::as_array) {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    content.push_str(text);
                }
            }
        }
        Self {
            content,
            tool_calls: parse_tool_calls(ProtocolKind::AnthropicMessages, data),
            model: string_field(data, "model"),
            finish_reason: string_field(data, "stop_reason"),
            usage: usage_of(data),
        }
    }
}

/// 取一个字符串字段（数字也转成字符串，网关偶尔回数字型 code）。
fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|field| match field {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

/// `usage` 对象原样带走；没有就是 `Null`。
fn usage_of(data: &Value) -> Value {
    data.get("usage").cloned().unwrap_or(Value::Null)
}

/// `message.content`：字符串直接用，数组走分块拼接。
fn chat_message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => text_parts(parts),
        _ => String::new(),
    }
}

/// 文本分块拼接：只认"带 text 的文本块"，图片 / 工具调用等块跳过。
fn text_parts(parts: &[Value]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            Value::String(text) => out.push_str(text),
            Value::Object(fields) => {
                let kind = fields.get("type").and_then(Value::as_str);
                let text_like = matches!(
                    kind,
                    None | Some("text" | "output_text" | "input_text" | "text_delta")
                );
                if text_like && let Some(text) = fields.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
            _ => {}
        }
    }
    out
}
