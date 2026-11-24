//! LLM 网关客户端：把一轮对话翻译成 `fengyegf.llm` 共享服务的一次调用。
//!
//! 为什么不自己发 HTTP：网关插件已经把三个协议、鉴权、超时、重试收口了，这边只要
//! 认 JSON 协议。网关是**硬依赖**（`Plugin::inject`）：没装 `fengyegf.llm` 时本插件
//! 不会被启动；运行期 `available` 仍每次现查宿主，用于诊断与状态展示，不缓存。
//!
//! 服务调用是**同步阻塞**的（宿主在本线程直接转交给提供方的方法），只能在 worker
//! 线程或工具 / 服务处理器里调；宿主回调里调会卡住整个接收循环。

use fuyubot_plugin_api::HostApi;
use serde_json::{Map, Value};

use crate::core::{AgentError, AgentResult, Config};

use super::completion::{Completion, ProtocolKind, protocol_kind};

/// LLM 网关客户端（走共享服务，不自己发 HTTP）。
pub struct GatewayClient {
    host: HostApi,
    service: String,
    method: String,
    /// 插件配置里的默认模型；请求没指定时下发，空串 = 交给网关的默认值。
    model: String,
}

impl GatewayClient {
    /// 按配置建客户端。
    pub fn new(host: HostApi, config: &Config) -> Self {
        Self {
            host,
            service: crate::core::consts::GATEWAY_SERVICE.to_owned(),
            method: config.gateway_method.clone(),
            model: config.model.clone(),
        }
    }

    /// 服务名（日志用）。
    pub fn service(&self) -> &str {
        &self.service
    }

    /// 网关方法名（`chat` / `messages` / `responses`，日志与 describe 用）。
    pub fn method(&self) -> &str {
        &self.method
    }

    /// 网关服务当前是否在场。
    ///
    /// 每次现查 `list_services`（JSON 数组文本）：诊断 / 状态展示用，解析失败按不可用处理。
    pub fn available(&self) -> bool {
        has_service(&self.host.list_services(), &self.service)
    }

    /// 网关是否提供流式方法（服务清单里 methods 含 "stream"）。
    ///
    /// 和 available 一样每次现查：老版本网关没有这个方法，新版本热更新后就该立刻用上。
    pub fn supports_stream(&self) -> bool {
        has_stream_method(&self.host.list_services(), &self.service)
    }

