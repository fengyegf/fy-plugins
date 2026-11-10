//! whois 接口的数据模型。
//!
//! 接口返回的 JSON 有两个坑：
//!
//! 1. `status` / `is_available` 等字段有时是数字、有时是字符串，不能直接 `as_i64`；
//! 2. 失败时 `data` 缺失、`fields` 可能是对象也可能是空数组，所以 `data` 一律按
//!    [`serde_json::Value`] 宽松解析，缺字段就用空串 / 0 兜底，绝不 panic。
//!
//! 这里不依赖 `serde` 的 derive，纯手写提取：接口字段名不做重命名映射，
//! 也方便给缺失字段留默认值。

use serde_json::Value;

/// `data.info`：whois 主体信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WhoisInfo {
    pub domain: String,
    pub registrant_name: String,
    pub registrant_email: String,
    pub registrar_name: String,
    pub creation_time: String,
    pub expiration_time: String,
    pub creation_days: i64,
    pub valid_days: i64,
    pub is_expire: i64,
    pub domain_status: Vec<String>,
    pub name_server: Vec<String>,
    pub whois_server: String,
}

/// `data`：一次 whois 查询的主体。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WhoisData {
    pub is_available: i64,
    pub domain: String,
    pub domain_suffix: String,
    pub query_time: String,
    pub info: WhoisInfo,
    /// 接口顶层偶尔直接带 IP 归属字段，作为 IP 接口失败时的兜底。
    pub ip: String,
    pub country: String,
    pub city: String,
    pub area: String,
    pub isp: String,
    /// 原始 whois 文本（接口 `raw=1`）。卡片不展示，留给以后 `/whois raw`。
    pub raw: String,
}

/// `/api/whois/ip/` 的 `data`：域名解析到的服务器归属。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IpInfo {
    pub domain: String,
    pub ip: String,
    pub country: String,
    pub city: String,
    pub district: String,
    pub isp: String,
    pub area: String,
}

/// 渲染卡片用的最终数据：whois + IP 归属合并后的结果。
///
/// 卡片模板使用的完整数据。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardData {
    pub is_available: i64,
    pub domain: String,
    pub domain_suffix: String,
    pub query_time: String,
    pub registrant_name: String,
    pub registrant_email: String,
    pub registrar_name: String,
    pub creation_time: String,
    pub expiration_time: String,
    pub creation_days: i64,
    pub valid_days: i64,
    pub is_expire: i64,
    pub domain_status: Vec<String>,
    pub name_server: Vec<String>,
    pub whois_server: String,
    pub raw: String,
    pub ip: String,
    pub country: String,
    pub city: String,
    pub area: String,
    pub isp: String,
}

impl CardData {
    /// 合并 whois 与 IP 归属：IP 接口优先，失败时退回 whois 顶层的同名字段。
    ///
    /// IP 接口字段优先，缺失时使用 Whois 数据中的同名字段。
    pub fn new(whois: &WhoisData, ip: Option<&IpInfo>) -> Self {
        let pick = |from_ip: Option<&String>, fallback: &str| -> String {
            from_ip
                .filter(|value| !value.is_empty())
                .cloned()
                .unwrap_or_else(|| fallback.to_owned())
        };

        Self {
            is_available: whois.is_available,
            domain: whois.domain.clone(),
            domain_suffix: whois.domain_suffix.clone(),
            query_time: whois.query_time.clone(),
            registrant_name: whois.info.registrant_name.clone(),
            registrant_email: whois.info.registrant_email.clone(),
            registrar_name: whois.info.registrar_name.clone(),
            creation_time: whois.info.creation_time.clone(),
            expiration_time: whois.info.expiration_time.clone(),
            creation_days: whois.info.creation_days,
            valid_days: whois.info.valid_days,
            is_expire: whois.info.is_expire,
            domain_status: whois.info.domain_status.clone(),
            name_server: whois.info.name_server.clone(),
            whois_server: whois.info.whois_server.clone(),
            raw: whois.raw.clone(),
            ip: pick(ip.map(|info| &info.ip), &whois.ip),
            country: pick(ip.map(|info| &info.country), &whois.country),
            city: pick(ip.map(|info| &info.city), &whois.city),
            area: pick(ip.map(|info| &info.area), &whois.area),
            isp: pick(ip.map(|info| &info.isp), &whois.isp),
        }
    }

    /// 卡片状态徽标文案：`is_expire == 0` 时看是否注册；否则看是否过期。
    pub fn status_label(&self) -> &'static str {
        match (self.is_expire, self.is_available) {
            (0, 0) => "已注册",
            (0, _) => "未注册",
            (1, _) => "已过期",
            (_, _) => "未过期",
        }
    }

    /// 徽标是否需要使用危险状态配色。
    pub fn status_is_danger(&self) -> bool {
        self.is_expire != 0
    }
}

impl WhoisData {
    /// 从接口 `data` 对象解析；不是对象就返回 `None`。
    pub fn from_api(data: &Value) -> Option<Self> {
        let object = data.as_object()?;
        Some(Self {
            is_available: integer(object.get("is_available")),
            domain: string(object.get("domain")),
            domain_suffix: string(object.get("domain_suffix")),
            query_time: string(object.get("query_time")),
            info: WhoisInfo::from_api(object.get("info").unwrap_or(&Value::Null)),
            ip: string(object.get("ip")),
            country: string(object.get("country")),
            city: string(object.get("city")),
            area: string(object.get("area")),
            isp: string(object.get("isp")),
            raw: string(object.get("raw")),
        })
    }
}

impl WhoisInfo {
    fn from_api(value: &Value) -> Self {
        let object = value.as_object();
        let get = |key: &str| object.and_then(|map| map.get(key));
        Self {
            domain: string(get("domain")),
            registrant_name: string(get("registrant_name")),
            registrant_email: string(get("registrant_email")),
            registrar_name: string(get("registrar_name")),
            creation_time: string(get("creation_time")),
            expiration_time: string(get("expiration_time")),
            creation_days: integer(get("creation_days")),
            valid_days: integer(get("valid_days")),
            is_expire: integer(get("is_expire")),
            domain_status: string_list(get("domain_status")),
            name_server: string_list(get("name_server")),
            whois_server: string(get("whois_server")),
        }
    }
}

impl IpInfo {
    /// 从 `/api/whois/ip/` 的 `data` 解析；不是对象就返回 `None`。
    pub fn from_api(data: &Value) -> Option<Self> {
        let object = data.as_object()?;
        Some(Self {
            domain: string(object.get("domain")),
            ip: string(object.get("ip")),
            country: string(object.get("country")),
            city: string(object.get("city")),
            district: string(object.get("district")),
            isp: string(object.get("isp")),
            area: string(object.get("area")),
        })
    }
}

/// 取字符串；缺失 / `null` / 非字符串一律空串。
fn string(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// 取整数；兼容数字、浮点、数字字符串与缺失。
fn integer(value: Option<&Value>) -> i64 {
    let Some(value) = value else {
        return 0;
    };
    if let Some(number) = value.as_i64() {
        return number;
    }
    if let Some(number) = value.as_f64() {
        return number as i64;
    }
    value
        .as_str()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// 取字符串数组；非数组 / 非字符串元素直接跳过。
fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
