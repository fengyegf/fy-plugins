//! 流式服务：把上游 SSE 增量缓冲下来，调用方用 `stream_poll` 拉取。
//!
//! 为什么用轮询而不是事件：服务方法本身就是同步 JSON，事件跨插件投递还要依赖宿主
//! 的 emit 作用域语义；轮询只有一条契约（服务调用），调用方拉多久、拉多快都可控，
//! 失败也只会落到一次普通调用上。延迟用一个轮询间隔（默认 50ms）换确定性。
//!
//! 三个方法：
//! - `stream`：开流，返回 stream_id（立刻返回，读线程在后台读 SSE）；
//! - `stream_poll`：`{stream_id, cursor}` → 新块 + 是否结束 + 错误 / usage；
//! - `stream_cancel`：置取消位（用户点「停止生成」时调用）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use crate::api::Protocol;
use crate::core::LlmError;
use crate::plugin::PluginState;

/// 一个流的状态（读线程写、poll 读）。
#[derive(Default)]
pub(crate) struct StreamState {
    chunks: Vec<String>,
    done: bool,
    cancelled: bool,
    error: Option<Value>,
    model: Option<String>,
    usage: Value,
}

/// 一个流：状态 + 取消位。
struct StreamSlot {
    state: Arc<Mutex<StreamState>>,
    cancel: Arc<AtomicBool>,
}

/// 活跃流登记表。
pub struct StreamRegistry {
    next: AtomicU64,
    active: Mutex<HashMap<u64, StreamSlot>>,
}

