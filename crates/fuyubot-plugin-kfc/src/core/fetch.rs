//! 拉文案：一次阻塞式 HTTP GET，返回纯文本。
//!
//! 接口返回的是 `text/html`，但内容是纯文本（几行诗），TS 版用 axios 直接取字符串。
//! 这里同样不解析 HTML，只把响应体当 UTF-8 文本读出来。
//!
//! 只在自己的业务线程里调用：`ureq` 是同步的，宿主回调里调会卡住 TDLib 接收线程。

use std::io::Read;
use std::time::Duration;

/// 响应体上限：接口正常只回几 KB，超过就当异常，避免被异常响应撑爆内存。
const MAX_BODY: u64 = 256 * 1024;

/// 取文案失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// HTTP 状态码非 2xx。
    #[error("接口返回 {0}")]
    Status(u16),
    /// 网络 / 超时 / 连接错误。
    #[error("请求失败: {0}")]
    Transport(String),
    /// 响应体读不出来或不是合法 UTF-8。
    #[error("响应体读取失败: {0}")]
    Body(String),
}

/// 拉取一句肯德基文案。
pub fn fetch_content(api_url: &str, timeout: Duration) -> Result<String, Error> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10).min(timeout))
        .timeout(timeout)
        .build();

    let response = agent
        .get(api_url)
        // 接口按 UA / Accept 返回不同内容，明确要文本即可。
        .set("Accept", "text/plain, text/html;q=0.9, */*;q=0.8")
        .set("User-Agent", "fuyubot-kfc/0.1 (+rust)")
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(code, _) => Error::Status(code),
            other => Error::Transport(other.to_string()),
        })?;

    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(Error::Status(status));
    }

    let mut body = String::new();
    response
        .into_reader()
        .take(MAX_BODY)
        .read_to_string(&mut body)
        .map_err(|error| Error::Body(error.to_string()))?;

    Ok(body)
}
