//! Anthropic Messages 协议客户端。
//!
//! # 端点
//!
//! - `POST {base_url}/messages`：创建消息（同步 / 流式）
//! - `POST {base_url}/messages/count_tokens`：统计 token（[`Client::count_tokens`]）
//!
//! 认证由共享层 `crate::core::Transport` 的 `AuthMode::Anthropic` 处理：
//! `x-api-key` + `anthropic-version`（可选 `anthropic-beta`）。
//!
//! # 文件
//!
//! - `types.rs`：请求 / 响应 / 内容块 / 工具 / 用量等 serde 类型
//! - `stream.rs`：SSE 事件类型、[`MessageStream`] 迭代器、聚合
//!
//! # 示例（同步）
//!
//! ```text
//! let client = Client::new(transport);
//! let request = MessageRequest::new("claude-sonnet-4-5", 1024, vec![Message::user("你好")]);
//! let response: MessageResponse = client.create(&request)?;
//! println!("{}", response.text());
//! ```
//!
//! # 示例（流式）
//!
//! ```text
//! let mut stream = client.create_stream(&request)?;
//! for event in stream.by_ref() {
//!     match event? {
//!         MessageStreamEvent::ContentBlockDelta(delta) => print!("{}", delta.delta.text.unwrap_or_default()),
//!         _ => {}
//!     }
//! }
//! // 或者一次性聚合：let final_message = client.create_stream(&request)?.aggregate()?;
//! ```
//!
//! # 示例（JSON 直通）
//!
//! ```text
//! let value = client.raw(&serde_json::json!({ "model": "...", "max_tokens": 64, "messages": [] }))?;
//! ```

mod stream;
mod types;

pub use stream::*;
pub use types::*;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::core::{AuthMode, LlmError, LlmResult, Transport};

/// Anthropic Messages 协议客户端。
///
/// 只依赖共享传输器（[`Transport`]），不做任何协议无关的 HTTP 细节。
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

    /// Messages 端点路径（默认 `/messages`，可配置）。
    pub fn messages_path(&self) -> &str {
        &self.transport.config().messages_path
    }

    /// count_tokens 端点路径（在 Messages 路径后追加 `/count_tokens`）。
    pub fn count_tokens_path(&self) -> String {
        format!(
            "{}/count_tokens",
            self.messages_path().trim_end_matches('/')
        )
    }

    /// 创建消息（同步）。请求体由 [`MessageRequest`] 序列化，响应反序列化成 [`MessageResponse`]。
    pub fn create(&self, request: &MessageRequest) -> LlmResult<MessageResponse> {
        let body = to_body(request)?;
        let value = self.transport.request_json(
            "POST",
            self.messages_path(),
            AuthMode::Anthropic,
            Some(&body),
        )?;
        parse_response(value)
    }

    /// 创建消息（流式）。
    ///
    /// 克隆请求并把 `stream` 强制设为 `true`，然后打开 SSE 流。
    pub fn create_stream(&self, request: &MessageRequest) -> LlmResult<MessageStream> {
        let body = stream_body(request)?;
        let reader = self.transport.open_stream(
            "POST",
            self.messages_path(),
            AuthMode::Anthropic,
            Some(&body),
        )?;
        Ok(MessageStream::new(reader))
    }

    /// 原始 JSON 直通：请求体原样发出，响应返回原始 JSON。
    pub fn raw(&self, body: &Value) -> LlmResult<Value> {
        self.transport.request_json(
            "POST",
            self.messages_path(),
            AuthMode::Anthropic,
            Some(body),
        )
    }

    /// 原始 JSON 流式直通。
    pub fn raw_stream(&self, body: &Value) -> LlmResult<MessageStream> {
        let reader = self.transport.open_stream(
            "POST",
            self.messages_path(),
            AuthMode::Anthropic,
            Some(body),
        )?;
        Ok(MessageStream::new(reader))
    }

    /// 统计 token（同步）。
    pub fn count_tokens(&self, request: &CountTokensRequest) -> LlmResult<CountTokensResponse> {
        let body = to_body(request)?;
        let path = self.count_tokens_path();
        let value = self
            .transport
            .request_json("POST", &path, AuthMode::Anthropic, Some(&body))?;
        parse_response(value)
    }

    /// 统计 token 的原始 JSON 直通。
    pub fn count_tokens_raw(&self, body: &Value) -> LlmResult<Value> {
        let path = self.count_tokens_path();
        self.transport
            .request_json("POST", &path, AuthMode::Anthropic, Some(body))
    }
}

/// 请求体序列化。
fn to_body<T: Serialize>(value: &T) -> LlmResult<Value> {
    serde_json::to_value(value)
        .map_err(|error| LlmError::Json(format!("请求体序列化失败：{error}")))
}

/// 流式请求体：克隆后把 `stream` 强制设为 `true`，不改动调用方的请求。
fn stream_body(request: &MessageRequest) -> LlmResult<Value> {
    let mut streaming = request.clone();
    streaming.stream = Some(true);
    to_body(&streaming)
}

/// 响应反序列化。
fn parse_response<T: DeserializeOwned>(value: Value) -> LlmResult<T> {
    serde_json::from_value(value)
        .map_err(|error| LlmError::Json(format!("响应反序列化失败：{error}")))
}
