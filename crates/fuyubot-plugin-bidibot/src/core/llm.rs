//! OpenAI 兼容的 chat 客户端（同步、阻塞）。
//!
//! 只做一件事：POST /chat/completions，把 messages 里的 system / user 两段发出去，
//! 取 choices[0].message.content。翻译与广告判定都复用这一条通道。
//!
//! **同步阻塞**：只能在插件的业务线程里调，绝不能在 update 回调里调。
//! 失败一律返回 LlmError，由调用方决定降级（翻译退回原文、广告退回关键词）。

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::core::config::AiConfig;

/// 单次请求的超时上限：配置给了也夹在这一档，防止一个卡住的请求拖死线程。
const MAX_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// 没配 api_key / base_url，或模型名为空。
    #[error("没有配置可用的 AI 凭据")]
    Disabled,
    /// 连接层失败（DNS / 超时 / TLS）。
    #[error("请求失败: {0}")]
    Http(String),
    /// HTTP 非 2xx。
    #[error("接口返回 {0}: {1}")]
    Status(u16, String),
    /// 应答不是预期形状（没有 choices / content 不是字符串）。
    #[error("应答解析失败: {0}")]
    Parse(String),
}

/// 一个 OpenAI 兼容的 chat 客户端。与 HostApi 无关，可放进 Arc 共享。
#[derive(Debug, Clone)]
pub struct Client {
    base_url: String,
    api_key: String,
    model: String,
    temperature: f64,
}

impl Client {
    pub fn new(ai: &AiConfig) -> Self {
        Self {
            base_url: normalize_base(&ai.base_url),
            api_key: ai.api_key.trim().to_owned(),
            model: ai.model.trim().to_owned(),
            temperature: ai.temperature,
        }
    }

    pub fn enabled(&self) -> bool {
        !self.api_key.is_empty() && !self.base_url.is_empty() && !self.model.is_empty()
    }

    /// 拼一次 chat/completions 的请求体。抽成纯函数，单测不必发网络。
    pub fn request_body(&self, system: &str, user: &str) -> Value {
        json!({
            "model": self.model,
            "temperature": self.temperature,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        })
    }

    /// 发一次对话，返回 assistant 的纯文本。
    pub fn chat(&self, system: &str, user: &str, timeout: Duration) -> Result<String, LlmError> {
        if !self.enabled() {
            return Err(LlmError::Disabled);
        }
        let timeout = timeout.min(MAX_TIMEOUT);
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        let url = format!("{}/chat/completions", self.base_url);
        let body = self.request_body(system, user).to_string();

        let response = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_string(&body);

        let (status, text) = match response {
            Ok(response) => (
                response.status(),
                response
                    .into_string()
                    .map_err(|error| LlmError::Http(error.to_string()))?,
            ),
            Err(ureq::Error::Status(status, response)) => (
                status,
                response
                    .into_string()
                    .unwrap_or_else(|error| error.to_string()),
            ),
            Err(error) => return Err(LlmError::Http(error.to_string())),
        };
        if !(200..300).contains(&status) {
            return Err(LlmError::Status(status, truncate(&text, 300)));
        }
        extract_content(&text)
    }

    /// 发一次对话并解析成 JSON（广告判定要结构化的结论）。
    ///
    /// 模型可能把 JSON 包在 markdown 代码块里，这里先剥再解析。
    pub fn chat_json<T: DeserializeOwned>(
        &self,
        system: &str,
        user: &str,
        timeout: Duration,
    ) -> Result<T, LlmError> {
        let text = self.chat(system, user, timeout)?;
        serde_json::from_str(&strip_code_fence(&text))
            .map_err(|error| LlmError::Parse(format!("{error}: {}", truncate(&text, 300))))
    }
}

/// 去掉结尾的斜杠，允许用户把 base_url 写成 ".../v1/"。
fn normalize_base(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_owned()
}

/// 从 chat/completions 应答里取 choices[0].message.content。
fn extract_content(body: &str) -> Result<String, LlmError> {
    let value: Value =
        serde_json::from_str(body).map_err(|error| LlmError::Parse(format!("{error}")))?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError::Parse("没有 choices[0].message.content".to_owned()))?;
    Ok(content.trim().to_owned())
}

/// 剥掉 markdown 代码围栏；没有围栏就原样返回。
pub fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_owned();
    };
    // 第一行是语言标记（json / JSON / 空），跳过去。
    let rest = rest.split_once('\n').map_or(rest, |(_, body)| body);
    rest.trim_end()
        .strip_suffix("```")
        .unwrap_or(rest)
        .trim()
        .to_owned()
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new(&AiConfig {
            base_url: "https://api.example.com/v1/".to_owned(),
            api_key: "sk-test".to_owned(),
            model: "gpt-test".to_owned(),
            timeout: Duration::from_secs(5),
            temperature: 0.2,
        })
    }

    #[test]
    fn base_url_trailing_slash_is_normalized() {
        assert_eq!(client().base_url, "https://api.example.com/v1");
    }

    #[test]
    fn request_body_is_openai_chat_shape() {
        let body = client().request_body("sys", "hi");
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert_eq!(body["temperature"], 0.2);
    }

    #[test]
    fn missing_key_means_disabled() {
        let mut client = client();
        client.api_key.clear();
        assert!(!client.enabled());
        assert!(matches!(
            client.chat("s", "u", Duration::from_secs(1)),
            Err(LlmError::Disabled)
        ));
    }

    #[test]
    fn content_is_extracted_from_choices() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":" 你好 "}}]}"#;
        assert_eq!(extract_content(body).unwrap(), "你好");
        assert!(extract_content(r#"{"choices":[]}"#).is_err());
    }

    #[test]
    fn code_fence_is_stripped() {
        const FENCE: &str = "\u{60}\u{60}\u{60}";
        const BODY: &str = r#"{"a":1}"#;
        assert_eq!(strip_code_fence("{\"a\":1}"), "{\"a\":1}");
        let fenced = format!("{}json\n{}\n{}", FENCE, BODY, FENCE);
        assert_eq!(strip_code_fence(&fenced), "{\"a\":1}");
        let bare = format!("{}\n{}\n{}", FENCE, BODY, FENCE);
        assert_eq!(strip_code_fence(&bare), "{\"a\":1}");
    }
}
