//! 统一门面：`LlmClient` 把三个协议收在同一处，同时提供「原始 JSON 直通」入口。
//!
//! 两种调用方式：
//! - **类型化**：`client.openai_chat().create(&request)` / `openai_responses()` / `anthropic_messages()`；
//! - **JSON 直通**：`client.request_json(Protocol::OpenAiChat, &body)`，用于工具 / 服务 / 未覆盖字段。
//!
//! 插件层（`crate::plugin`）只依赖第二种，所以协议模块可以独立演进。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::{AuthMode, ClientConfig, LlmResult, SseEvent, SseResponse, Transport};

/// 三个受支持的协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    /// OpenAI Chat Completions（`POST /v1/chat/completions`）。
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    /// OpenAI Responses（`POST /v1/responses`）。
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// Anthropic Messages（`POST /v1/messages`）。
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
}

impl Protocol {
    /// 全部协议，按固定顺序。
    pub const ALL: [Self; 3] = [
        Self::OpenAiChat,
        Self::OpenAiResponses,
        Self::AnthropicMessages,
    ];

    /// 稳定的字符串 id（工具 / 服务 / 配置里用）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai-chat",
            Self::OpenAiResponses => "openai-responses",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }

    /// 给人看的名字。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpenAiChat => "OpenAI Chat Completions",
            Self::OpenAiResponses => "OpenAI Responses",
            Self::AnthropicMessages => "Anthropic Messages",
        }
    }

    /// 该协议的认证方式。
    pub fn auth(self) -> AuthMode {
        match self {
            Self::OpenAiChat | Self::OpenAiResponses => AuthMode::Bearer,
            Self::AnthropicMessages => AuthMode::Anthropic,
        }
    }

    /// 宽松解析：接受 id、别名与大小写变体。
    pub fn parse(text: &str) -> Option<Self> {
        let normalized = text.trim().to_ascii_lowercase().replace('_', "-");
        match normalized.as_str() {
            "openai-chat" | "openai-chat-completions" | "chat" | "chat-completions" | "openai" => {
                Some(Self::OpenAiChat)
            }
            "openai-responses" | "responses" | "response" => Some(Self::OpenAiResponses),
            "anthropic-messages" | "anthropic" | "messages" | "message" | "claude" => {
                Some(Self::AnthropicMessages)
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 原始 SSE 流：只把每条事件的 `data` 解析成 JSON，解释交给调用方。
pub struct RawStream {
    protocol: Protocol,
    reader: SseResponse,
    finished: bool,
}

impl RawStream {
    /// 所属协议。
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// 读下一条 SSE 事件；结束返回 `None`。
    pub fn next_event(&mut self) -> LlmResult<Option<SseEvent>> {
        if self.finished {
            return Ok(None);
        }
        match self.reader.next_event() {
            Ok(Some(event)) => {
                if event.is_done() {
                    self.finished = true;
                    return Ok(None);
                }
                Ok(Some(event))
            }
            Ok(None) => {
                self.finished = true;
                Ok(None)
            }
            Err(error) => {
                self.finished = true;
                Err(error)
            }
        }
    }

    /// 读下一条事件的 JSON 数据；空数据事件被跳过。
    pub fn next_json(&mut self) -> LlmResult<Option<Value>> {
        loop {
            let Some(event) = self.next_event()? else {
                return Ok(None);
            };
            if event.data.trim().is_empty() {
                continue;
            }
            return event.json().map(Some);
        }
    }
}

impl Iterator for RawStream {
    type Item = LlmResult<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_json() {
            Ok(Some(value)) => Some(Ok(value)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

/// 单协议测试结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolTest {
    /// 协议 id。
    pub protocol: String,
    /// 是否成功。
    pub ok: bool,
    /// 往返耗时（毫秒）。
    pub latency_ms: u64,
    /// 上游 HTTP 状态（成功时通常 200；本地错误为 `None`）。
    pub status: Option<u16>,
    /// 模型回答的文本（截断到 200 字符）。
    pub text: Option<String>,
    /// 失败原因。
    pub error: Option<String>,
}

/// `LlmClient::test` 的汇总结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestReport {
    /// 三个协议是否全部成功。
    pub ok: bool,
    /// 测试使用的接入地址。
    pub base_url: String,
    /// 测试使用的模型。
    pub model: String,
    /// 每个协议的结果。
    pub protocols: Vec<ProtocolTest>,
}

impl TestReport {
    /// 转成 JSON（工具 / 服务出参）。
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "ok": self.ok }))
    }
}

/// 协议统一客户端。
#[derive(Clone)]
pub struct LlmClient {
    transport: Transport,
}

impl LlmClient {
    /// 用显式配置构造。
    pub fn new(config: ClientConfig) -> LlmResult<Self> {
        Ok(Self {
            transport: Transport::new(config)?,
        })
    }

    /// 用环境变量构造（见 [`ClientConfig::from_env`]）。
    pub fn from_env() -> LlmResult<Self> {
        Self::new(ClientConfig::from_env())
    }

    /// 读取底层配置。
    pub fn config(&self) -> &ClientConfig {
        self.transport.config()
    }

    /// 读取底层传输器。
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// OpenAI Chat Completions 类型化客户端。
    pub fn openai_chat(&self) -> crate::openai_chat::Client {
        crate::openai_chat::Client::new(self.transport.clone())
    }

    /// OpenAI Responses 类型化客户端。
    pub fn openai_responses(&self) -> crate::openai_responses::Client {
        crate::openai_responses::Client::new(self.transport.clone())
    }

    /// Anthropic Messages 类型化客户端。
    pub fn anthropic_messages(&self) -> crate::anthropic_messages::Client {
        crate::anthropic_messages::Client::new(self.transport.clone())
    }

