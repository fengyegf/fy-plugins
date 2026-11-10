//! whois 接口访问：两次阻塞式 HTTP GET（whois 主体 + IP 归属）。
//!
//! 只在自己的业务线程里调用：`ureq` 是同步的，宿主回调里调会卡住 TDLib 接收线程。
//!
//! 接口形态（whoiscx.com）：
//!
//! ```json
//! // 成功
//! {"status":1,"data":{"is_available":0,"domain":"example.com", ...,"raw":"..."}}
//! // 失败
//! {"status":0,"error":"域名不合法","req_id":"..."}
//! ```
//!
//! `status != 1` 统一归为 [`Error::Api`]，把接口给的中文原因原样带回命令层展示。

use std::io::Read;
use std::time::Duration;

use serde_json::Value;

use crate::core::Config;
use crate::core::model::{IpInfo, WhoisData};

/// 响应体上限：whois 原始文本通常十几 KB，1 MiB 足够，也避免异常响应撑爆内存。
const MAX_BODY: u64 = 1024 * 1024;

/// 查询失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 接口明确返回业务错误（`status != 1`），例如「域名不合法」。
    #[error("{0}")]
    Api(String),
    /// HTTP 状态码非 2xx。
    #[error("接口返回 HTTP {0}")]
    Status(u16),
    /// 网络 / 超时 / 连接错误。
    #[error("请求失败: {0}")]
    Transport(String),
    /// 响应体读不出来。
    #[error("响应体读取失败: {0}")]
    Body(String),
    /// 响应不是合法 JSON 或缺少必要字段。
    #[error("响应解析失败: {0}")]
    Parse(String),
}

/// Whois 查询结果。只有已注册域名需要携带完整数据用于出图。
#[derive(Debug, Clone)]
pub enum WhoisQuery {
    Registered(Box<WhoisData>),
    Available,
}

/// 查询域名 whois 信息。
pub fn query_whois(config: &Config, domain: &str) -> Result<WhoisQuery, Error> {
    let url = endpoint(&config.whois_api_url, domain, true);
    let payload = get_json(&url, config.timeout)?;
    classify(&payload)
}

/// 查询域名解析到的 IP 归属；失败返回 `None`，不阻断卡片主体。
pub fn query_ip(config: &Config, domain: &str) -> Option<IpInfo> {
    let url = endpoint(&config.ip_api_url, domain, false);
    let payload = get_json(&url, config.timeout).ok()?;
    if integer(payload.get("status")) != 1 {
        return None;
    }
    IpInfo::from_api(payload.get("data")?)
}

/// 把接口信封解析成结果；独立出来是为了能离线单测。
fn classify(payload: &Value) -> Result<WhoisQuery, Error> {
    if integer(payload.get("status")) != 1 {
        let reason = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("接口返回异常");
        return Err(Error::Api(reason.to_owned()));
    }
    let data = payload
        .get("data")
        .ok_or_else(|| Error::Parse("响应缺少 data".to_owned()))?;
    let whois =
        WhoisData::from_api(data).ok_or_else(|| Error::Parse("data 不是对象".to_owned()))?;
    Ok(if whois.is_available == 1 {
        WhoisQuery::Available
    } else {
        WhoisQuery::Registered(Box::new(whois))
    })
}

/// 发一次 GET 并解析 JSON。
fn get_json(url: &str, timeout: Duration) -> Result<Value, Error> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10).min(timeout))
        .timeout(timeout)
        .build();

    let response = agent
        .get(url)
        .set("Accept", "application/json, */*;q=0.8")
        .set("User-Agent", "fuyubot-whois/0.1 (+rust)")
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

    serde_json::from_str(&body).map_err(|error| Error::Parse(error.to_string()))
}

/// 拼接查询地址：`base` 允许已经带查询串（此时用 `&` 续接）。
fn endpoint(base: &str, domain: &str, raw: bool) -> String {
    let base = base.trim().trim_end_matches(['?', '&']);
    let separator = if base.contains('?') { '&' } else { '?' };
    let mut url = format!("{base}{separator}domain={}", encode_component(domain));
    if raw {
        url.push_str("&raw=1");
    }
    url
}

/// RFC 3986 的 percent-encoding：不做 `+` 替换，中文域名按 UTF-8 字节编码。
fn encode_component(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// 取整数；兼容数字与数字字符串。
fn integer(value: Option<&Value>) -> i64 {
    let Some(value) = value else {
        return 0;
    };
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .unwrap_or(0)
}
