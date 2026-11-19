//! 阻塞式 HTTP 传输层：三个协议共用。
//!
//! 只用 `ureq` 做「发请求 / 收字节」，JSON、SSE、重试、认证、超时全部自己实现，
//! 不引入任何 AI SDK 或 async runtime。
//!
//! - 同步请求：[`Transport::request_json`]
//! - 流式请求：[`Transport::open_stream`] 返回 [`SseReader`]
//! - 认证：OpenAI 系走 `Authorization: Bearer`，Anthropic 走 `x-api-key` + `anthropic-version`

use std::io::{BufReader, Read};
use std::thread::sleep;
use std::time::Duration;

use serde_json::Value;
use ureq::{Agent, AgentBuilder, Error as UreqError};

use crate::core::{ClientConfig, LlmError, LlmResult, SseReader};

/// 认证方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// `Authorization: Bearer <key>`（OpenAI Chat Completions / Responses）。
    Bearer,
    /// `x-api-key: <key>` + `anthropic-version: …`（Anthropic Messages）。
    Anthropic,
}

/// SSE 响应流的具体类型：`Box<dyn Read + Send + Sync>` 来自 `ureq::Response::into_reader`。
pub type SseResponse = SseReader<BufReader<Box<dyn Read + Send + Sync>>>;

/// 协议无关的传输器，`Clone` 只是克隆内部的 `ureq::Agent`（廉价，连接池共享）。
#[derive(Clone)]
pub struct Transport {
    config: ClientConfig,
    /// 普通请求：有总超时。
    agent: Agent,
    /// 流式请求：不设总超时，只设连接 / 单次读超时。
    stream_agent: Agent,
}

/// 传输层内部错误：区分「上游给了状态码」和「根本没连上」。
enum SendFailure {
    Status(u16, ureq::Response),
    Transport(String),
}

impl Transport {
    /// 构造传输器并校验配置。
    pub fn new(config: ClientConfig) -> LlmResult<Self> {
        config.validate()?;
        let agent = AgentBuilder::new()
            .timeout_connect(Duration::from_secs(config.connect_timeout_secs.max(1)))
            .timeout(Duration::from_secs(config.timeout_secs.max(1)))
            .user_agent(&config.user_agent)
            .build();
        let stream_agent = AgentBuilder::new()
            .timeout_connect(Duration::from_secs(config.connect_timeout_secs.max(1)))
            .timeout_read(Duration::from_secs(config.read_timeout_secs.max(1)))
            .user_agent(&config.user_agent)
            .build();
        Ok(Self {
            config,
            agent,
            stream_agent,
        })
    }

    /// 读取配置。
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// 发一个同步 JSON 请求并解析响应。
    ///
    /// `method` 支持 `GET` / `POST` / `DELETE`；`path` 为相对路径（会拼 `base_url`）
    /// 或绝对 URL。`body` 为 `None` 时不带请求体。
    pub fn request_json(
        &self,
        method: &str,
        path: &str,
        auth: AuthMode,
        body: Option<&Value>,
    ) -> LlmResult<Value> {
        let url = self.config.endpoint(path);
        let payload = body
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| LlmError::Json(format!("请求体序列化失败：{error}")))?;

        if self.config.debug {
            eprintln!(
                "[llm] {method} {url} auth={:?} body={} bytes",
                auth,
                payload.as_ref().map(String::len).unwrap_or(0)
            );
        }

