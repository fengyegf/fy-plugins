//! 网页抓取：DMM / DLSite 作品页。
//!
//! 只做「把页面拿回来」这一件事：不解析、不发消息。
//!
//! DMM 作品页会先跳年龄确认：这里直接带上 DMM 的 `age_check_done` Cookie，
//! 否则拿到的永远是确认页而不是作品页（ureq 2 默认不存 Cookie）。

use std::io::Read;
use std::time::Duration;

use url::Url;

/// 一次抓取的结果。
pub struct Page {
    /// 页面 HTML。
    pub html: String,
    /// 跟随重定向后的最终地址（DMM 的年龄确认会把地址改掉）。
    pub url: String,
}

/// DMM 在年龄确认页写入的 Cookie，值固定为 `1`。
///
/// ureq 2 默认没有 Cookie 罐（`cookie_store` 要开 `cookies` 特性），
/// 不带这个标记请求会被 302 弹回 `age_check`，抓到的永远不是作品页。
const DMM_AGE_COOKIE: &str = "age_check_done=1";

/// 页面请求超时。
const PAGE_TIMEOUT: Duration = Duration::from_secs(90);
/// 图片只用来当封面发出去，Telegram 的照片上限是 10 MB。
const IMAGE_LIMIT: u64 = 10 * 1024 * 1024;

/// 抓取页面。`proxy` 形如 `http://user:pass@host:port`，`None` 直连。
pub fn fetch(url: &str, proxy: Option<&str>) -> Result<Page, String> {
    let agent = agent(proxy)?;
    let mut request = agent.get(url);
    // DMM 的年龄确认用 Cookie 记住：直接带上标记，免得被弹回确认页。
    if is_dmm(url) {
        request = request.set("Cookie", DMM_AGE_COOKIE);
    }

    let response = request
        .call()
        .map_err(|error| format!("请求 {url} 失败: {error}"))?;
    let final_url = response.get_url().to_owned();
    let html = response
        .into_string()
        .map_err(|error| format!("读取页面内容失败: {error}"))?;
    Ok(Page {
        html,
        url: final_url,
    })
}

/// 抓取一张图片的原始字节。
pub fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
    let response = agent(None)?
        .get(url)
        .call()
        .map_err(|error| format!("请求 {url} 失败: {error}"))?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(IMAGE_LIMIT)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取图片失败: {error}"))?;
    Ok(bytes)
}

fn agent(proxy: Option<&str>) -> Result<ureq::Agent, String> {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout(PAGE_TIMEOUT);
    if let Some(proxy) = proxy {
        builder = builder.proxy(ureq::Proxy::new(proxy).map_err(|e| format!("代理地址无效: {e}"))?);
    }
    Ok(builder.build())
}

/// 链接是否指向 DMM（`dmm.co.jp` / `dmm.com` 及其子域）。
fn is_dmm(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };

    ["dmm.co.jp", "dmm.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}
