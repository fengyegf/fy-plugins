//! # fuyubot-plugin-llm
//!
//! 面向 AI Agent 的大模型协议网关。**不依赖任何 AI SDK**，HTTP / SSE / 认证 / 重试全部自己实现，
//! 把三个常用协议封装成三个独立目录：
//!
//! - [`openai_chat`]：OpenAI Chat Completions（`POST /v1/chat/completions`）
//! - [`openai_responses`]：OpenAI Responses（`POST /v1/responses`）
//! - [`anthropic_messages`]：Anthropic Messages（`POST /v1/messages`）
//!
//! ## 目录约定
//!
//! ```text
//! src/
//! ├── core/                 # 与协议无关：配置、错误、HTTP、SSE
//! ├── openai_chat/          # 协议 1：请求 / 响应 / 流式 / 客户端
//! ├── openai_responses/     # 协议 2
//! ├── anthropic_messages/   # 协议 3
//! ├── api.rs               # LlmClient：统一门面 + JSON 直通 + 自检
//! └── plugin/              # 宿主插件壳：工具 / 共享服务
//! ```
//!
//! ## 内部调用（Rust）
//!
//! ```no_run
//! use llm::{ClientConfig, LlmClient, Protocol};
//! use serde_json::json;
//!
//! let client = LlmClient::new(ClientConfig::new(
//!     "https://api.deepseek.com",
//!     "sk-…",
//! ))?;
//!
//! // JSON 直通（最稳，覆盖协议所有字段）
//! let value = client.request_json(
//!     Protocol::OpenAiChat,
//!     &json!({
//!         "model": "deepseek-chat",
//!         "messages": [{ "role": "user", "content": "你好" }],
//!         "max_tokens": 256,
//!     }),
//! )?;
//! println!("{}", value);
//! # Ok::<(), llm::LlmError>(())
//! ```
//!
//! ## 外部调用
//!
//! 插件启动后注册六个工具与一个共享服务，见 [`plugin`]：
//!
//! - 工具：`fengyegf.llm.chat` / `.responses` / `.messages` / `.models` / `.test` / `.describe`
//! - 服务：`fengyegf.llm`（方法 `chat` / `responses` / `messages` / `models` / `test` / `describe`）

pub mod anthropic_messages;
pub mod api;
pub mod core;
pub mod openai_chat;
pub mod openai_responses;
pub mod plugin;

pub use api::{LlmClient, Protocol, ProtocolTest, RawStream, TestReport, value_to_pretty};
pub use core::{
    AuthMode, ClientConfig, DEFAULT_ANTHROPIC_VERSION, DEFAULT_BASE_URL, LlmError, LlmResult,
    SseEvent, SseReader, Transport,
};
