//! 抓取 https://github.com/trending?since=... 的 HTML。
//!
//! GitHub 没有给 Trending 榜的公开 JSON 接口，只能抓页面；抓取只负责拿到 HTML，
//! 解析在 crate::core::parse。阻塞调用，只能在业务线程里跑。
//!
//! 请求带浏览器 UA 与 Accept-Language：GitHub 对空 UA / 脚本 UA 会直接返回 403，
//! 页面的语言文案也随 Accept-Language 变化（en-US 时增量是 stars today/week/month）。

use std::io::Read;
use std::time::Duration;

use crate::core::{Config, Period, TrendingItem};

/// 响应体上限：真实页面约 600 KiB，4 MiB 留足余量也避免异常响应撑爆内存。
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// 带上常见桌面浏览器 UA：GitHub 对非浏览器 UA 的 Trending 页可能 403。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// 连接超时上限：配置的读超时再长，建连也不该耗太久。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 抓取失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 连接 / 读取失败（超时、DNS、TLS）。
    #[error("请求 GitHub Trending 失败: {0}")]
    Http(String),
    /// 非 200 响应。
    #[error("GitHub Trending 返回 HTTP {0}")]
    Status(u16),
    /// 页面结构与预期不符（一条都没解析出来）。
    #[error("GitHub Trending 页面解析失败: {0}")]
    Parse(String),
}

/// 拼榜单页地址：{base}?since={since}。
///
/// base 允许已经带查询串（此时用 & 续接），末尾多余的 ? / & 会被去掉。
pub fn page_url(base: &str, period: Period) -> String {
    let base = base.trim().trim_end_matches(['?', '&']);
    let separator = if base.contains('?') { '&' } else { '?' };
    format!("{base}{separator}since={}", period.since())
}

/// 抓一页榜单并解析成条目。
pub fn fetch_trending(config: &Config, period: Period) -> Result<Vec<TrendingItem>, Error> {
    let url = page_url(&config.trending_url, period);

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT.min(config.timeout))
        .timeout(config.timeout)
        .build();

    let response = agent
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .set(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .set("Accept-Language", "en-US,en;q=0.9")
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(code, _) => Error::Status(code),
            other => Error::Http(other.to_string()),
        })?;

    let status = response.status();
    if status != 200 {
        return Err(Error::Status(status));
    }

    let mut body = String::new();
    response
        .into_reader()
        .take(MAX_BODY)
        .read_to_string(&mut body)
        .map_err(|error| Error::Http(error.to_string()))?;

    parse_body(&body)
}

/// 解析响应体；一条都没解析出来算抓取失败（多半是页面改版或被拦截）。
fn parse_body(body: &str) -> Result<Vec<TrendingItem>, Error> {
    let items = crate::core::parse::parse_items(body);
    if items.is_empty() {
        return Err(Error::Parse("页面里没有解析出任何仓库".to_owned()));
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_url_appends_since() {
        assert_eq!(
            page_url("https://github.com/trending", Period::Today),
            "https://github.com/trending?since=daily"
        );
        assert_eq!(
            page_url("https://github.com/trending", Period::Weekly),
            "https://github.com/trending?since=weekly"
        );
        assert_eq!(
            page_url("https://github.com/trending", Period::Monthly),
            "https://github.com/trending?since=monthly"
        );
    }

    #[test]
    fn page_url_handles_existing_query_and_trailing_separators() {
        assert_eq!(
            page_url("https://example.test/trending?language=rust", Period::Today),
            "https://example.test/trending?language=rust&since=daily"
        );
        assert_eq!(
            page_url("https://example.test/trending?", Period::Today),
            "https://example.test/trending?since=daily"
        );
        assert_eq!(
            page_url("  https://github.com/trending/  ", Period::Weekly),
            "https://github.com/trending/?since=weekly"
        );
    }

    #[test]
    fn empty_page_is_reported_as_parse_error() {
        let error = parse_body("").unwrap_err();
        assert!(matches!(error, Error::Parse(_)));
        assert!(
            error
                .to_string()
                .starts_with("GitHub Trending 页面解析失败")
        );
    }

    #[test]
    fn errors_render_readable_messages() {
        assert_eq!(
            Error::Status(403).to_string(),
            "GitHub Trending 返回 HTTP 403"
        );
        assert_eq!(
            Error::Http("timeout".to_owned()).to_string(),
            "请求 GitHub Trending 失败: timeout"
        );
    }
}
