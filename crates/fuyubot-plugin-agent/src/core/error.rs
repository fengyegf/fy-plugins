//! 插件错误：每一类都带稳定 code，工具 / 服务回包直接复用。

use serde_json::{Value, json};

/// 插件统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// 宿主 ABI 调用失败。
    #[error("宿主调用失败：{0}")]
    Host(String),
    /// TDLib 请求 / 应答失败。
    #[error("TDLib 调用失败：{0}")]
    Td(String),
    /// LLM 网关错误（服务不存在 / 上游报错）。
    #[error("LLM 网关错误：{0}")]
    Gateway(String),
    /// 会话存储失败。
    #[error("会话存储错误：{0}")]
    Store(String),
    /// 配置问题。
    #[error("配置错误：{0}")]
    Config(String),
    /// 入参不合法。
    #[error("参数错误：{0}")]
    BadRequest(String),
    /// 功能被配置关闭。
    #[error("功能未启用：{0}")]
    Disabled(String),
}

impl AgentError {
    /// 稳定的英文错误代号，工具 / 服务回包里用。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Host(_) => "host",
            Self::Td(_) => "tdlib",
            Self::Gateway(_) => "gateway",
            Self::Store(_) => "store",
            Self::Config(_) => "config",
            Self::BadRequest(_) => "bad_request",
            Self::Disabled(_) => "disabled",
        }
    }

    /// 工具 / 服务统一失败回包。
    pub fn to_json(&self) -> Value {
        json!({ "ok": false, "code": self.code(), "error": self.to_string() })
    }
}

/// 插件 Result 别名。
pub type AgentResult<T> = Result<T, AgentError>;