    /// 开一个网关流：返回流 id；增量从事件 fengyegf.llm.stream 推回来。
    ///
    /// stream_id 由调用方在 body 里指定（先注册路由再开流，避免丢第一块）。
    pub fn stream_chat(&self, body: Value) -> AgentResult<u64> {
        let raw = self
            .host
            .call_service(&self.service, "stream", &body.to_string())
            .map_err(|error| {
                AgentError::Gateway(format!("调用网关 {}.stream 失败：{error}", self.service))
            })?;
        let response: Value = serde_json::from_str(&raw).map_err(|error| {
            AgentError::Gateway(format!(
                "网关 {}.stream 的响应不是 JSON：{error}",
                self.service
            ))
        })?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(gateway_failed(&self.service, "stream", &response));
        }
        response
            .get("stream_id")
            .and_then(Value::as_u64)
            .filter(|id| *id > 0)
            .ok_or_else(|| {
                AgentError::Gateway(format!("网关 {}.stream 回包缺少 stream_id", self.service))
            })
    }

    /// 轮询一个网关流：返回 `cursor` 之后的新块与状态。
    pub fn poll_stream(&self, stream_id: u64, cursor: u64) -> AgentResult<StreamPoll> {
        let payload = serde_json::json!({ "stream_id": stream_id, "cursor": cursor }).to_string();
        let raw = self
            .host
            .call_service(&self.service, "stream_poll", &payload)
            .map_err(|error| {
                AgentError::Gateway(format!(
                    "调用网关 {}.stream_poll 失败：{error}",
                    self.service
                ))
            })?;
        let response: Value = serde_json::from_str(&raw).map_err(|error| {
            AgentError::Gateway(format!(
                "网关 {}.stream_poll 的响应不是 JSON：{error}",
                self.service
            ))
        })?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(gateway_failed(&self.service, "stream_poll", &response));
        }
        let chunks = response
            .get("chunks")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let error = response
            .get("error")
            .filter(|error| !error.is_null())
            .map(|error| {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("上游未给出原因");
                format!("{code}：{message}")
            });
        Ok(StreamPoll {
            chunks,
            cursor: response
                .get("cursor")
                .and_then(Value::as_u64)
                .unwrap_or(cursor),
            done: response
                .get("done")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cancelled: response
                .get("cancelled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            error,
            model: response
                .get("model")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            usage: response.get("usage").cloned().unwrap_or(Value::Null),
        })
    }

    /// 取消一个网关流（用户点「停止生成」时调用）。
    pub fn cancel_stream(&self, stream_id: u64) -> AgentResult<()> {
        let payload = serde_json::json!({ "stream_id": stream_id }).to_string();
        self.host
            .call_service(&self.service, "stream_cancel", &payload)
            .map(|_| ())
            .map_err(|error| AgentError::Gateway(format!("取消网关流 {stream_id} 失败：{error}")))
    }

    /// 原始调用：把协议原生 body 交给网关。
    ///
    /// 响应契约（见 `fengyegf.llm`）：
    /// 成功 `{"ok":true,"protocol":"openai-chat","data":{…上游原始响应…}}`；
    /// 失败 `{"ok":false,"kind":"api","status":401,"message":"…"}`。
    /// `ok` 不是 `true`、缺 `data`、响应不是 JSON、服务调用本身失败——一律
    /// `AgentError::Gateway`，并把 code / status / 上游 message 带上，方便日志定位。
    pub fn complete(&self, body: Value) -> AgentResult<Completion> {
        let payload = body.to_string();
        let raw = self
            .host
            .call_service(&self.service, &self.method, &payload)
            .map_err(|error| {
                AgentError::Gateway(format!(
                    "调用网关 {}.{} 失败：{error}",
                    self.service, self.method
                ))
            })?;
        let response: Value = serde_json::from_str(&raw).map_err(|error| {
            AgentError::Gateway(format!(
                "网关 {}.{} 的响应不是 JSON：{error}",
                self.service, self.method
            ))
        })?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(gateway_failed(&self.service, &self.method, &response));
        }
        let Some(data) = response.get("data") else {
            return Err(AgentError::Gateway(format!(
                "网关 {}.{} 的响应缺少 data 字段",
                self.service, self.method
            )));
        };
        // 网关回什么协议就按什么协议解析；没回就按我们调的方法名认。
        let protocol = response
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or(self.method.as_str());
        Ok(Completion::from_data(data, protocol))
    }

    /// 组 OpenAI Chat Completions 语义的 messages 并调用。
    ///
    /// body 形状按 `gateway_method` 变（见 `build_body`）；model 优先级：
    /// 入参 → 配置 `model` → 不下发（网关有默认模型）。
    /// `tools` 是**已经按协议渲染好**的工具定义（见 `agent::tools`），`None` 表示
    /// 这一轮不带工具（工具轮次耗尽时用）。
    pub fn chat(
        &self,
        messages: Vec<Value>,
        model: Option<&str>,
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        tools: Option<&Value>,
    ) -> AgentResult<Completion> {
        self.complete(self.chat_body(messages, model, max_tokens, temperature, tools))
    }

    /// 组请求体（同步 chat 与流式 stream 共用）。
    pub fn chat_body(
        &self,
        messages: Vec<Value>,
        model: Option<&str>,
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        tools: Option<&Value>,
    ) -> Value {
        let model = model
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .or_else(|| Some(self.model.as_str()).filter(|model| !model.trim().is_empty()));
        build_body(
            protocol_kind(&self.method),
            messages,
            model,
            max_tokens,
            temperature,
            tools,
        )
    }

    /// 这套网关方法对应的协议形状（工具定义 / 调用解析都按它分支）。
    pub(crate) fn protocol(&self) -> ProtocolKind {
        protocol_kind(&self.method)
    }
}

/// 一次流式轮询的结果。
#[derive(Debug, Clone, Default)]
pub struct StreamPoll {
    /// 新增的文本块（按顺序拼接即可）。
    pub chunks: Vec<String>,
    /// 下一次轮询要传的游标。
    pub cursor: u64,
    /// 上游是否结束。
    pub done: bool,
    /// 是否被取消。
    pub cancelled: bool,
    /// 中途错误（`code：message`）。
    pub error: Option<String>,
    pub model: Option<String>,
    pub usage: Value,
}

/// `list_services` 的 JSON 里有没有这个服务名。抽成纯函数：单测不必造 HostApi。
fn has_service(catalog: &str, service: &str) -> bool {
    let Ok(catalog) = serde_json::from_str::<Value>(catalog) else {
        return false;
    };
    let Some(items) = catalog.as_array() else {
        return false;
    };
    items
        .iter()
        .any(|item| item.get("name").and_then(Value::as_str) == Some(service))
}

