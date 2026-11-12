//! 阻塞式 HTTP 客户端。
//!
//! 业务线程本来就是独占的，没必要引 async runtime。这里的职责很窄：
//! 建 agent（可选代理 / 超时 / UA）、取文本、把各种失败归一成 [`FetchError`]。
//!
//! 响应体一律按「最多再读 1 字节」的方式限长：接口不一定带 `Content-Length`
//! （chunked、gzip 压缩时都不可信），所以头部只能当提前拒绝的提示，
//! 真正的上限必须在读取时兜底。

use std::io::Read;
use std::time::Duration;

/// HTTP 层的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// 建客户端失败（代理地址非法等）。
    #[error("构建 HTTP 客户端失败: {0}")]
    Client(String),
    /// 网络 / TLS / 超时等传输层失败。
    #[error("请求 {url} 失败: {message}")]
    Network { url: String, message: String },
    /// 非 2xx 响应。
    #[error("HTTP {status}: {url}")]
    Status { status: u16, url: String },
    /// 响应体超过上限。
    #[error("响应体超过上限 {limit} 字节: {url}")]
    TooLarge { url: String, limit: u64 },
    /// 文本解码 / 结构解析失败。
    #[error("解析响应失败: {0}")]
    Decode(String),
}

/// 复用的阻塞 HTTP 客户端。
#[derive(Debug, Clone)]
pub struct HttpClient {
    agent: ureq::Agent,
}

impl HttpClient {
    /// `proxy` 形如 `http://127.0.0.1:7890`；`None` 直连。
    ///
    /// 超时是**整次请求**的超时（连接 + 收响应 + 读 body），业务线程只有一个，
    /// 不能让它卡在网络上一动不动。空串 / 纯空白等同于直连。
    pub fn new(
        proxy: Option<&str>,
        timeout: Duration,
        user_agent: &str,
    ) -> Result<Self, FetchError> {
        let mut builder = ureq::AgentBuilder::new()
            .timeout(timeout)
            .user_agent(user_agent);

        if let Some(proxy) = proxy.map(str::trim).filter(|proxy| !proxy.is_empty()) {
            let parsed = ureq::Proxy::new(proxy)
                .map_err(|error| FetchError::Client(format!("代理地址 {proxy:?} 非法: {error}")))?;
            builder = builder.proxy(parsed);
        }

        Ok(Self {
            agent: builder.build(),
        })
    }

    /// GET 一段文本（JSON / HTML）。
    pub fn get_text(&self, url: &str, max_bytes: u64) -> Result<String, FetchError> {
        let body = self.request(url, max_bytes)?;
        // Epic 接口固定 UTF-8；解不出来说明拿到的不是预期内容，宁可报错也别乱码入图。
        String::from_utf8(body)
            .map_err(|error| FetchError::Decode(format!("响应不是合法 UTF-8: {error}")))
    }

    /// 发 GET 并读出受限于 `max_bytes` 的响应体。
    fn request(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|error| map_error(url, error))?;

        // 头部声明超限时直接拒绝，省得白拉一遍体积。
        if let Some(declared) = content_length(&response)
            && declared > max_bytes
        {
            return Err(FetchError::TooLarge {
                url: url.to_owned(),
                limit: max_bytes,
            });
        }

        // 多读 1 字节：读满 `max_bytes + 1` 即判定超限，不会把超大响应整个读进内存。
        // 这里同时兜住了「没有 Content-Length」与「头部撒谎」两种情况。
        let limit = max_bytes.saturating_add(1);
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit)
            .read_to_end(&mut body)
            .map_err(|error| FetchError::Network {
                url: url.to_owned(),
                message: format!("读取响应体失败: {error}"),
            })?;

        if body.len() as u64 > max_bytes {
            return Err(FetchError::TooLarge {
                url: url.to_owned(),
                limit: max_bytes,
            });
        }
        Ok(body)
    }
}

/// 把 ureq 的两类错误映射成可读的 [`FetchError`]。
///
/// `Status` 是「服务器答了，但不是 2xx」；`Transport` 是「根本没拿到响应」
/// （DNS / 连接拒绝 / TLS / 超时）。两者对用户的意义完全不同，不能混成一个错误。
fn map_error(url: &str, error: ureq::Error) -> FetchError {
    match error {
        ureq::Error::Status(status, _response) => FetchError::Status {
            status,
            url: url.to_owned(),
        },
        ureq::Error::Transport(transport) => FetchError::Network {
            url: url.to_owned(),
            message: transport.to_string(),
        },
    }
}

/// 读 `Content-Length`（可能缺失，也可能因为 gzip 而只是压缩后大小）。
fn content_length(response: &ureq::Response) -> Option<u64> {
    response.header("content-length")?.trim().parse().ok()
}
