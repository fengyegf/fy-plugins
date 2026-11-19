//! OpenAI Responses 的流式事件、流读取器与聚合。
//!
//! SSE 每条事件带 event: 名，data 是 JSON（内部通常还有 type 字段）。本模块：
//! - 把 data 解析成 ResponseStreamEvent（已知事件强类型，未知事件放在 Unknown 里保留原始 JSON）；
//! - ResponseStream 提供 next_event() 与 Iterator，遇 response.completed / response.failed /
//!   response.incomplete / error 或 [DONE] 时结束；
//! - aggregate() 消费剩余事件，返回最终 ResponseObject（优先用 response.completed 里
//!   完整的 response；上游没给终止事件时，用增量事件重建输出）。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::core::{LlmError, LlmResult, SseEvent, SseResponse};

use super::types::{
    Extra, ReasoningSummaryPart, ResponseContentPart, ResponseObject, ResponseOutputItem,
};

// ===== 事件负载 =====

/// 生命周期事件（response.created / in_progress / queued / completed / failed / incomplete）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseLifecycleEvent {
    /// 当前响应对象快照。
    pub response: ResponseObject,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 输出项事件（response.output_item.added / done）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputItemEvent {
    /// 新增 / 完成的输出项。
    pub item: ResponseOutputItem,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 内容块事件（response.content_part.added / done）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentPartEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 内容块下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,
    /// 内容块。
    pub part: ResponseContentPart,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 文本增量事件（response.output_text.delta / refusal.delta / reasoning_text.delta）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextDeltaEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 内容块下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,
    /// 增量文本。
    pub delta: String,
    /// logprob 明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Value>,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 文本完成事件（response.output_text.done / reasoning_text.done）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextDoneEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 内容块下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,
    /// 完整文本。
    pub text: String,
    /// logprob 明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Value>,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 拒答完成事件（response.refusal.done）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RefusalDoneEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 内容块下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,
    /// 完整拒答文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 函数参数增量事件（response.function_call_arguments.delta）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArgumentsDeltaEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 参数增量。
    pub delta: String,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 函数参数完成事件（response.function_call_arguments.done）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArgumentsDoneEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 完整参数 JSON 字符串。
    pub arguments: String,
    /// 函数名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 推理摘要增量事件（response.reasoning_summary_text.delta）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReasoningSummaryDeltaEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 摘要片段下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_index: Option<u32>,
    /// 摘要增量。
    pub delta: String,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 推理摘要完成事件（response.reasoning_summary_text.done）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReasoningSummaryDoneEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 摘要片段下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_index: Option<u32>,
    /// 完整摘要文本。
    pub text: String,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 推理摘要片段事件（response.reasoning_summary_part.added / done）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReasoningSummaryPartEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 摘要片段下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_index: Option<u32>,
    /// 摘要片段。
    pub part: ResponseContentPart,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 文本注解事件（response.output_text.annotation.added）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnnotationEvent {
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 输出项下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    /// 内容块下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,
    /// 注解对象。
    pub annotation: Value,
    /// 注解下标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation_index: Option<u32>,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 流内错误事件（type = error）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamErrorEvent {
    /// 错误码。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 错误信息。
    pub message: String,
    /// 出错参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// 事件序号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 未建模事件的兜底：保留 event: 名与完整 data。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnknownEvent {
    /// 事件名（SSE event 名或 data 里的 type）。
    pub event_type: String,
    /// 原始 data JSON。
    pub data: Value,
}

// ===== 事件枚举 =====

/// Responses 流式事件。
///
/// 已知事件强类型；未知事件放 Unknown，原始 JSON 原样保留，不会中断流。
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseStreamEvent {
    /// response.created
    Created(ResponseLifecycleEvent),
    /// response.in_progress
    InProgress(ResponseLifecycleEvent),
    /// response.queued
    Queued(ResponseLifecycleEvent),
    /// response.completed（终止）
    Completed(ResponseLifecycleEvent),
    /// response.failed（终止）
    Failed(ResponseLifecycleEvent),
    /// response.incomplete（终止）
    Incomplete(ResponseLifecycleEvent),
    /// response.output_item.added
    OutputItemAdded(OutputItemEvent),
    /// response.output_item.done
    OutputItemDone(OutputItemEvent),
    /// response.content_part.added
    ContentPartAdded(ContentPartEvent),
    /// response.content_part.done
    ContentPartDone(ContentPartEvent),
    /// response.output_text.delta
    OutputTextDelta(TextDeltaEvent),
    /// response.output_text.done
    OutputTextDone(TextDoneEvent),
    /// response.refusal.delta
    RefusalDelta(TextDeltaEvent),
    /// response.refusal.done
    RefusalDone(RefusalDoneEvent),
    /// response.function_call_arguments.delta
    FunctionCallArgumentsDelta(ArgumentsDeltaEvent),
    /// response.function_call_arguments.done
    FunctionCallArgumentsDone(ArgumentsDoneEvent),
    /// response.reasoning_summary_text.delta
    ReasoningSummaryTextDelta(ReasoningSummaryDeltaEvent),
    /// response.reasoning_summary_text.done
    ReasoningSummaryTextDone(ReasoningSummaryDoneEvent),
    /// response.reasoning_summary_part.added
    ReasoningSummaryPartAdded(ReasoningSummaryPartEvent),
    /// response.reasoning_summary_part.done
    ReasoningSummaryPartDone(ReasoningSummaryPartEvent),
    /// response.reasoning_text.delta
    ReasoningTextDelta(TextDeltaEvent),
    /// response.reasoning_text.done
    ReasoningTextDone(TextDoneEvent),
    /// response.output_text.annotation.added
    OutputTextAnnotationAdded(AnnotationEvent),
    /// error（终止）
    Error(StreamErrorEvent),
    /// 未建模事件
    Unknown(UnknownEvent),
}

macro_rules! parse_event {
    ($payload:ty, $variant:ident, $value:expr, $event_type:expr) => {{
        match serde_json::from_value::<$payload>($value.clone()) {
            Ok(inner) => Self::$variant(inner),
            Err(_) => Self::unknown($event_type, $value),
        }
    }};
}

impl ResponseStreamEvent {
    /// 从一条 SSE 事件解析；空 data / [DONE] 返回 Ok(None)。
    ///
    /// 事件类型优先取 data JSON 的 type，其次取 SSE 的 event: 名。
    pub fn from_sse(event: &SseEvent) -> LlmResult<Option<Self>> {
        let data = event.data.trim();
        if data.is_empty() || event.is_done() {
            return Ok(None);
        }
        let value = event.json()?;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .unwrap_or("message")
            .to_owned();
        Ok(Some(Self::from_value(&event_type, value)))
    }

    /// 按事件类型把 JSON 转成强类型事件；无法匹配负载时退化为 Unknown。
    pub fn from_value(event_type: &str, value: Value) -> Self {
        match event_type {
            "response.created" => parse_event!(ResponseLifecycleEvent, Created, value, event_type),
            "response.in_progress" => {
                parse_event!(ResponseLifecycleEvent, InProgress, value, event_type)
            }
            "response.queued" => parse_event!(ResponseLifecycleEvent, Queued, value, event_type),
            "response.completed" => {
                parse_event!(ResponseLifecycleEvent, Completed, value, event_type)
            }
            "response.failed" => parse_event!(ResponseLifecycleEvent, Failed, value, event_type),
            "response.incomplete" => {
                parse_event!(ResponseLifecycleEvent, Incomplete, value, event_type)
            }
            "response.output_item.added" => {
                parse_event!(OutputItemEvent, OutputItemAdded, value, event_type)
            }
            "response.output_item.done" => {
                parse_event!(OutputItemEvent, OutputItemDone, value, event_type)
            }
            "response.content_part.added" => {
                parse_event!(ContentPartEvent, ContentPartAdded, value, event_type)
            }
            "response.content_part.done" => {
                parse_event!(ContentPartEvent, ContentPartDone, value, event_type)
            }
            "response.output_text.delta" => {
                parse_event!(TextDeltaEvent, OutputTextDelta, value, event_type)
            }
            "response.output_text.done" => {
                parse_event!(TextDoneEvent, OutputTextDone, value, event_type)
            }
            "response.refusal.delta" => {
                parse_event!(TextDeltaEvent, RefusalDelta, value, event_type)
            }
            "response.refusal.done" => {
                parse_event!(RefusalDoneEvent, RefusalDone, value, event_type)
            }
            "response.function_call_arguments.delta" => {
                parse_event!(
                    ArgumentsDeltaEvent,
                    FunctionCallArgumentsDelta,
                    value,
                    event_type
                )
            }
            "response.function_call_arguments.done" => {
                parse_event!(
                    ArgumentsDoneEvent,
                    FunctionCallArgumentsDone,
                    value,
                    event_type
                )
            }
            "response.reasoning_summary_text.delta" => {
                parse_event!(
                    ReasoningSummaryDeltaEvent,
                    ReasoningSummaryTextDelta,
                    value,
                    event_type
                )
            }
            "response.reasoning_summary_text.done" => {
                parse_event!(
                    ReasoningSummaryDoneEvent,
                    ReasoningSummaryTextDone,
                    value,
                    event_type
                )
            }
            "response.reasoning_summary_part.added" => {
                parse_event!(
                    ReasoningSummaryPartEvent,
                    ReasoningSummaryPartAdded,
                    value,
                    event_type
                )
            }
            "response.reasoning_summary_part.done" => {
                parse_event!(
                    ReasoningSummaryPartEvent,
                    ReasoningSummaryPartDone,
                    value,
                    event_type
                )
            }
            "response.reasoning_text.delta" => {
                parse_event!(TextDeltaEvent, ReasoningTextDelta, value, event_type)
            }
            "response.reasoning_text.done" => {
                parse_event!(TextDoneEvent, ReasoningTextDone, value, event_type)
            }
            "response.output_text.annotation.added" => {
                parse_event!(
                    AnnotationEvent,
                    OutputTextAnnotationAdded,
                    value,
                    event_type
                )
            }
            "error" => parse_event!(StreamErrorEvent, Error, value, event_type),
            _ => Self::unknown(event_type, value),
        }
    }

    fn unknown(event_type: &str, value: Value) -> Self {
        Self::Unknown(UnknownEvent {
            event_type: event_type.to_owned(),
            data: value,
        })
    }

    /// 协议事件名。
    pub fn event_type(&self) -> &str {
        match self {
            Self::Created(_) => "response.created",
            Self::InProgress(_) => "response.in_progress",
            Self::Queued(_) => "response.queued",
            Self::Completed(_) => "response.completed",
            Self::Failed(_) => "response.failed",
            Self::Incomplete(_) => "response.incomplete",
            Self::OutputItemAdded(_) => "response.output_item.added",
            Self::OutputItemDone(_) => "response.output_item.done",
            Self::ContentPartAdded(_) => "response.content_part.added",
            Self::ContentPartDone(_) => "response.content_part.done",
            Self::OutputTextDelta(_) => "response.output_text.delta",
            Self::OutputTextDone(_) => "response.output_text.done",
            Self::RefusalDelta(_) => "response.refusal.delta",
            Self::RefusalDone(_) => "response.refusal.done",
            Self::FunctionCallArgumentsDelta(_) => "response.function_call_arguments.delta",
            Self::FunctionCallArgumentsDone(_) => "response.function_call_arguments.done",
            Self::ReasoningSummaryTextDelta(_) => "response.reasoning_summary_text.delta",
            Self::ReasoningSummaryTextDone(_) => "response.reasoning_summary_text.done",
            Self::ReasoningSummaryPartAdded(_) => "response.reasoning_summary_part.added",
            Self::ReasoningSummaryPartDone(_) => "response.reasoning_summary_part.done",
            Self::ReasoningTextDelta(_) => "response.reasoning_text.delta",
            Self::ReasoningTextDone(_) => "response.reasoning_text.done",
            Self::OutputTextAnnotationAdded(_) => "response.output_text.annotation.added",
            Self::Error(_) => "error",
            Self::Unknown(inner) => &inner.event_type,
        }
    }

    /// 是否是终止事件。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed(_) | Self::Failed(_) | Self::Incomplete(_) | Self::Error(_)
        )
    }

    /// 事件携带的当前响应快照（仅生命周期事件）。
    pub fn response(&self) -> Option<&ResponseObject> {
        match self {
            Self::Created(event)
            | Self::InProgress(event)
            | Self::Queued(event)
            | Self::Completed(event)
            | Self::Failed(event)
            | Self::Incomplete(event) => Some(&event.response),
            _ => None,
        }
    }

    /// 输出项事件携带的 item。
    pub fn item(&self) -> Option<&ResponseOutputItem> {
        match self {
            Self::OutputItemAdded(event) | Self::OutputItemDone(event) => Some(&event.item),
            _ => None,
        }
    }

    /// 增量 / 完成事件携带的文本。
    pub fn delta(&self) -> Option<&str> {
        match self {
            Self::OutputTextDelta(event)
            | Self::RefusalDelta(event)
            | Self::ReasoningTextDelta(event) => Some(&event.delta),
            Self::FunctionCallArgumentsDelta(event) => Some(&event.delta),
            Self::ReasoningSummaryTextDelta(event) => Some(&event.delta),
            _ => None,
        }
    }

    /// 完成事件携带的完整文本。
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::OutputTextDone(event) | Self::ReasoningTextDone(event) => Some(&event.text),
            Self::ReasoningSummaryTextDone(event) => Some(&event.text),
            _ => None,
        }
    }
}

