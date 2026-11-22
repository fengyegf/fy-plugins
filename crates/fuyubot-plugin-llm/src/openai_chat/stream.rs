//! OpenAI Chat Completions 流式事件、迭代器与聚合。
//!
//! 流格式：SSE，每个事件的 data 是一个 chunk JSON，最后一条是 data: [DONE]。
//! 这里负责：
//! - 把每条 data 反序列化成 ChatCompletionChunk；
//! - 把协议自己的终止标记 [DONE] 当作流结束（不产出 chunk）；
//! - 边读边累积，aggregate() 把剩余事件读完并拼成 ChatCompletionResponse。
//!
//! 注意：部分兼容网关在流里下发 {"error": {...}} 而不是 HTTP 错误，这里会转成
//! LlmError::Sse 返回。

use serde_json::Value;

use crate::core::{LlmError, LlmResult, SseResponse};

use super::types::{
    ChatChoice, ChatCompletionChunk, ChatCompletionResponse, ChatMessage, ContentPart, Delta,
    FunctionCall, MessageContent, ToolCall, Usage,
};

/// Chat Completions 的流式响应。
///
/// 示例（逐块打印，最后聚合成完整响应）：
///
/// ```no_run
/// use llm::{ClientConfig, Transport};
/// use llm::openai_chat::{ChatCompletionRequest, ChatMessage, Client};
///
/// let transport = Transport::new(ClientConfig::new("https://api.openai.com/v1", "sk-test"))?;
/// let client = Client::new(transport);
/// let request = ChatCompletionRequest::new("gpt-4o-mini")
///     .with_messages(vec![ChatMessage::user("hello")]);
/// let mut stream = client.create_stream(&request)?;
/// for chunk in stream.by_ref() {
///     let chunk = chunk?;
///     for choice in &chunk.choices {
///         if let Some(text) = &choice.delta.content {
///             print!("{text}");
///         }
///     }
/// }
/// let response = stream.aggregate()?;
/// println!("{:?}", response.text());
/// # Ok::<(), llm::LlmError>(())
/// ```
pub struct ChatCompletionStream {
    reader: SseResponse,
    finished: bool,
    saw_done: bool,
    aggregator: Aggregator,
}

impl ChatCompletionStream {
    /// 包装一个 SSE 读取器（一般由 crate::openai_chat::Client 构造）。
    pub fn new(reader: SseResponse) -> Self {
        Self {
            reader,
            finished: false,
            saw_done: false,
            aggregator: Aggregator::default(),
        }
    }

    /// 是否读到了协议终止标记 [DONE]。
    pub fn saw_done(&self) -> bool {
        self.saw_done
    }

    /// 流是否已经结束。
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// 到目前为止累积到的 usage（没有则为 None）。
    pub fn usage(&self) -> Option<&Usage> {
        self.aggregator.response.usage.as_ref()
    }

    /// 读下一条 chunk；流结束（收到 [DONE] 或 EOF）返回 Ok(None)。
    pub fn next_chunk(&mut self) -> LlmResult<Option<ChatCompletionChunk>> {
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
                self.saw_done = true;
                self.finished = true;
                return Ok(None);
            }

            if event.data.trim().is_empty() {
                continue;
            }

            let value = event.json()?;

            // 兼容网关可能把错误塞进 200 的 SSE 里。
            if value.get("error").is_some() && value.get("choices").is_none() {
                self.finished = true;
                let detail = value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        let preview: String = event.data.chars().take(300).collect();
                        format!("原始事件：{preview}")
                    });
                return Err(LlmError::Sse(format!("流式响应返回错误：{detail}")));
            }

            let chunk: ChatCompletionChunk = serde_json::from_value(value)
                .map_err(|error| LlmError::Sse(format!("流式 chunk 解析失败：{error}")))?;
            self.aggregator.push(&chunk);
            return Ok(Some(chunk));
        }
    }

    /// 消费剩余事件，把它们拼成一个同步响应。
    ///
    /// 已经通过迭代器取走的 chunk 也会计入（迭代时会实时累积），所以可以先消费一部分
    /// 再聚合剩余部分。
    pub fn aggregate(mut self) -> LlmResult<ChatCompletionResponse> {
        while self.next_chunk()?.is_some() {}
        Ok(self.aggregator.finish())
    }
}

impl Iterator for ChatCompletionStream {
    type Item = LlmResult<ChatCompletionChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_chunk() {
            Ok(Some(chunk)) => Some(Ok(chunk)),
            Ok(None) => None,
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}

impl std::fmt::Debug for ChatCompletionStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChatCompletionStream")
            .field("finished", &self.finished)
            .field("saw_done", &self.saw_done)
            .finish_non_exhaustive()
    }
}

/// 聚合过程中的累积状态。
#[derive(Debug, Default)]
struct Aggregator {
    response: ChatCompletionResponse,
}