/// 服务清单里这个服务是否提供了 stream 方法。
fn has_stream_method(catalog: &str, service: &str) -> bool {
    let Ok(catalog) = serde_json::from_str::<Value>(catalog) else {
        return false;
    };
    let Some(items) = catalog.as_array() else {
        return false;
    };
    items.iter().any(|item| {
        item.get("name").and_then(Value::as_str) == Some(service)
            && item
                .get("methods")
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|method| method.as_str() == Some("stream"))
                })
    })
}

/// 把网关的失败回包翻成带上下文的错误。
fn gateway_failed(service: &str, method: &str, response: &Value) -> AgentError {
    let field = |key: &str| {
        response.get(key).and_then(|field| match field {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
    };
    let kind = field("kind")
        .or_else(|| field("type"))
        .unwrap_or_else(|| "unknown".to_owned());
    let mut detail = format!("kind={kind}");
    if let Some(status) = field("status") {
        detail.push_str(&format!(", status={status}"));
    }
    if let Some(code) = field("code") {
        detail.push_str(&format!(", code={code}"));
    }
    let message = field("message")
        .or_else(|| field("error"))
        .unwrap_or_else(|| "上游未给出原因".to_owned());
    AgentError::Gateway(format!(
        "网关 {service}.{method} 返回失败（{detail}）：{message}"
    ))
}

/// 按协议组请求体。
///
/// - chat：`{"model"?,"messages","max_tokens"?,"temperature"?,"tools"?,"tool_choice"?}`
/// - messages：`{"model"?,"messages","max_tokens"?,"temperature"?,"system"?,"tools"?}`，
///   其中 system 角色的消息要提到顶层（Anthropic 不接受 messages 里的 system）；
/// - responses：`{"model"?,"input":[…],"max_output_tokens"?,"tools"?,"tool_choice"?}`
///   （不支持 temperature）。
///
/// `tools` 由 `agent::tools::ToolCatalog::definitions` 渲染，形状已经按协议分好；
/// 这里只负责放到正确的键上（Anthropic 默认就是 auto，所以不再下发 tool_choice）。
fn build_body(
    protocol: ProtocolKind,
    messages: Vec<Value>,
    model: Option<&str>,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
    tools: Option<&Value>,
) -> Value {
    let mut body = Map::new();
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        body.insert("model".to_owned(), Value::from(model));
    }
    match protocol {
        ProtocolKind::OpenAiChat => {
            body.insert("messages".to_owned(), Value::Array(messages));
            if let Some(tools) = tools {
                body.insert("tools".to_owned(), tools.clone());
                body.insert("tool_choice".to_owned(), Value::from("auto"));
            }
            if let Some(max_tokens) = max_tokens {
                body.insert("max_tokens".to_owned(), Value::from(max_tokens));
            }
            if let Some(temperature) = temperature {
                body.insert("temperature".to_owned(), Value::from(temperature));
            }
        }
        ProtocolKind::OpenAiResponses => {
            body.insert("input".to_owned(), Value::Array(messages));
            if let Some(tools) = tools {
                body.insert("tools".to_owned(), tools.clone());
                body.insert("tool_choice".to_owned(), Value::from("auto"));
            }
            if let Some(max_tokens) = max_tokens {
                body.insert("max_output_tokens".to_owned(), Value::from(max_tokens));
            }
        }
        ProtocolKind::AnthropicMessages => {
            let mut system: Vec<String> = Vec::new();
            let mut turns: Vec<Value> = Vec::with_capacity(messages.len());
            for message in messages {
                if message.get("role").and_then(Value::as_str) == Some("system") {
                    if let Some(content) = message.get("content") {
                        let text = match content {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        if !text.trim().is_empty() {
                            system.push(text);
                        }
                    }
                    continue;
                }
                turns.push(message);
            }
            body.insert("messages".to_owned(), Value::Array(turns));
            if !system.is_empty() {
                body.insert("system".to_owned(), Value::String(system.join("\n")));
            }
            if let Some(tools) = tools {
                body.insert("tools".to_owned(), tools.clone());
            }
            if let Some(max_tokens) = max_tokens {
                body.insert("max_tokens".to_owned(), Value::from(max_tokens));
            }
            if let Some(temperature) = temperature {
                body.insert("temperature".to_owned(), Value::from(temperature));
            }
        }
    }
    Value::Object(body)
}
