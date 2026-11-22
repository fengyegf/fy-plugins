//! 统一错误类型：协议层、传输层、SSE 层、插件层共用一个错误枚举。
//!
//! 设计约束：
//! - 不依赖任何 AI SDK，错误形状自己定义；
//! - 把三类上游错误（OpenAI / Anthropic / 网关）都归一成 [`LlmError::Api`]；
//! - [`LlmError::to_json`] 同时给工具 / 服务调用方一个稳定的 JSON 形状。

use serde_json::{Value, json};

/// 协议网关的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// 本地配置不完整 / 不合法。
    #[error("配置错误：{0}")]
    Config(String),

    /// 网络层失败（DNS、连接、超时、TLS、读写中断等）。
    #[error("HTTP 传输失败：{0}")]
    Transport(String),

    /// 上游返回了非 2xx。`body` 保留原始响应，便于排错。
    #[error("API 返回错误（HTTP {status}）：{message}")]
    Api {
        status: u16,
        message: String,
        /// OpenAI 的 `error.type`，或 Anthropic 的 `error.type`。
        kind: Option<String>,
        /// OpenAI 的 `error.code`。
        code: Option<String>,
        /// 原始响应体（可能很大，调用方自行截断展示）。
        body: String,
    },

    /// 上游返回 2xx，但响应体不是合法 JSON。
    #[error("JSON 解析失败：{0}")]
    Json(String),

    /// SSE 流格式不合法（不是 `field: value` 的行、数据不是 JSON 等）。
    #[error("SSE 解析失败：{0}")]
    Sse(String),

    /// 协议名不在支持列表里。
    #[error("不支持的协议：{0}")]
    UnsupportedProtocol(String),

    /// 其它错误（参数校验、内部状态等）。
    #[error("其他错误：{0}")]
    Other(String),
}

/// 本 crate 的 `Result` 别名。
pub type LlmResult<T> = std::result::Result<T, LlmError>;

impl LlmError {
    /// 从非 2xx 响应构造错误，并尽量识别 OpenAI / Anthropic 的错误形状。
    ///
    /// 识别顺序：
    /// 1. `{"error":{"message":…,"type":…,"code":…}}`（OpenAI）
    /// 2. `{"type":"error","error":{"type":…,"message":…}}`（Anthropic）
    /// 3. `{"message":…}` / `{"error":"…"}`（网关兜底）
    /// 4. 原始文本前 400 字符
    pub fn api(status: u16, body: &str) -> Self {
        let value: Option<Value> = serde_json::from_str(body).ok();
        let mut message = None;
        let mut kind = None;
        let mut code = None;

        if let Some(value) = &value {
            // OpenAI / 大多数兼容网关。
            if let Some(error) = value.get("error") {
                if let Some(text) = error.as_str() {
                    message = Some(text.to_owned());
                } else {
                    message = string_field(error, "message");
                    kind = string_field(error, "type");
                    code = string_field(error, "code");
                }
            }
            // Anthropic：顶层 type=error，错误细节在 error 对象里。
            if message.is_none()
                && value.get("type").and_then(Value::as_str) == Some("error")
                && let Some(error) = value.get("error")
            {
                message = string_field(error, "message");
                kind = kind.or_else(|| string_field(error, "type"));
            }
            // 网关 / 中间层的扁平形状。
            if message.is_none() {
                message = string_field(value, "message");
                kind = kind.or_else(|| string_field(value, "type"));
                code = code.or_else(|| string_field(value, "code"));
            }
        }

        let message = message
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| truncate(body.trim(), 400));

        Self::Api {
            status,
            message,
            kind,
            code,
            body: body.to_owned(),
        }
    }

    /// 错误分类，给工具层 / 服务层拼 JSON 用。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Config(_) => "config",
            Self::Transport(_) => "transport",
            Self::Api { .. } => "api",
            Self::Json(_) => "json",
            Self::Sse(_) => "sse",
            Self::UnsupportedProtocol(_) => "unsupported_protocol",
            Self::Other(_) => "other",
        }
    }

    /// 是否值得重试：429 / 5xx / 网络错误。
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Api { status, .. } => {
                *status == 408 || *status == 409 || *status == 429 || *status >= 500
            }
            Self::Transport(_) => true,
            _ => false,
        }
    }

    /// 上游 HTTP 状态码（本地错误返回 `None`）。
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// 稳定的 JSON 形状：`{"ok":false,"kind":…,"message":…}`。
    pub fn to_json(&self) -> Value {
        match self {
            Self::Api {
                status,
                message,
                kind,
                code,
                body,
            } => json!({
                "ok": false,
                "kind": "api",
                "status": status,
                "message": message,
                "type": kind,
                "code": code,
                "body": truncate(body, 4000),
            }),
            other => json!({
                "ok": false,
                "kind": other.kind(),
                "message": other.to_string(),
            }),
        }
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|field| match field {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}