        let mut attempt = 0u32;
        loop {
            match self.send(&self.agent, method, &url, auth, payload.as_deref(), false) {
                Ok(response) => {
                    if self.config.debug {
                        eprintln!("[llm] <- {} {}", response.status(), response.status_text());
                    }
                    return decode_json(response);
                }
                Err(SendFailure::Status(status, response)) => {
                    let retry_after = parse_retry_after(response.header("retry-after"));
                    let text = response.into_string().unwrap_or_default();
                    let error = LlmError::api(status, &text);
                    if attempt < self.config.max_retries && error.is_retryable() {
                        attempt += 1;
                        sleep(backoff(attempt, retry_after));
                        continue;
                    }
                    return Err(error);
                }
                Err(SendFailure::Transport(message)) => {
                    let error = LlmError::Transport(message);
                    if attempt < self.config.max_retries {
                        attempt += 1;
                        sleep(backoff(attempt, None));
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    /// 发一个流式请求，返回逐事件读取器。
    ///
    /// 重试只覆盖「连接阶段失败 / 429 / 5xx」；一旦拿到 2xx 响应，后续断流不重试
    /// （重试会重复消费已经产生的 token）。
    pub fn open_stream(
        &self,
        method: &str,
        path: &str,
        auth: AuthMode,
        body: Option<&Value>,
    ) -> LlmResult<SseResponse> {
        let url = self.config.endpoint(path);
        let payload = body
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| LlmError::Json(format!("请求体序列化失败：{error}")))?;

        if self.config.debug {
            eprintln!(
                "[llm] {method} {url} (stream) auth={:?} body={} bytes",
                auth,
                payload.as_ref().map(String::len).unwrap_or(0)
            );
        }

        let mut attempt = 0u32;
        loop {
            match self.send(
                &self.stream_agent,
                method,
                &url,
                auth,
                payload.as_deref(),
                true,
            ) {
                Ok(response) => {
                    if self.config.debug {
                        eprintln!(
                            "[llm] <- {} {} (stream, content-type={:?})",
                            response.status(),
                            response.status_text(),
                            response.header("content-type")
                        );
                    }
                    let reader = response.into_reader();
                    return Ok(SseReader::new(BufReader::new(reader)));
                }
                Err(SendFailure::Status(status, response)) => {
                    let retry_after = parse_retry_after(response.header("retry-after"));
                    let text = response.into_string().unwrap_or_default();
                    let error = LlmError::api(status, &text);
                    if attempt < self.config.max_retries && error.is_retryable() {
                        attempt += 1;
                        sleep(backoff(attempt, retry_after));
                        continue;
                    }
                    return Err(error);
                }
                Err(SendFailure::Transport(message)) => {
                    let error = LlmError::Transport(message);
                    if attempt < self.config.max_retries {
                        attempt += 1;
                        sleep(backoff(attempt, None));
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    fn send(
        &self,
        agent: &Agent,
        method: &str,
        url: &str,
        auth: AuthMode,
        body: Option<&str>,
        stream: bool,
    ) -> Result<ureq::Response, SendFailure> {
        let mut request = match method {
            "GET" => agent.get(url),
            "POST" => agent.post(url),
            "DELETE" => agent.delete(url),
            other => {
                return Err(SendFailure::Transport(format!(
                    "不支持的 HTTP 方法：{other}"
                )));
            }
        };

        request = request.set(
            "Accept",
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        );
        for (name, value) in self.auth_headers(auth) {
            request = request.set(name, &value);
        }
        for (name, value) in &self.config.headers {
            request = request.set(name, value);
        }

        let result = match body {
            Some(body) => request
                .set("Content-Type", "application/json")
                .send_string(body),
            None => request.call(),
        };

        result.map_err(|error| match error {
            UreqError::Status(status, response) => SendFailure::Status(status, response),
            UreqError::Transport(error) => SendFailure::Transport(error.to_string()),
        })
    }

    fn auth_headers(&self, auth: AuthMode) -> Vec<(&'static str, String)> {
        let key = self.config.api_key.trim();
        match auth {
            AuthMode::Bearer => {
                let mut headers = Vec::new();
                if !key.is_empty() {
                    headers.push(("Authorization", format!("Bearer {key}")));
                }
                if !self.config.organization.trim().is_empty() {
                    headers.push((
                        "OpenAI-Organization",
                        self.config.organization.trim().to_owned(),
                    ));
                }
                if !self.config.project.trim().is_empty() {
                    headers.push(("OpenAI-Project", self.config.project.trim().to_owned()));
                }
                headers
            }
            AuthMode::Anthropic => {
                let mut headers = Vec::new();
                if !key.is_empty() {
                    headers.push(("x-api-key", key.to_owned()));
                }
                headers.push((
                    "anthropic-version",
                    if self.config.anthropic_version.trim().is_empty() {
                        crate::core::config::DEFAULT_ANTHROPIC_VERSION.to_owned()
                    } else {
                        self.config.anthropic_version.trim().to_owned()
                    },
                ));
                if !self.config.anthropic_beta.trim().is_empty() {
                    headers.push((
                        "anthropic-beta",
                        self.config.anthropic_beta.trim().to_owned(),
                    ));
                }
                headers
            }
        }
    }
}

fn decode_json(response: ureq::Response) -> LlmResult<Value> {
    let text = response
        .into_string()
        .map_err(|error| LlmError::Transport(format!("读取响应体失败：{error}")))?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|error| {
        let preview: String = text.chars().take(400).collect();
        LlmError::Json(format!("响应不是合法 JSON：{error}；响应体：{preview}"))
    })
}

fn parse_retry_after(value: Option<&str>) -> Option<u64> {
    value
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(|secs| secs.min(60))
}

fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    if let Some(secs) = retry_after {
        return Duration::from_secs(secs.max(1));
    }
    // 300ms、600ms、1.2s…，上限 5s。
    let millis = 300u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(10));
    Duration::from_millis(millis.min(5_000))
}