impl Default for StreamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamRegistry {
    /// 空表。
    pub(crate) fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
            active: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, StreamSlot>> {
        self.active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// 登记一个流并返回 id。
    pub(crate) fn register(&self, cancel: Arc<AtomicBool>) -> (u64, Arc<Mutex<StreamState>>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let state = Arc::new(Mutex::new(StreamState::default()));
        self.lock().insert(
            id,
            StreamSlot {
                state: Arc::clone(&state),
                cancel,
            },
        );
        (id, state)
    }

    /// 用调用方给的 id 登记（调用方先建好轮询游标，避免第一块丢失）。
    pub(crate) fn register_at(&self, id: u64, cancel: Arc<AtomicBool>) -> Arc<Mutex<StreamState>> {
        let state = Arc::new(Mutex::new(StreamState::default()));
        self.lock().insert(
            id,
            StreamSlot {
                state: Arc::clone(&state),
                cancel,
            },
        );
        state
    }

    /// 取消一个流；返回是否找到。
    pub(crate) fn cancel(&self, id: u64) -> bool {
        let guard = self.lock();
        match guard.get(&id) {
            Some(slot) => {
                slot.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// 移除（流结束后调用）。
    pub(crate) fn remove(&self, id: u64) {
        self.lock().remove(&id);
    }

    /// 插件停止时取消全部。
    pub(crate) fn cancel_all(&self) {
        for slot in self.lock().values() {
            slot.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// `stream` 方法。
pub fn start(state: &PluginState, input: &Value) -> Value {
    let protocol = match input.get("protocol").and_then(Value::as_str) {
        Some(text) => match Protocol::parse(text) {
            Some(protocol) => protocol,
            None => return LlmError::Other(format!("未知协议：{text}")).to_json(),
        },
        None => Protocol::OpenAiChat,
    };

    let mut body = input.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("protocol");
        object.remove("stream_id");
    }
    // prepare 是同步调用的公共校验（会拒绝 stream:true）：先补默认字段，再打开 stream。
    let mut prepared = match state.prepare(protocol, &body) {
        Ok(value) => value,
        Err(error) => return error.to_json(),
    };
    if let Some(object) = prepared.as_object_mut() {
        object.insert("stream".to_owned(), Value::Bool(true));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let (id, stream_state) = match input
        .get("stream_id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
    {
        Some(id) => (id, state.streams.register_at(id, Arc::clone(&cancel))),
        None => state.streams.register(Arc::clone(&cancel)),
    };

    let client = state.client.clone();
    let host = state.host;
    let spawned = std::thread::Builder::new()
        .name(format!("llm-stream-{id}"))
        .spawn(move || {
            pump(host, client, protocol, prepared, id, cancel, stream_state);
        });
    if let Err(error) = spawned {
        state.streams.remove(id);
        return LlmError::Other(format!("起流式线程失败：{error}")).to_json();
    }

    json!({ "ok": true, "stream_id": id, "protocol": protocol.as_str() })
}

/// `stream_poll` 方法：返回 `cursor` 之后的新块。
pub fn poll(state: &PluginState, input: &Value) -> Value {
    let Some(id) = input.get("stream_id").and_then(Value::as_u64) else {
        return LlmError::Other("缺少 stream_id".to_owned()).to_json();
    };
    let cursor = input.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize;

    let slot_state = match state.streams.lock().get(&id) {
        Some(slot) => Arc::clone(&slot.state),
        None => {
            return json!({
                "ok": false,
                "kind": "stream",
                "message": format!("未知的 stream_id：{id}"),
            });
        }
    };
    let snapshot = {
        let guard = slot_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let start = cursor.min(guard.chunks.len());
        json!({
            "ok": true,
            "stream_id": id,
            "chunks": guard.chunks[start..].to_vec(),
            "cursor": guard.chunks.len(),
            "done": guard.done,
            "cancelled": guard.cancelled,
            "error": guard.error,
            "model": guard.model,
            "usage": guard.usage,
        })
    };
    // 读完且已结束：移除登记，避免流表无限增长。
    let finished = snapshot.get("done").and_then(Value::as_bool) == Some(true)
        && snapshot
            .get("cursor")
            .and_then(Value::as_u64)
            .is_some_and(|end| {
                end as usize
                    <= snapshot
                        .get("chunks")
                        .and_then(Value::as_array)
                        .map(|c| c.len())
                        .unwrap_or(0)
                        + cursor
            });
    if finished {
        state.streams.remove(id);
    }
    snapshot
}

/// `stream_cancel` 方法。
pub fn cancel(state: &PluginState, input: &Value) -> Value {
    let Some(id) = input.get("stream_id").and_then(Value::as_u64) else {
        return LlmError::Other("缺少 stream_id".to_owned()).to_json();
    };
    json!({ "ok": true, "stream_id": id, "cancelled": state.streams.cancel(id) })
}

/// 读线程：开流、逐事件把文本推进缓冲区。
fn pump(
    _host: HostApi,
    client: crate::api::LlmClient,
    protocol: Protocol,
    body: Value,
    _id: u64,
    cancel: Arc<AtomicBool>,
    stream_state: Arc<Mutex<StreamState>>,
) {
    let mut stream = match client.request_stream(protocol, &body) {
        Ok(stream) => stream,
        Err(error) => {
            finish_with_error(&stream_state, &error);
            return;
        }
    };

    loop {
        if cancel.load(Ordering::SeqCst) {
            let mut guard = stream_state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            guard.cancelled = true;
            guard.done = true;
            return;
        }
        match stream.next_json() {
            Ok(Some(event)) => {
                if let Some(text) = extract_delta(protocol, &event)
                    && !text.is_empty()
                {
                    let mut guard = stream_state
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    guard.chunks.push(text);
                }
                if let Some(usage) = extract_usage(protocol, &event) {
                    stream_state
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .usage = usage;
                }
                if let Some(model) = extract_model(protocol, &event) {
                    let mut guard = stream_state
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    if guard.model.is_none() {
                        guard.model = Some(model);
                    }
                }
            }
            Ok(None) => {
                stream_state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .done = true;
                return;
            }
            Err(error) => {
                finish_with_error(&stream_state, &error);
                return;
            }
        }
    }
}

/// 把错误写进流状态并标结束。
fn finish_with_error(stream_state: &Arc<Mutex<StreamState>>, error: &LlmError) {
    let mut guard = stream_state
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    guard.error = Some(json!({ "code": error_code(error), "message": error.to_string() }));
    guard.done = true;
}

/// 稳定错误代号。
fn error_code(error: &LlmError) -> &'static str {
    match error {
        LlmError::Config(_) => "config",
        LlmError::Transport(_) => "transport",
        LlmError::Api { .. } => "api",
        LlmError::Sse(_) => "sse",
        LlmError::Json(_) => "json",
        LlmError::UnsupportedProtocol(_) => "protocol",
        LlmError::Other(_) => "other",
    }
}

/// 从一条 SSE 事件里抽「新增文本」。
pub(crate) fn extract_delta(protocol: Protocol, event: &Value) -> Option<String> {
    match protocol {
        Protocol::OpenAiChat => {
            let content = event.pointer("/choices/0/delta/content")?;
            content_to_string(content)
        }
        Protocol::OpenAiResponses => {
            if event.get("type").and_then(Value::as_str) == Some("response.output_text.delta") {
                return event
                    .get("delta")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
            }
            event
                .get("delta")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        }
        Protocol::AnthropicMessages => {
            if event.get("type").and_then(Value::as_str) == Some("content_block_delta") {
                return event
                    .pointer("/delta/text")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
            }
            None
        }
    }
}

/// content 可能是字符串，也可能是 [{type:"text",text:"…"}] 分片。
fn content_to_string(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// 抽 usage。
fn extract_usage(protocol: Protocol, event: &Value) -> Option<Value> {
    let usage = match protocol {
        Protocol::OpenAiChat => event.get("usage"),
        Protocol::OpenAiResponses => event
            .pointer("/response/usage")
            .or_else(|| event.get("usage")),
        Protocol::AnthropicMessages => event
            .get("usage")
            .or_else(|| event.pointer("/message/usage")),
    }?;
    (!usage.is_null()).then(|| usage.clone())
}

/// 抽 model。
fn extract_model(protocol: Protocol, event: &Value) -> Option<String> {
    let model = match protocol {
        Protocol::OpenAiChat => event.get("model"),
        Protocol::OpenAiResponses => event
            .pointer("/response/model")
            .or_else(|| event.get("model")),
        Protocol::AnthropicMessages => event
            .pointer("/message/model")
            .or_else(|| event.get("model")),
    }?;
    model.as_str().map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_delta_string_and_parts() {
        let event = json!({"choices":[{"delta":{"content":"你好"}}]});
        assert_eq!(
            extract_delta(Protocol::OpenAiChat, &event).as_deref(),
            Some("你好")
        );
        let parts = json!({"choices":[{"delta":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]});
        assert_eq!(
            extract_delta(Protocol::OpenAiChat, &parts).as_deref(),
            Some("ab")
        );
        assert_eq!(
            extract_delta(
                Protocol::OpenAiChat,
                &json!({"choices":[{"delta":{"content":null}}]})
            ),
            None
        );
    }

    #[test]
    fn responses_delta_and_usage() {
        let event = json!({"type":"response.output_text.delta","delta":"hi"});
        assert_eq!(
            extract_delta(Protocol::OpenAiResponses, &event).as_deref(),
            Some("hi")
        );
        let done = json!({"type":"response.completed","response":{"model":"m","usage":{"total_tokens":3}}});
        assert_eq!(
            extract_usage(Protocol::OpenAiResponses, &done).unwrap()["total_tokens"],
            3
        );
        assert_eq!(
            extract_model(Protocol::OpenAiResponses, &done).as_deref(),
            Some("m")
        );
    }

    #[test]
    fn anthropic_content_block_delta() {
        let event =
            json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"hey"}});
        assert_eq!(
            extract_delta(Protocol::AnthropicMessages, &event).as_deref(),
            Some("hey")
        );
        assert_eq!(
            extract_delta(
                Protocol::AnthropicMessages,
                &json!({"type":"message_start"})
            ),
            None
        );
    }

    #[test]
    fn registry_ids_and_cancel() {
        let registry = StreamRegistry::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let (first, _state) = registry.register(Arc::clone(&cancel));
        let (second, _state2) = registry.register(Arc::new(AtomicBool::new(false)));
        assert!(first > 0 && second > first);
        assert!(registry.cancel(first));
        assert!(cancel.load(Ordering::SeqCst));
        assert!(!registry.cancel(999));
        registry.remove(first);
        assert!(!registry.cancel(first));
    }
}
