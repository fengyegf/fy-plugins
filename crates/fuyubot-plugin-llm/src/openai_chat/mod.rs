//! OpenAI Chat Completions 协议客户端。
//!
//! 端点：POST {base_url}/chat/completions（路径取 ClientConfig.chat_completions_path），
//! 认证：Authorization: Bearer <api_key>（另可带 OpenAI-Organization / OpenAI-Project 头，
//! 由传输层统一加）。
//!
//! 类型化调用：
//!
//! ```no_run
//! use llm::{ClientConfig, Transport};
//! use llm::openai_chat::{ChatCompletionRequest, ChatMessage, Client};
//!
//! let transport = Transport::new(ClientConfig::new("https://api.openai.com/v1", "sk-test"))?;
//! let client = Client::new(transport);
//! let request = ChatCompletionRequest::new("gpt-4o-mini")
//!     .with_messages(vec![ChatMessage::user("hello")]);
//! let response = client.create(&request)?;
//! println!("{:?}", response.text());
//! # Ok::<(), llm::LlmError>(())
//! ```
//!
//! JSON 直通（工具 / 未覆盖字段 / 转发）：
//!
//! ```no_run
//! # use llm::{ClientConfig, Transport};
//! # use llm::openai_chat::Client;
//! use serde_json::json;
//!
//! # let transport = Transport::new(ClientConfig::new("https://api.openai.com/v1", "sk-test"))?;
//! # let client = Client::new(transport);
//! let body = json!({
//!     "model": "gpt-4o-mini",
//!     "messages": [{ "role": "user", "content": "hello" }]
//! });
//! let value = client.raw(&body)?;
//! println!("{value}");
//! # Ok::<(), llm::LlmError>(())
//! ```
//!
//! 所有方法都是同步阻塞调用；错误统一走 crate::core::LlmResult，不会 panic。

mod stream;
mod types;

pub use stream::*;
pub use types::*;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::core::{AuthMode, LlmError, LlmResult, Transport};

/// 默认的 Chat Completions 相对路径；实际路径以 ClientConfig.chat_completions_path 为准。
pub const DEFAULT_CHAT_COMPLETIONS_PATH: &str = "/chat/completions";

/// OpenAI Chat Completions 协议客户端。
#[derive(Clone)]
pub struct Client {
    transport: Transport,
}

impl Client {
    /// 用共享传输器构造。
    pub fn new(transport: Transport) -> Self {
        Self { transport }
    }

    /// 读取底层传输器（高级用法）。
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// 本客户端实际使用的相对路径。
    pub fn path(&self) -> &str {
        &self.transport.config().chat_completions_path
    }

    /// 完整端点 URL（不含查询参数），用于日志 / 描述。
    pub fn endpoint(&self) -> String {
        self.transport.config().endpoint(self.path())
    }

    /// 同步调用：POST /chat/completions，返回完整响应。
    pub fn create(&self, request: &ChatCompletionRequest) -> LlmResult<ChatCompletionResponse> {
        let body = encode(request)?;
        let value =
            self.transport
                .request_json("POST", self.path(), AuthMode::Bearer, Some(&body))?;
        decode(value, "Chat Completions 响应")
    }

    /// 流式调用：把请求里的 stream 置为 true 后打开 SSE 流。
    ///
    /// 需要末尾的 usage chunk 时，请自行设置
    /// request.stream_options.include_usage = Some(true)；
    /// 协议本身并不强制要求该选项，所以这里不改写调用方的 stream_options。
    pub fn create_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> LlmResult<ChatCompletionStream> {
        let body = encode(&request.clone().streaming())?;
        self.open_stream_body(&body)
    }

    /// 原始 JSON 直通：请求体就是该协议的原生 body。
    pub fn raw(&self, body: &Value) -> LlmResult<Value> {
        self.transport
            .request_json("POST", self.path(), AuthMode::Bearer, Some(body))
    }

    /// 原始 JSON 流式直通：会强制把 body 的 stream 置为 true（否则服务端返回非 SSE）。
    pub fn raw_stream(&self, body: &Value) -> LlmResult<ChatCompletionStream> {
        let mut body = body.clone();
        match body.as_object_mut() {
            Some(object) => {
                object.insert("stream".to_owned(), Value::Bool(true));
            }
            None => {
                return Err(LlmError::Other(
                    "raw_stream 的请求体必须是 JSON 对象".to_owned(),
                ));
            }
        }
        self.open_stream_body(&body)
    }

    fn open_stream_body(&self, body: &Value) -> LlmResult<ChatCompletionStream> {
        let reader =
            self.transport
                .open_stream("POST", self.path(), AuthMode::Bearer, Some(body))?;
        Ok(ChatCompletionStream::new(reader))
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("endpoint", &self.endpoint())
            .finish_non_exhaustive()
    }
}

/// 请求体序列化。
fn encode(request: &ChatCompletionRequest) -> LlmResult<Value> {
    serde_json::to_value(request)
        .map_err(|error| LlmError::Json(format!("请求体序列化失败：{error}")))
}

/// 响应反序列化。
fn decode<T: DeserializeOwned>(value: Value, what: &str) -> LlmResult<T> {
    serde_json::from_value(value)
        .map_err(|error| LlmError::Json(format!("{what}解析失败：{error}")))
}
