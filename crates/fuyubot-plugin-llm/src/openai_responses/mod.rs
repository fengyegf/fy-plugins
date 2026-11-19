//! OpenAI Responses 协议客户端。
//!
//! - 同步：POST {base}/responses
//! - 流式：POST {base}/responses，SSE，事件带 event: 名，data 为 JSON
//! - 查询：GET {base}/responses/{id}
//! - 删除：DELETE {base}/responses/{id}
//! - 认证：Bearer（Authorization: Bearer 加 api_key）
//!
//! 本目录只依赖 crate::core 的 Transport：HTTP、SSE、认证、超时、重试全部复用共享层，
//! 不直接使用 ureq，也不引入任何 AI SDK。
//!
//! # 示例
//!
//! ```no_run
//! use llm::{ClientConfig, LlmClient};
//! use llm::openai_responses::{ResponseInput, ResponseRequest};
//!
//! # fn main() -> Result<(), llm::LlmError> {
//! let client = LlmClient::new(ClientConfig::new("http://127.0.0.1:8080/v1", "sk-key"))?;
//! let request = ResponseRequest {
//!     model: Some("gpt-5".to_owned()),
//!     input: Some(ResponseInput::Text("你好".to_owned())),
//!     ..ResponseRequest::default()
//! };
//! let response = client.openai_responses().create(&request)?;
//! println!("{}", response.output_text());
//! # Ok(())
//! # }
//! ```
//!
//! 流式：
//!
//! ```no_run
//! use llm::{ClientConfig, LlmClient};
//! use llm::openai_responses::{ResponseInput, ResponseRequest};
//!
//! # fn main() -> Result<(), llm::LlmError> {
//! let client = LlmClient::new(ClientConfig::new("http://127.0.0.1:8080/v1", "sk-key"))?;
//! let request = ResponseRequest {
//!     model: Some("gpt-5".to_owned()),
//!     input: Some(ResponseInput::Text("写一首短诗".to_owned())),
//!     ..ResponseRequest::default()
//! };
//! let mut stream = client.openai_responses().create_stream(&request)?;
//! while let Some(event) = stream.next_event()? {
//!     if let Some(delta) = event.delta() {
//!         print!("{delta}");
//!     }
//! }
//! let response = client
//!     .openai_responses()
//!     .create_stream(&request)?
//!     .aggregate()?;
//! println!("{}", response.output_text());
//! # Ok(())
//! # }
//! ```

mod stream;
mod types;

pub use stream::*;
pub use types::*;

use serde_json::Value;

use crate::core::{AuthMode, LlmError, LlmResult, Transport};

/// OpenAI Responses 协议客户端。
///
/// 只持有共享传输器；克隆成本与 Transport 一致（连接池共享）。
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

    /// 同步创建响应（POST /responses）。
    pub fn create(&self, request: &ResponseRequest) -> LlmResult<ResponseObject> {
        let body = self.encode(request)?;
        let value = self.raw(&body)?;
        self.decode(value)
    }

    /// 流式创建响应（POST /responses，SSE）。
    ///
    /// 会自动把请求里的 stream 置为 true，调用方不用手动设置。
    pub fn create_stream(&self, request: &ResponseRequest) -> LlmResult<ResponseStream> {
        let body = self.encode_stream(request)?;
        self.open_stream(&body)
    }

    /// 原始 JSON 直通（非流式）：请求体原样发送，返回原始 JSON。
    ///
    /// 用于工具、未覆盖字段或转发场景；不做类型校验。
    pub fn raw(&self, body: &Value) -> LlmResult<Value> {
        let path = self.transport.config().responses_path.clone();
        self.transport
            .request_json("POST", &path, AuthMode::Bearer, Some(body))
    }

    /// 原始 JSON 直通（流式）：请求体原样发送，并把 stream 置为 true。
    pub fn raw_stream(&self, body: &Value) -> LlmResult<ResponseStream> {
        let mut body = body.clone();
        enable_stream(&mut body);
        self.open_stream(&body)
    }

    /// 查询响应（GET /responses/{id}）。
    pub fn retrieve(&self, response_id: &str) -> LlmResult<ResponseObject> {
        let path = self.response_path(response_id);
        let value = self
            .transport
            .request_json("GET", &path, AuthMode::Bearer, None)?;
        self.decode(value)
    }

    /// 删除已存储的响应（DELETE /responses/{id}）。
    pub fn delete(&self, response_id: &str) -> LlmResult<ResponseDeletion> {
        let path = self.response_path(response_id);
        let value = self
            .transport
            .request_json("DELETE", &path, AuthMode::Bearer, None)?;
        serde_json::from_value(value)
            .map_err(|error| LlmError::Json(format!("Responses 删除结果解析失败：{error}")))
    }

    /// 请求体序列化。
    fn encode(&self, request: &ResponseRequest) -> LlmResult<Value> {
        serde_json::to_value(request)
            .map_err(|error| LlmError::Json(format!("Responses 请求序列化失败：{error}")))
    }

    /// 流式请求体：序列化后强制 stream = true。
    fn encode_stream(&self, request: &ResponseRequest) -> LlmResult<Value> {
        let mut body = self.encode(request)?;
        enable_stream(&mut body);
        Ok(body)
    }

    /// 响应反序列化。
    fn decode(&self, value: Value) -> LlmResult<ResponseObject> {
        serde_json::from_value(value)
            .map_err(|error| LlmError::Json(format!("Responses 响应解析失败：{error}")))
    }

    fn open_stream(&self, body: &Value) -> LlmResult<ResponseStream> {
        let path = self.transport.config().responses_path.clone();
        let reader = self
            .transport
            .open_stream("POST", &path, AuthMode::Bearer, Some(body))?;
        Ok(ResponseStream::new(reader))
    }

    /// responses_path 加响应 ID，例如 /responses/resp_123。
    fn response_path(&self, response_id: &str) -> String {
        let base = self.transport.config().responses_path.trim_end_matches('/');
        format!("{base}/{}", response_id.trim_matches('/'))
    }
}

/// 把请求体里的 stream 强制置为 true（仅对 JSON 对象生效）。
fn enable_stream(body: &mut Value) {
    if let Value::Object(map) = body {
        map.insert("stream".to_owned(), Value::Bool(true));
    }
}