impl Serialize for ResponseStreamEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = match self {
            Self::Created(event) => tagged("response.created", event),
            Self::InProgress(event) => tagged("response.in_progress", event),
            Self::Queued(event) => tagged("response.queued", event),
            Self::Completed(event) => tagged("response.completed", event),
            Self::Failed(event) => tagged("response.failed", event),
            Self::Incomplete(event) => tagged("response.incomplete", event),
            Self::OutputItemAdded(event) => tagged("response.output_item.added", event),
            Self::OutputItemDone(event) => tagged("response.output_item.done", event),
            Self::ContentPartAdded(event) => tagged("response.content_part.added", event),
            Self::ContentPartDone(event) => tagged("response.content_part.done", event),
            Self::OutputTextDelta(event) => tagged("response.output_text.delta", event),
            Self::OutputTextDone(event) => tagged("response.output_text.done", event),
            Self::RefusalDelta(event) => tagged("response.refusal.delta", event),
            Self::RefusalDone(event) => tagged("response.refusal.done", event),
            Self::FunctionCallArgumentsDelta(event) => {
                tagged("response.function_call_arguments.delta", event)
            }
            Self::FunctionCallArgumentsDone(event) => {
                tagged("response.function_call_arguments.done", event)
            }
            Self::ReasoningSummaryTextDelta(event) => {
                tagged("response.reasoning_summary_text.delta", event)
            }
            Self::ReasoningSummaryTextDone(event) => {
                tagged("response.reasoning_summary_text.done", event)
            }
            Self::ReasoningSummaryPartAdded(event) => {
                tagged("response.reasoning_summary_part.added", event)
            }
            Self::ReasoningSummaryPartDone(event) => {
                tagged("response.reasoning_summary_part.done", event)
            }
            Self::ReasoningTextDelta(event) => tagged("response.reasoning_text.delta", event),
            Self::ReasoningTextDone(event) => tagged("response.reasoning_text.done", event),
            Self::OutputTextAnnotationAdded(event) => {
                tagged("response.output_text.annotation.added", event)
            }
            Self::Error(event) => tagged("error", event),
            Self::Unknown(inner) => {
                let mut data = inner.data.clone();
                if let Value::Object(map) = &mut data {
                    if !map.contains_key("type") {
                        map.insert("type".to_owned(), Value::String(inner.event_type.clone()));
                    }
                }
                data
            }
        };
        value.serialize(serializer)
    }
}

