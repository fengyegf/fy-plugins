//! 协议客户端配置：一个 `ClientConfig` 服务三个协议。
//!
//! 解析来源优先级（插件场景由调用方决定）：
//! 1. 显式构造（[`ClientConfig::new`]）；
//! 2. 环境变量（[`ClientConfig::from_env`]）；
//! 3. 插件配置段 `[plugin.fengyegf.llm]`（[`ClientConfig::from_json`]）。
//!
//! 字段全部有默认值，所以只写关心的键也能跑。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::{LlmError, LlmResult};

/// 默认接入地址（标准厂商示例：DeepSeek 的 OpenAI 兼容入口，可换成任意兼容地址）。
pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
/// Anthropic Messages 默认版本头。
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_owned()
}

fn default_anthropic_version() -> String {
    DEFAULT_ANTHROPIC_VERSION.to_owned()
}

fn default_timeout_secs() -> u64 {
    120
}

fn default_connect_timeout_secs() -> u64 {
    15
}

fn default_read_timeout_secs() -> u64 {
    300
}

fn default_max_retries() -> u32 {
    2
}

fn default_user_agent() -> String {
    concat!("fuyubot-plugin-llm/", env!("CARGO_PKG_VERSION")).to_owned()
}

fn default_chat_path() -> String {
    "/chat/completions".to_owned()
}

fn default_responses_path() -> String {
    "/responses".to_owned()
}

fn default_messages_path() -> String {
    "/messages".to_owned()
}

fn default_models_path() -> String {
    "/models".to_owned()
}

/// 三个协议共享的传输配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// 服务根地址，不带协议路径后缀，例如 `https://api.deepseek.com`。
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// API Key。OpenAI 走 `Authorization: Bearer`，Anthropic 走 `x-api-key`。
    pub api_key: String,
    /// OpenAI 可选身份头 `OpenAI-Organization`。
    pub organization: String,
    /// OpenAI 可选身份头 `OpenAI-Project`。
    pub project: String,
    /// Anthropic Messages 必带版本头。
    #[serde(default = "default_anthropic_version")]
    pub anthropic_version: String,
    /// Anthropic 可选 beta 头（逗号分隔，例如 `prompt-caching-2024-07-31`）。
    pub anthropic_beta: String,
    /// 单次请求总超时（秒）。
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// 连接超时（秒）。
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// 流式读取的单次 read 超时（秒）。
    #[serde(default = "default_read_timeout_secs")]
    pub read_timeout_secs: u64,
    /// 429 / 5xx / 网络错误的重试次数（不含首次请求）。
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// User-Agent。
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
    /// OpenAI Chat Completions 路径。
    #[serde(default = "default_chat_path")]
    pub chat_completions_path: String,
    /// OpenAI Responses 路径。
    #[serde(default = "default_responses_path")]
    pub responses_path: String,
    /// Anthropic Messages 路径。
    #[serde(default = "default_messages_path")]
    pub messages_path: String,
    /// 模型列表路径（OpenAI 风格 `GET /models`）。
    #[serde(default = "default_models_path")]
    pub models_path: String,
    /// 附加到每个请求的自定义头。
    pub headers: BTreeMap<String, String>,
    /// 打印请求 / 响应摘要到 stderr（不打印 Key）。
    pub debug: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_key: String::new(),
            organization: String::new(),
            project: String::new(),
            anthropic_version: default_anthropic_version(),
            anthropic_beta: String::new(),
            timeout_secs: default_timeout_secs(),
            connect_timeout_secs: default_connect_timeout_secs(),
            read_timeout_secs: default_read_timeout_secs(),
            max_retries: default_max_retries(),
            user_agent: default_user_agent(),
            chat_completions_path: default_chat_path(),
            responses_path: default_responses_path(),
            messages_path: default_messages_path(),
            models_path: default_models_path(),
            headers: BTreeMap::new(),
            debug: false,
        }
    }
}

impl ClientConfig {
    /// 显式构造：接入地址 + API Key。
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    /// 从环境变量构造，未设置的变量保持默认值。
    ///
    /// 支持的变量：
    /// - `FUYUBOT_LLM_BASE_URL`
    /// - `FUYUBOT_LLM_API_KEY`
    /// - `FUYUBOT_LLM_ORGANIZATION`
    /// - `FUYUBOT_LLM_PROJECT`
    /// - `FUYUBOT_LLM_ANTHROPIC_VERSION`
    /// - `FUYUBOT_LLM_ANTHROPIC_BETA`
    /// - `FUYUBOT_LLM_TIMEOUT_SECS`
    /// - `FUYUBOT_LLM_MAX_RETRIES`
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(value) = std::env::var("FUYUBOT_LLM_BASE_URL") {
            if !value.trim().is_empty() {
                config.base_url = value;
            }
        }
        if let Ok(value) = std::env::var("FUYUBOT_LLM_API_KEY") {
            config.api_key = value;
        }
        if let Ok(value) = std::env::var("FUYUBOT_LLM_ORGANIZATION") {
            config.organization = value;
        }
        if let Ok(value) = std::env::var("FUYUBOT_LLM_PROJECT") {
            config.project = value;
        }
        if let Ok(value) = std::env::var("FUYUBOT_LLM_ANTHROPIC_VERSION") {
            if !value.trim().is_empty() {
                config.anthropic_version = value;
            }
        }
        if let Ok(value) = std::env::var("FUYUBOT_LLM_ANTHROPIC_BETA") {
            config.anthropic_beta = value;
        }
        if let Ok(value) = std::env::var("FUYUBOT_LLM_TIMEOUT_SECS") {
            if let Ok(secs) = value.parse::<u64>() {
                config.timeout_secs = secs;
            }
        }
        if let Ok(value) = std::env::var("FUYUBOT_LLM_MAX_RETRIES") {
            if let Ok(retries) = value.parse::<u32>() {
                config.max_retries = retries;
            }
        }
        config
    }

    /// 从插件配置段的 JSON 解析；缺失字段用默认值。
    pub fn from_json(value: &Value) -> LlmResult<Self> {
        serde_json::from_value(value.clone())
            .map_err(|error| LlmError::Config(format!("配置段不是合法的 ClientConfig：{error}")))
    }

    /// 校验最低限度的可用性。
    pub fn validate(&self) -> LlmResult<()> {
        if self.base_url.trim().is_empty() {
            return Err(LlmError::Config("base_url 不能为空".to_owned()));
        }
        if !(self.base_url.starts_with("http://") || self.base_url.starts_with("https://")) {
            return Err(LlmError::Config(format!(
                "base_url 必须以 http:// 或 https:// 开头：{}",
                self.base_url
            )));
        }
        Ok(())
    }

    /// 是否配置了 API Key。
    pub fn has_api_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// 打码后的 Key，用于日志 / `describe`。
    pub fn masked_api_key(&self) -> String {
        let key = self.api_key.trim();
        if key.is_empty() {
            return String::new();
        }
        let prefix: String = key.chars().take(6).collect();
        let suffix: String = key
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{prefix}…{suffix}")
    }

    /// 拼接最终 URL。
    ///
    /// - `path` 是绝对 URL（`http://…` / `https://…`）时原样返回；
    /// - 否则 `base_url` 去尾斜杠、`path` 去头斜杠后拼接。
    pub fn endpoint(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_owned();
        }
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}