impl Aggregator {
    /// 吸收一条 chunk。
    fn push(&mut self, chunk: &ChatCompletionChunk) {
        if self.response.id.is_none() {
            self.response.id = chunk.id.clone();
        }
        if self.response.created.is_none() {
            self.response.created = chunk.created;
        }
        if self.response.model.is_none() {
            self.response.model = chunk.model.clone();
        }
        if self.response.system_fingerprint.is_none() {
            self.response.system_fingerprint = chunk.system_fingerprint.clone();
        }
        if self.response.service_tier.is_none() {
            self.response.service_tier = chunk.service_tier.clone();
        }
        if chunk.usage.is_some() {
            self.response.usage = chunk.usage.clone();
        }
        for (key, value) in &chunk.extra {
            self.response
                .extra
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }

        for chunk_choice in &chunk.choices {
            let index = chunk_choice.index;
            let existing = self
                .response
                .choices
                .iter()
                .position(|choice| choice.index == index);
            let position = match existing {
                Some(position) => position,
                None => {
                    self.response.choices.push(ChatChoice {
                        index,
                        ..ChatChoice::default()
                    });
                    self.response.choices.len() - 1
                }
            };

            let choice = &mut self.response.choices[position];
            merge_delta(&mut choice.message, &chunk_choice.delta);
            if chunk_choice.finish_reason.is_some() {
                choice.finish_reason = chunk_choice.finish_reason.clone();
            }
            if chunk_choice.logprobs.is_some() {
                choice.logprobs = chunk_choice.logprobs.clone();
            }
            for (key, value) in &chunk_choice.extra {
                choice
                    .extra
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }

    /// 收尾：补对象类型、排序候选、补默认角色。
    fn finish(mut self) -> ChatCompletionResponse {
        self.response.object = Some("chat.completion".to_owned());
        self.response.choices.sort_by_key(|choice| choice.index);
        for choice in &mut self.response.choices {
            if choice.message.role.is_empty() {
                choice.message.role = "assistant".to_owned();
            }
        }
        self.response
    }
}

/// 把一块增量合并进累积消息。
fn merge_delta(message: &mut ChatMessage, delta: &Delta) {
    if message.role.is_empty()
        && let Some(role) = &delta.role
    {
        message.role = role.clone();
    }
    append_content(&mut message.content, &delta.content);
    append_text(&mut message.refusal, &delta.refusal);
    append_text(&mut message.reasoning_content, &delta.reasoning_content);
    if let Some(audio) = &delta.audio {
        message.audio = Some(audio.clone());
    }
    if let Some(function) = &delta.function_call {
        merge_function(
            message
                .function_call
                .get_or_insert_with(FunctionCall::default),
            function,
        );
    }
    if let Some(tool_calls) = &delta.tool_calls {
        merge_tool_calls(message, tool_calls);
    }
    for (key, value) in &delta.extra {
        message
            .extra
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

/// 拼接文本增量。
fn append_text(target: &mut Option<String>, delta: &Option<String>) {
    if let Some(fragment) = delta {
        target
            .get_or_insert_with(String::new)
            .push_str(fragment.as_str());
    }
}

/// 拼接消息内容增量。
fn append_content(target: &mut Option<MessageContent>, delta: &Option<String>) {
    let Some(fragment) = delta else {
        return;
    };
    match target {
        Some(MessageContent::Text(text)) => text.push_str(fragment),
        Some(MessageContent::Parts(parts)) => {
            parts.push(ContentPart::text(fragment.clone()));
        }
        None => *target = Some(MessageContent::Text(fragment.clone())),
    }
}

/// 合并函数调用增量。
fn merge_function(target: &mut FunctionCall, delta: &FunctionCall) {
    if let Some(name) = &delta.name {
        target.name = Some(name.clone());
    }
    append_text(&mut target.arguments, &delta.arguments);
    for (key, value) in &delta.extra {
        target
            .extra
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

/// 按 index 合并工具调用增量。
fn merge_tool_calls(message: &mut ChatMessage, deltas: &[ToolCall]) {
    let calls = message.tool_calls.get_or_insert_with(Vec::new);
    for delta in deltas {
        let position = delta
            .index
            .map(|index| index as usize)
            .unwrap_or(calls.len());
        while calls.len() <= position {
            calls.push(ToolCall::default());
        }
        let target = &mut calls[position];
        if let Some(id) = &delta.id {
            target.id = Some(id.clone());
        }
        if let Some(call_type) = &delta.call_type {
            target.call_type = Some(call_type.clone());
        }
        if let Some(function) = &delta.function {
            merge_function(
                target.function.get_or_insert_with(FunctionCall::default),
                function,
            );
        }
        for (key, value) in &delta.extra {
            target
                .extra
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }
}