fn tagged<T: Serialize>(event_type: &str, payload: &T) -> Value {
    let mut value = serde_json::to_value(payload).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.insert("type".to_owned(), Value::String(event_type.to_owned()));
    }
    value
}

impl<'de> Deserialize<'de> for ResponseStreamEvent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message")
            .to_owned();
        Ok(Self::from_value(&event_type, value))
    }
}

// ===== 流 =====

/// Responses 流读取器：逐事件读取，可聚合为最终响应。
pub struct ResponseStream {
    reader: SseResponse,
    finished: bool,
    final_response: Option<ResponseObject>,
    terminal_error: Option<String>,
}

impl ResponseStream {
    /// 包装共享层返回的 SSE 读取器。
    pub fn new(reader: SseResponse) -> Self {
        Self {
            reader,
            finished: false,
            final_response: None,
            terminal_error: None,
        }
    }

    /// 最近一次生命周期事件里的响应快照。
    pub fn final_response(&self) -> Option<&ResponseObject> {
        self.final_response.as_ref()
    }

    /// 流内失败 / 错误事件的原因（若有）。
    pub fn terminal_error(&self) -> Option<&str> {
        self.terminal_error.as_deref()
    }

    /// 读下一条事件；流结束（含 [DONE] 与所有终止事件）返回 Ok(None)。
    pub fn next_event(&mut self) -> LlmResult<Option<ResponseStreamEvent>> {
        if self.finished {
            return Ok(None);
        }
        loop {
            match self.reader.next_event() {
                Ok(None) => {
                    self.finished = true;
                    return Ok(None);
                }
                Err(error) => {
                    self.finished = true;
                    return Err(error);
                }
                Ok(Some(sse)) => {
                    if sse.is_done() {
                        self.finished = true;
                        return Ok(None);
                    }
                    let parsed = match ResponseStreamEvent::from_sse(&sse) {
                        Ok(Some(event)) => event,
                        Ok(None) => continue,
                        Err(error) => {
                            self.finished = true;
                            return Err(error);
                        }
                    };
                    if let Some(response) = parsed.response() {
                        self.final_response = Some(response.clone());
                    }
                    if parsed.is_terminal() {
                        self.finished = true;
                    }
                    match &parsed {
                        ResponseStreamEvent::Failed(event) => {
                            self.terminal_error = Some(failure_message(&event.response));
                        }
                        ResponseStreamEvent::Error(event) => {
                            self.terminal_error = Some(format!(
                                "{}（code={}）",
                                event.message,
                                event.code.as_deref().unwrap_or("unknown")
                            ));
                        }
                        _ => {}
                    }
                    return Ok(Some(parsed));
                }
            }
        }
    }

