//! 协议无关的基础设施：配置、错误、HTTP 传输、SSE 解析。

pub mod config;
pub mod error;
pub mod http;
pub mod sse;

pub use config::{ClientConfig, DEFAULT_ANTHROPIC_VERSION, DEFAULT_BASE_URL};
pub use error::{LlmError, LlmResult};
pub use http::{AuthMode, SseResponse, Transport};
pub use sse::{SseEvent, SseReader, parse_sse_block};
