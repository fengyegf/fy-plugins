//! Anthropic Messages 流式事件、流迭代器与聚合。
//!
//! 官方流式事件（每个 `data` 都是带 `type` 的 JSON，`event:` 行给同名事件名）：
//!
//! - `message_start`：带一条不完整的 `message`（content 为空、usage 只有 input/output）；
//! - `content_block_start`：带 `index` 与初始块；
//! - `content_block_delta`：`delta.type` 为 `text_delta` / `input_json_delta` /
//!   `thinking_delta` / `signature_delta` / `citations_delta`；
//! - `content_block_stop`：块结束；
//! - `message_delta`：最终的 `stop_reason` / `stop_sequence` / `stop_details` 与 usage；
//! - `message_stop`：协议终止标记；
//! - `ping`：心跳；
//! - `error`：流内错误。
//!
//! 未知事件类型不会报错，统一进 [`MessageStreamEvent::Unknown`]，可原样往返。

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::core::{LlmError, LlmResult, SseResponse};

use super::types::{ContentBlock, Extra, MessageResponse, Usage};

/// `message_start` 事件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageStartEvent {
    /// 事件类型（`message_start`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 初始消息（content 为空，usage 含 input/output token）。
    pub message: MessageResponse,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `content_block_start` 事件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockStartEvent {
    /// 事件类型（`content_block_start`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 内容块下标。
    #[serde(default)]
    pub index: u32,
    /// 初始内容块。
    pub content_block: ContentBlock,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `content_block_delta` 事件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockDeltaEvent {
    /// 事件类型（`content_block_delta`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 内容块下标。
    #[serde(default)]
    pub index: u32,
    /// 增量内容。
    pub delta: ContentBlockDelta,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// 内容块增量：用 `delta_type` 区分，各字段按需出现。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockDelta {
    /// `text_delta` / `input_json_delta` / `thinking_delta` / `signature_delta` / `citations_delta`。
    #[serde(rename = "type")]
    pub delta_type: String,
    /// `text_delta` 的文本增量。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `input_json_delta` 的部分 JSON 字符串。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_json: Option<String>,
    /// `thinking_delta` 的思考增量。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// `signature_delta` 的签名增量。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// `citations_delta` 的引用对象。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<Value>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `content_block_stop` 事件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockStopEvent {
    /// 事件类型（`content_block_stop`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 内容块下标。
    #[serde(default)]
    pub index: u32,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `message_delta` 事件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaEvent {
    /// 事件类型（`message_delta`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 最终的停止信息。
    pub delta: MessageDelta,
    /// usage 增量（部分字段）。
    #[serde(default)]
    pub usage: Usage,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `message_delta.delta`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageDelta {
    /// 停止原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// 命中的停止序列。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// 结构化停止细节。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<Value>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `message_stop` 事件（协议终止标记）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageStopEvent {
    /// 事件类型（`message_stop`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `ping` 事件（心跳）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PingEvent {
    /// 事件类型（`ping`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `error` 事件。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamErrorEvent {
    /// 事件类型（`error`）。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 错误详情。
    pub error: StreamError,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// `error.error`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamError {
    /// 错误类型（`overloaded_error` / `invalid_request_error` …）。
    #[serde(rename = "type")]
    pub error_type: String,
    /// 错误信息。
    #[serde(default)]
    pub message: String,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl StreamErrorEvent {
    /// 转成统一的 [`LlmError`]。
    pub fn into_error(self) -> LlmError {
        LlmError::Other(format!(
            "Anthropic 流式错误（{}）：{}",
            self.error.error_type, self.error.message
        ))
    }
}

/// 一条 Anthropic Messages 流式事件。
///
/// 反序列化按 `type` 字段分派；未知事件进 [`MessageStreamEvent::Unknown`]，
/// 序列化时原样写回。
#[derive(Debug, Clone, PartialEq)]
pub enum MessageStreamEvent {
    /// `message_start`。
    MessageStart(MessageStartEvent),
    /// `content_block_start`。
    ContentBlockStart(ContentBlockStartEvent),
    /// `content_block_delta`。
    ContentBlockDelta(ContentBlockDeltaEvent),
    /// `content_block_stop`。
    ContentBlockStop(ContentBlockStopEvent),
    /// `message_delta`。
    MessageDelta(MessageDeltaEvent),
    /// `message_stop`。
    MessageStop(MessageStopEvent),
    /// `ping`。
    Ping(PingEvent),
    /// `error`。
    Error(StreamErrorEvent),
    /// 未知事件（原样保留）。
    Unknown(Value),
}

impl Default for MessageStreamEvent {
    fn default() -> Self {
        Self::Unknown(Value::Null)
    }
}

fn parse_as<T: DeserializeOwned>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

impl MessageStreamEvent {
    /// 从 JSON 值解析；未知 `type` 或形状不匹配时进 `Unknown`，不会返回错误。
    pub fn from_value(value: Value) -> Self {
        let event_name = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let parsed = match event_name.as_str() {
            "message_start" => parse_as::<MessageStartEvent>(&value).map(Self::MessageStart),
            "content_block_start" => {
                parse_as::<ContentBlockStartEvent>(&value).map(Self::ContentBlockStart)
            }
            "content_block_delta" => {
                parse_as::<ContentBlockDeltaEvent>(&value).map(Self::ContentBlockDelta)
            }
            "content_block_stop" => {
                parse_as::<ContentBlockStopEvent>(&value).map(Self::ContentBlockStop)
            }
            "message_delta" => parse_as::<MessageDeltaEvent>(&value).map(Self::MessageDelta),
            "message_stop" => parse_as::<MessageStopEvent>(&value).map(Self::MessageStop),
            "ping" => parse_as::<PingEvent>(&value).map(Self::Ping),
            "error" => parse_as::<StreamErrorEvent>(&value).map(Self::Error),
            _ => None,
        };
        parsed.unwrap_or(Self::Unknown(value))
    }

    /// 事件类型名；未知事件从原始 JSON 的 `type` 取。
    pub fn event_type(&self) -> &str {
        match self {
            Self::MessageStart(event) => &event.event_type,
            Self::ContentBlockStart(event) => &event.event_type,
            Self::ContentBlockDelta(event) => &event.event_type,
            Self::ContentBlockStop(event) => &event.event_type,
            Self::MessageDelta(event) => &event.event_type,
            Self::MessageStop(event) => &event.event_type,
            Self::Ping(event) => &event.event_type,
            Self::Error(event) => &event.event_type,
            Self::Unknown(value) => value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        }
    }

    /// 是否是协议终止事件（`message_stop` 或 `error`）。
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::MessageStop(_) | Self::Error(_))
    }

    /// 是否是流内错误事件。
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}

impl Serialize for MessageStreamEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::MessageStart(event) => event.serialize(serializer),
            Self::ContentBlockStart(event) => event.serialize(serializer),
            Self::ContentBlockDelta(event) => event.serialize(serializer),
            Self::ContentBlockStop(event) => event.serialize(serializer),
            Self::MessageDelta(event) => event.serialize(serializer),
            Self::MessageStop(event) => event.serialize(serializer),
            Self::Ping(event) => event.serialize(serializer),
            Self::Error(event) => event.serialize(serializer),
            Self::Unknown(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MessageStreamEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Ok(Self::from_value(value))
    }
}

/// Anthropic Messages 的 SSE 流。
///
/// [`Iterator`] 产出 [`LlmResult<MessageStreamEvent>`]：
/// - 读到 `message_stop` 后自动结束（不再读后续事件）；
/// - 读到 `error` 事件时产出一个 [`LlmError::Other`]，然后结束。
///
/// 也可以直接用 [`MessageStream::next_event`] 拿到 `Error` 事件本身。
pub struct MessageStream {
    reader: SseResponse,
    finished: bool,
}

impl MessageStream {
    /// 包装一个 SSE 读取器。
    pub fn new(reader: SseResponse) -> Self {
        Self {
            reader,
            finished: false,
        }
    }

    /// 是否已经结束（终止标记 / 错误 / 流关闭）。
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// 读下一条事件；流结束返回 `Ok(None)`。
    ///
    /// 空 `data` 事件（keep-alive 注释等）会被跳过。读到 `message_stop` / `error` 后
    /// 置为结束状态，再调用返回 `Ok(None)`。
    pub fn next_event(&mut self) -> LlmResult<Option<MessageStreamEvent>> {
        if self.finished {
            return Ok(None);
        }
        loop {
            let event = match self.reader.next_event() {
                Ok(Some(event)) => event,
                Ok(None) => {
                    self.finished = true;
                    return Ok(None);
                }
                Err(error) => {
                    self.finished = true;
                    return Err(error);
                }
            };
            if event.is_done() {
                self.finished = true;
                return Ok(None);
            }
            if event.data.trim().is_empty() {
                continue;
            }
            let value = match event.json() {
                Ok(value) => value,
                Err(error) => {
                    self.finished = true;
                    return Err(error);
                }
            };
            let parsed = MessageStreamEvent::from_value(value);
            if parsed.is_terminal() {
                self.finished = true;
            }
            return Ok(Some(parsed));
        }
    }

    /// 消费剩余事件并聚合成完整响应。
    ///
    /// 必须在开始迭代之前调用（它只消费「剩下」的事件）。
    pub fn aggregate(mut self) -> LlmResult<MessageResponse> {
        let mut accumulator = MessageAccumulator::default();
        while let Some(event) = self.next_event()? {
            accumulator.apply(event)?;
        }
        accumulator.finish()
    }
}

impl Iterator for MessageStream {
    type Item = LlmResult<MessageStreamEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_event() {
            Ok(Some(MessageStreamEvent::Error(event))) => Some(Err(event.into_error())),
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

/// 把一串事件聚合成最终响应（测试 / 已缓存事件场景）。
pub fn aggregate_events<I>(events: I) -> LlmResult<MessageResponse>
where
    I: IntoIterator<Item = LlmResult<MessageStreamEvent>>,
{
    let mut accumulator = MessageAccumulator::default();
    for event in events {
        accumulator.apply(event?)?;
    }
    accumulator.finish()
}

/// 按 `message_start` / `content_block_*` / `message_delta` 增量拼出完整响应。
#[derive(Debug, Default)]
struct MessageAccumulator {
    message: Option<MessageResponse>,
    /// 每个 tool_use / server_tool_use 块累计的 `partial_json`。
    partial_json: HashMap<usize, String>,
}

impl MessageAccumulator {
    fn apply(&mut self, event: MessageStreamEvent) -> LlmResult<()> {
        match event {
            MessageStreamEvent::MessageStart(event) => {
                self.message = Some(event.message);
            }
            MessageStreamEvent::ContentBlockStart(event) => {
                let index = event.index as usize;
                let message = self.message.get_or_insert_with(MessageResponse::default);
                ensure_block(message, index);
                message.content[index] = event.content_block;
            }
            MessageStreamEvent::ContentBlockDelta(event) => {
                let index = event.index as usize;
                let message = self.message.get_or_insert_with(MessageResponse::default);
                ensure_block(message, index);
                let block = &mut message.content[index];
                let delta = event.delta;
                match delta.delta_type.as_str() {
                    "text_delta" => append_piece(&mut block.text, delta.text),
                    "thinking_delta" => append_piece(&mut block.thinking, delta.thinking),
                    "signature_delta" => append_piece(&mut block.signature, delta.signature),
                    "input_json_delta" => {
                        let buffer = self.partial_json.entry(index).or_default();
                        if let Some(piece) = delta.partial_json {
                            buffer.push_str(&piece);
                        }
                    }
                    "citations_delta" => {
                        if let Some(citation) = delta.citation {
                            push_citation(block, citation);
                        }
                    }
                    _ => {}
                }
            }
            MessageStreamEvent::ContentBlockStop(event) => {
                self.finish_block(event.index as usize);
            }
            MessageStreamEvent::MessageDelta(event) => {
                let message = self.message.get_or_insert_with(MessageResponse::default);
                if let Some(stop_reason) = event.delta.stop_reason {
                    message.stop_reason = Some(stop_reason);
                }
                if let Some(stop_sequence) = event.delta.stop_sequence {
                    message.stop_sequence = Some(stop_sequence);
                }
                if let Some(stop_details) = event.delta.stop_details {
                    message.stop_details = Some(stop_details);
                }
                message.usage.merge(&event.usage);
            }
            MessageStreamEvent::MessageStop(_) | MessageStreamEvent::Ping(_) => {}
            MessageStreamEvent::Error(event) => return Err(event.into_error()),
            MessageStreamEvent::Unknown(_) => {}
        }
        Ok(())
    }

    /// 结束一个工具块：解析累计的 partial_json。
    ///
    /// 解析失败（例如 `max_tokens` 截断）不报错，把原始片段放进块的 `extra["partial_json"]`，
    /// 保证信息不丢。
    fn finish_block(&mut self, index: usize) {
        let Some(buffer) = self.partial_json.remove(&index) else {
            return;
        };
        if buffer.trim().is_empty() {
            return;
        }
        let Some(message) = self.message.as_mut() else {
            return;
        };
        let Some(block) = message.content.get_mut(index) else {
            return;
        };
        match serde_json::from_str::<Value>(&buffer) {
            Ok(input) => block.input = Some(input),
            Err(_) => {
                block
                    .extra
                    .insert("partial_json".to_owned(), Value::String(buffer));
            }
        }
    }

    fn finish(mut self) -> LlmResult<MessageResponse> {
        // 流被截断时，未收到 content_block_stop 的工具块也尽量收尾。
        let mut pending: Vec<usize> = self.partial_json.keys().copied().collect();
        pending.sort_unstable();
        for index in pending {
            self.finish_block(index);
        }
        self.message.ok_or_else(|| {
            LlmError::Other("流式响应里没有 message_start 事件，无法聚合".to_owned())
        })
    }
}

fn ensure_block(message: &mut MessageResponse, index: usize) {
    while message.content.len() <= index {
        message.content.push(ContentBlock::default());
    }
}

fn append_piece(target: &mut Option<String>, piece: Option<String>) {
    if let Some(piece) = piece {
        target.get_or_insert_with(String::new).push_str(&piece);
    }
}

fn push_citation(block: &mut ContentBlock, citation: Value) {
    match &mut block.citations {
        Some(Value::Array(items)) => items.push(citation),
        _ => block.citations = Some(Value::Array(vec![citation])),
    }
}
