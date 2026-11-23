//! 与宿主无关的共享层：配置、错误、纯数据类型。

pub mod config;
pub mod error;
pub mod tex;

pub use tex::{math_open, normalize_math};
pub mod types;

pub use config::{Config, ReplyMode};
pub use error::{AgentError, AgentResult};
pub use types::{
    ChatKey, ChatKind, ChatMessage, CommandCall, IncomingMessage, RunMode, TurnOutcome,
    TurnRequest, chunk_text, is_chat_content, strip_markdown,
};
