//! 网页抓取：DMM / DLSite 作品页。
//!
//! 只做「把页面拿回来」这一件事：不解析、不发消息。
//!
//! DMM 作品页会先跳年龄确认，这里把地址改写成 `declared=yes` 的年龄确认页——
//! 服务器顺手写好年龄 Cookie 再跳回原链接，否则拿到的永远是确认页而不是作品页。

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

/// 站点 → 年龄确认页前缀（含 `declared=yes`，由服务器写入年龄 Cookie）。
const DMM_AGE_CHECK: [(&str, &str); 2] = [
    (
        "dmm.co.jp",
        "https://www.dmm.co.jp/age_check/=/declared=yes/?rurl=",
    ),
    (
        "dmm.com",
        "https://www.dmm.com/age_check/=/declared=yes/?rurl=",
    ),
];

/// 页面请求超时。
const PAGE_TIMEOUT: Duration = Duration::from_secs(90);
/// 图片只用来当封面发出去，Telegram 的照片上限是 10 MB。
const IMAGE_LIMIT: u64 = 10 * 1024 * 1024;

/// 抓取页面。`proxy` 形如 `http://user:pass@host:port`，`None` 直连。
pub fn fetch(url: &str, proxy: Option<&str>) -> Result<Page, String> {
    let agent = agent(proxy)?;
    let target = age_check_url(url);

    let response = agent
        .get(&target)
        .call()
        .map_err(|error| format!("请求 {target} 失败: {error}"))?;
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

/// DMM 链接改写成年龄确认地址；其它站点、或本身已是确认页时原样返回。
fn age_check_url(url: &str) -> String {
    if url.contains("/age_check/") {
        return url.to_owned();
    }
    let Ok(parsed) = Url::parse(url) else {
        return url.to_owned();
    };
    let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else {
        return url.to_owned();
    };

    DMM_AGE_CHECK
        .iter()
        .find(|(domain, _)| host == *domain || host.ends_with(&format!(".{domain}")))
        .map(|(_, prefix)| format!("{prefix}{}", encode(url)))
        .unwrap_or_else(|| url.to_owned())
}

/// 百分号编码。
fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}