    /// 协议对应的路径（来自配置）。
    pub fn path_for(&self, protocol: Protocol) -> &str {
        match protocol {
            Protocol::OpenAiChat => &self.config().chat_completions_path,
            Protocol::OpenAiResponses => &self.config().responses_path,
            Protocol::AnthropicMessages => &self.config().messages_path,
        }
    }

    /// 绝对 URL（描述 / 测试用）。
    pub fn endpoint_for(&self, protocol: Protocol) -> String {
        self.config().endpoint(self.path_for(protocol))
    }

    /// 原始 JSON 直通：请求体就是对应协议的原生 body。
    pub fn request_json(&self, protocol: Protocol, body: &Value) -> LlmResult<Value> {
        self.transport
            .request_json("POST", self.path_for(protocol), protocol.auth(), Some(body))
    }

    /// 原始 JSON 流式直通。
    pub fn request_stream(&self, protocol: Protocol, body: &Value) -> LlmResult<RawStream> {
        let reader = self.transport.open_stream(
            "POST",
            self.path_for(protocol),
            protocol.auth(),
            Some(body),
        )?;
        Ok(RawStream {
            protocol,
            reader,
            finished: false,
        })
    }

    /// `GET /models`（OpenAI 风格，Bearer 认证）。
    pub fn models(&self) -> LlmResult<Value> {
        self.transport
            .request_json("GET", &self.config().models_path, AuthMode::Bearer, None)
    }

    /// 自检：用最小请求依次测试三个协议。
    ///
    /// 这个接口同时服务三件事：插件 `fengyegf.llm.test` 工具、部署后验证、CI 冒烟。
    pub fn test(&self, model: &str) -> TestReport {
        let mut protocols = Vec::with_capacity(Protocol::ALL.len());
        for protocol in Protocol::ALL {
            protocols.push(self.test_one(protocol, model));
        }
        TestReport {
            ok: protocols.iter().all(|item| item.ok),
            base_url: self.config().base_url.clone(),
            model: model.to_owned(),
            protocols,
        }
    }

    fn test_one(&self, protocol: Protocol, model: &str) -> ProtocolTest {
        let body = match protocol {
            Protocol::OpenAiChat => json!({
                "model": model,
                "messages": [{ "role": "user", "content": "Reply with exactly: pong" }],
                "max_tokens": 64,
                "temperature": 0.0,
            }),
            Protocol::OpenAiResponses => json!({
                "model": model,
                "input": "Reply with exactly: pong",
                "max_output_tokens": 64,
            }),
            Protocol::AnthropicMessages => json!({
                "model": model,
                "max_tokens": 64,
                "messages": [{ "role": "user", "content": "Reply with exactly: pong" }],
            }),
        };

        let started = Instant::now();
        match self.request_json(protocol, &body) {
            Ok(value) => ProtocolTest {
                protocol: protocol.as_str().to_owned(),
                ok: true,
                latency_ms: started.elapsed().as_millis() as u64,
                status: Some(200),
                text: extract_text(protocol, &value),
                error: None,
            },
            Err(error) => ProtocolTest {
                protocol: protocol.as_str().to_owned(),
                ok: false,
                latency_ms: started.elapsed().as_millis() as u64,
                status: error.status(),
                text: None,
                error: Some(error.to_string()),
            },
        }
    }

    /// 给 AI Agent / 面板用的自描述 JSON（不发起网络请求）。
    pub fn describe(&self) -> Value {
        let config = self.config();
        json!({
            "ok": true,
            "plugin": "fengyegf.llm",
            "base_url": config.base_url,
            "api_key": config.masked_api_key(),
            "has_api_key": config.has_api_key(),
            "protocols": Protocol::ALL
                .iter()
                .map(|protocol| json!({
                    "id": protocol.as_str(),
                    "name": protocol.display_name(),
                    "path": self.path_for(*protocol),
                    "endpoint": self.endpoint_for(*protocol),
                    "auth": match protocol.auth() {
                        AuthMode::Bearer => "bearer",
                        AuthMode::Anthropic => "x-api-key",
                    },
                    "stream": true,
                }))
                .collect::<Vec<_>>(),
            "default_headers": config.headers,
            "timeout_secs": config.timeout_secs,
            "max_retries": config.max_retries,
        })
    }
}

/// 从三种响应形状里尽量取出一段文本（自检 / 日志用）。
fn extract_text(protocol: Protocol, value: &Value) -> Option<String> {
    let text = match protocol {
        Protocol::OpenAiChat => value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                value
                    .pointer("/choices/0/message/reasoning_content")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
        Protocol::OpenAiResponses => {
            let mut collected = String::new();
            if let Some(items) = value.get("output").and_then(Value::as_array) {
                for item in items {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                if let Some(text) = part.get("text").and_then(Value::as_str) {
                                    collected.push_str(text);
                                }
                            }
                        }
                    }
                }
            }
            value
                .get("output_text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or((!collected.is_empty()).then_some(collected))
        }
        Protocol::AnthropicMessages => {
            let mut collected = String::new();
            if let Some(blocks) = value.get("content").and_then(Value::as_array) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            collected.push_str(text);
                        }
                    }
                }
            }
            (!collected.is_empty()).then_some(collected)
        }
    };
    text.map(|text| {
        let trimmed = text.trim();
        let preview: String = trimmed.chars().take(200).collect();
        if trimmed.chars().count() > 200 {
            format!("{preview}…")
        } else {
            preview
        }
    })
}

/// 把任意 `Value` 转成便于展示的 JSON 字符串。
pub fn value_to_pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}