    /// 消费剩余事件并返回最终响应。
    ///
    /// 优先返回 response.completed / response.incomplete 里的完整对象；
    /// 上游没有终止事件时，用 output_item / text delta / function arguments 增量重建。
    pub fn aggregate(mut self) -> LlmResult<ResponseObject> {
        let mut accumulator = Accumulator::default();
        for event in self.by_ref() {
            accumulator.apply(&event?);
        }
        if let Some(error) = self
            .terminal_error
            .clone()
            .or_else(|| accumulator.error.clone())
        {
            return Err(LlmError::Other(format!(
                "OpenAI Responses 流式失败：{error}"
            )));
        }
        Ok(accumulator.finish())
    }
}

impl Iterator for ResponseStream {
    type Item = LlmResult<ResponseStreamEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_event() {
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

fn failure_message(response: &ResponseObject) -> String {
    response
        .error
        .as_ref()
        .and_then(|error| error.message.clone())
        .or_else(|| {
            response
                .incomplete_details
                .as_ref()
                .and_then(|details| details.reason.clone())
        })
        .unwrap_or_else(|| "response.failed".to_owned())
}

// ===== 聚合 =====

#[derive(Default)]
struct Accumulator {
    response: Option<ResponseObject>,
    items: BTreeMap<usize, ResponseOutputItem>,
    parts: BTreeMap<(usize, usize), ResponseContentPart>,
    texts: BTreeMap<(usize, usize), String>,
    refusals: BTreeMap<(usize, usize), String>,
    reasoning_texts: BTreeMap<(usize, usize), String>,
    arguments: BTreeMap<usize, String>,
    summary_texts: BTreeMap<(usize, usize), String>,
    summary_parts: BTreeMap<(usize, usize), ResponseContentPart>,
    error: Option<String>,
}

impl Accumulator {
    fn apply(&mut self, event: &ResponseStreamEvent) {
        match event {
            ResponseStreamEvent::Created(event)
            | ResponseStreamEvent::InProgress(event)
            | ResponseStreamEvent::Queued(event)
            | ResponseStreamEvent::Completed(event)
            | ResponseStreamEvent::Incomplete(event) => {
                self.response = Some(event.response.clone());
            }
            ResponseStreamEvent::Failed(event) => {
                self.response = Some(event.response.clone());
                self.error = Some(failure_message(&event.response));
            }
            ResponseStreamEvent::OutputItemAdded(event)
            | ResponseStreamEvent::OutputItemDone(event) => {
                if let Some(index) = event.output_index {
                    self.items.insert(index as usize, event.item.clone());
                }
            }
            ResponseStreamEvent::ContentPartAdded(event)
            | ResponseStreamEvent::ContentPartDone(event) => {
                if let (Some(output), Some(content)) = (event.output_index, event.content_index) {
                    self.parts
                        .insert((output as usize, content as usize), event.part.clone());
                }
            }
            ResponseStreamEvent::OutputTextDelta(event) => {
                if let Some(index) = event.output_index {
                    let key = (index as usize, event.content_index.unwrap_or(0) as usize);
                    self.texts.entry(key).or_default().push_str(&event.delta);
                }
            }
            ResponseStreamEvent::OutputTextDone(event) => {
                if let Some(index) = event.output_index {
                    let key = (index as usize, event.content_index.unwrap_or(0) as usize);
                    self.texts.insert(key, event.text.clone());
                }
            }
            ResponseStreamEvent::RefusalDelta(event) => {
                if let Some(index) = event.output_index {
                    let key = (index as usize, event.content_index.unwrap_or(0) as usize);
                    self.refusals.entry(key).or_default().push_str(&event.delta);
                }
            }
            ResponseStreamEvent::RefusalDone(event) => {
                if let Some((Some(output), Some(content))) =
                    Some((event.output_index, event.content_index))
                {
                    if let Some(refusal) = &event.refusal {
                        self.refusals
                            .insert((output as usize, content as usize), refusal.clone());
                    }
                }
            }
            ResponseStreamEvent::FunctionCallArgumentsDelta(event) => {
                if let Some(index) = event.output_index {
                    self.arguments
                        .entry(index as usize)
                        .or_default()
                        .push_str(&event.delta);
                }
            }
            ResponseStreamEvent::FunctionCallArgumentsDone(event) => {
                if let Some(index) = event.output_index {
                    self.arguments
                        .insert(index as usize, event.arguments.clone());
                }
            }
            ResponseStreamEvent::ReasoningSummaryTextDelta(event) => {
                if let Some(index) = event.output_index {
                    let key = (index as usize, event.summary_index.unwrap_or(0) as usize);
                    self.summary_texts
                        .entry(key)
                        .or_default()
                        .push_str(&event.delta);
                }
            }
            ResponseStreamEvent::ReasoningSummaryTextDone(event) => {
                if let Some(index) = event.output_index {
                    let key = (index as usize, event.summary_index.unwrap_or(0) as usize);
                    self.summary_texts.insert(key, event.text.clone());
                }
            }
            ResponseStreamEvent::ReasoningSummaryPartAdded(event)
            | ResponseStreamEvent::ReasoningSummaryPartDone(event) => {
                if let (Some(output), Some(summary)) = (event.output_index, event.summary_index) {
                    self.summary_parts
                        .insert((output as usize, summary as usize), event.part.clone());
                }
            }
            ResponseStreamEvent::ReasoningTextDelta(event) => {
                if let Some(index) = event.output_index {
                    let key = (index as usize, event.content_index.unwrap_or(0) as usize);
                    self.reasoning_texts
                        .entry(key)
                        .or_default()
                        .push_str(&event.delta);
                }
            }
            ResponseStreamEvent::ReasoningTextDone(event) => {
                if let Some(index) = event.output_index {
                    let key = (index as usize, event.content_index.unwrap_or(0) as usize);
                    self.reasoning_texts.insert(key, event.text.clone());
                }
            }
            ResponseStreamEvent::Error(event) => {
                self.error = Some(format!(
                    "{}（code={}）",
                    event.message,
                    event.code.as_deref().unwrap_or("unknown")
                ));
            }
            _ => {}
        }
    }

    fn finish(self) -> ResponseObject {
        let mut response = self.response.clone().unwrap_or_default();
        if response.object.is_none() {
            response.object = Some("response".to_owned());
        }
        let has_output = response
            .output
            .as_ref()
            .map(|items| !items.is_empty())
            .unwrap_or(false);
        if !has_output {
            let output = self.build_output(&response);
            if !output.is_empty() {
                response.output = Some(output);
            }
        }
        if response.output_text.is_none() {
            let text = response.output_text();
            if !text.is_empty() {
                response.output_text = Some(text);
            }
        }
        if response.status.is_none() {
            response.status = Some("incomplete".to_owned());
        }
        response
    }

    fn build_output(&self, response: &ResponseObject) -> Vec<ResponseOutputItem> {
        let mut indexed: BTreeMap<usize, ResponseOutputItem> = self.items.clone();
        if indexed.is_empty() {
            if let Some(items) = &response.output {
                for (index, item) in items.iter().enumerate() {
                    indexed.insert(index, item.clone());
                }
            }
        }

        let mut indices: BTreeSet<usize> = BTreeSet::new();
        for (key, _) in &self.texts {
            indices.insert(key.0);
        }
        for (key, _) in &self.refusals {
            indices.insert(key.0);
        }
        for (key, _) in &self.reasoning_texts {
            indices.insert(key.0);
        }
        for (key, _) in &self.summary_texts {
            indices.insert(key.0);
        }
        for (key, _) in &self.parts {
            indices.insert(key.0);
        }
        for (key, _) in &self.summary_parts {
            indices.insert(key.0);
        }
        for &index in self.arguments.keys() {
            indices.insert(index);
        }
        for index in indices {
            indexed.entry(index).or_default();
        }

        let mut output = Vec::with_capacity(indexed.len());
        for (index, mut item) in indexed {
            let mut content: BTreeMap<usize, ResponseContentPart> = BTreeMap::new();
            if let Some(existing) = &item.content {
                for (position, part) in existing.iter().enumerate() {
                    content.insert(position, part.clone());
                }
            }
            for ((output_index, content_index), part) in &self.parts {
                if *output_index == index {
                    content.insert(*content_index, part.clone());
                }
            }
            for ((output_index, content_index), text) in &self.texts {
                if *output_index != index {
                    continue;
                }
                let part = content
                    .entry(*content_index)
                    .or_insert_with(|| ResponseContentPart {
                        part_type: Some("output_text".to_owned()),
                        ..Default::default()
                    });
                part.text = Some(text.clone());
            }
            for ((output_index, content_index), refusal) in &self.refusals {
                if *output_index != index {
                    continue;
                }
                let part = content
                    .entry(*content_index)
                    .or_insert_with(|| ResponseContentPart {
                        part_type: Some("refusal".to_owned()),
                        ..Default::default()
                    });
                part.refusal = Some(refusal.clone());
            }
            for ((output_index, content_index), text) in &self.reasoning_texts {
                if *output_index != index {
                    continue;
                }
                let part = content
                    .entry(*content_index)
                    .or_insert_with(|| ResponseContentPart {
                        part_type: Some("reasoning_text".to_owned()),
                        ..Default::default()
                    });
                part.text = Some(text.clone());
            }
            if !content.is_empty() {
                item.content = Some(content.into_values().collect());
                if item.item_type.is_none() {
                    item.item_type = Some("message".to_owned());
                }
                if item.item_type.as_deref() == Some("message") && item.role.is_none() {
                    item.role = Some("assistant".to_owned());
                }
            }

            if let Some(arguments) = self.arguments.get(&index) {
                item.arguments = Some(arguments.clone());
                if item.item_type.is_none() {
                    item.item_type = Some("function_call".to_owned());
                }
            }

            let mut summary: BTreeMap<usize, ReasoningSummaryPart> = BTreeMap::new();
            if let Some(existing) = &item.summary {
                for (position, part) in existing.iter().enumerate() {
                    summary.insert(position, part.clone());
                }
            }
            for ((output_index, summary_index), text) in &self.summary_texts {
                if *output_index != index {
                    continue;
                }
                summary.insert(
                    *summary_index,
                    ReasoningSummaryPart {
                        part_type: Some("summary_text".to_owned()),
                        text: Some(text.clone()),
                        ..Default::default()
                    },
                );
            }
            for ((output_index, summary_index), part) in &self.summary_parts {
                if *output_index != index {
                    continue;
                }
                summary.insert(
                    *summary_index,
                    ReasoningSummaryPart {
                        part_type: part.part_type.clone(),
                        text: part.text.clone(),
                        ..Default::default()
                    },
                );
            }
            if !summary.is_empty() {
                item.summary = Some(summary.into_values().collect());
                if item.item_type.is_none() {
                    item.item_type = Some("reasoning".to_owned());
                }
            }

            output.push(item);
        }
        output
    }
}
