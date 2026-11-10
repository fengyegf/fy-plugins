//! 运行期配置。
//!
//! 全部来自宿主的 `[plugin.whois]` 段（`fuyubot.toml`），键名同时接受 snake_case
//! 与大写写法。
//!
//! ```toml
//! [plugin.whois]
//! whois_api_url = "https://whoiscx.com/api/whois/info/" # whois 接口
//! ip_api_url    = "https://whoiscx.com/api/whois/ip/"   # IP 归属接口
//! width         = 1200   # 卡片宽度（px），夹到 900–1600
//! timeout_ms    = 15000  # 单次查询超时
//! favicon       = true   # 是否抓取网站图标
//! favicon_timeout_ms = 6000  # 抓取图标的整体预算
//! prefixes      = ["/", "!", "！", ".", "#"]  # 不写就用缺省前缀
//! ```
//!
//! 读取顺序：`start()` 传进来的配置 JSON → `get_config("whois", "")` 兜底；
//! 两者缺什么就各自退回缺省值。命令前缀的第三层兜底是宿主顶层
//! `[config].PREFIXES`；没有这个键时继续用缺省值。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::Value;

/// 插件段名：`[plugin.whois]`。
pub const PLUGIN_NAME: &str = "whois";

/// Whois 查询接口的缺省地址。
pub const DEFAULT_WHOIS_API_URL: &str = "https://whoiscx.com/api/whois/info/";

/// IP 归属查询接口的缺省地址。
pub const DEFAULT_IP_API_URL: &str = "https://whoiscx.com/api/whois/ip/";

/// 命令前缀缺省值，与宿主 `[config].PREFIXES` 的缺省一致。
const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// 卡片宽度缺省值。
const DEFAULT_WIDTH: u32 = 1200;

/// 单次查询超时缺省值。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 是否抓取网站图标的缺省值。
const DEFAULT_FAVICON: bool = true;

/// 抓取网站图标的整体预算缺省值。
const DEFAULT_FAVICON_TIMEOUT: Duration = Duration::from_secs(6);

/// 图标预算的允许范围：太短等于关掉功能，太长会把 `/whois` 拖慢。
const MIN_FAVICON_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_FAVICON_TIMEOUT: Duration = Duration::from_secs(30);

/// 宽度下限：再窄两列信息框就挤成一列，status 文本也会大量换行。
/// 与 `render` 的下限保持一致。
const MIN_WIDTH: u32 = 900;
/// 宽度上限：Telegram 再大也没必要，防止配置写错渲出巨图。
const MAX_WIDTH: u32 = 1600;

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// whois 接口地址（不带查询串；`domain` 参数由 `fetch` 拼接）。
    pub whois_api_url: String,
    /// IP 归属接口地址。
    pub ip_api_url: String,
    /// 卡片宽度（px）。
    pub width: u32,
    /// 单次查询（含连接 + 读取）的超时。
    pub timeout: Duration,
    /// 是否抓取网站图标；关掉后卡片固定使用内置图标。
    pub favicon: bool,
    /// 抓取网站图标的整体预算（多次尝试合计）。
    pub favicon_timeout: Duration,
    /// 命令前缀。
    pub prefixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            whois_api_url: DEFAULT_WHOIS_API_URL.to_owned(),
            ip_api_url: DEFAULT_IP_API_URL.to_owned(),
            width: DEFAULT_WIDTH,
            timeout: DEFAULT_TIMEOUT,
            favicon: DEFAULT_FAVICON,
            favicon_timeout: DEFAULT_FAVICON_TIMEOUT,
            prefixes: DEFAULT_PREFIXES.iter().map(|p| (*p).to_owned()).collect(),
        }
    }
}

impl Config {
    /// 从宿主读配置；缺失或类型不对就退回缺省值并记一条日志。
    pub fn load(host: &HostApi, config_json: &str) -> Self {
        let mut config = Self::default();
        // 记录「插件段里到底有没有显式写 prefixes」。只有没写时才允许回落到
        // 宿主顶层 `[config].PREFIXES`；否则 default() 里的缺省列表会让回落永远不生效。
        let mut prefixes_configured = false;

        if let Ok(value) = serde_json::from_str::<Value>(config_json)
            && value.is_object()
        {
            prefixes_configured |= config.apply(&value);
        }
        // 再走一遍配置读接口兜底（start 传空串时就需要它）。
        if let Some(value) = host.config_value(PLUGIN_NAME, "") {
            prefixes_configured |= config.apply(&value);
        }

        // 前缀：插件段里没写就跟随宿主顶层 `[config].PREFIXES`。
        if !prefixes_configured
            && let Some(value) = host
                .config_value("", "config.PREFIXES")
                .or_else(|| host.config_value("", "config.prefixes"))
        {
            let list: Vec<String> = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|prefix| !prefix.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !list.is_empty() {
                config.prefixes = list;
            }
        }

        host.info(&format!(
            "whois: 配置 whois_api={} ip_api={} 宽度={}px 超时={}s 图标={} 图标预算={}ms 前缀={:?}",
            config.whois_api_url,
            config.ip_api_url,
            config.width,
            config.timeout.as_secs(),
            if config.favicon { "开" } else { "关" },
            config.favicon_timeout.as_millis(),
            config.prefixes
        ));
        config
    }

    /// 把一段配置对象合进当前值；类型不对的键忽略。
    ///
    /// 返回 `true` 表示这段配置显式设置了 `prefixes`（非空列表）。
    fn apply(&mut self, section: &Value) -> bool {
        let mut prefixes_configured = false;
        if let Some(url) = string(section, &["whois_api_url", "WHOIS_API_URL"])
            && url.trim().starts_with("http")
        {
            self.whois_api_url = url.trim().to_owned();
        }
        if let Some(url) = string(section, &["ip_api_url", "IP_API_URL"])
            && url.trim().starts_with("http")
        {
            self.ip_api_url = url.trim().to_owned();
        }
        if let Some(width) = number(section, &["width", "WIDTH"]) {
            self.width = (width as u32).clamp(MIN_WIDTH, MAX_WIDTH);
        }
        // 兼容两种写法：timeout_ms（毫秒）优先，其次 timeout（秒）。
        if let Some(ms) = number(section, &["timeout_ms", "TIMEOUT_MS"]) {
            self.timeout = Duration::from_millis(ms as u64);
        } else if let Some(seconds) = number(section, &["timeout", "TIMEOUT"]) {
            self.timeout = Duration::from_secs(seconds as u64);
        }
        if let Some(enabled) = boolean(section, &["favicon", "FAVICON"]) {
            self.favicon = enabled;
        }
        if let Some(ms) = number(section, &["favicon_timeout_ms", "FAVICON_TIMEOUT_MS"]) {
            self.favicon_timeout = Duration::from_millis(ms.max(0.0) as u64)
                .clamp(MIN_FAVICON_TIMEOUT, MAX_FAVICON_TIMEOUT);
        }
        if let Some(items) = section
            .get("prefixes")
            .or_else(|| section.get("PREFIXES"))
            .and_then(Value::as_array)
        {
            let list: Vec<String> = items
                .iter()
                .filter_map(Value::as_str)
                .filter(|prefix| !prefix.is_empty())
                .map(str::to_owned)
                .collect();
            if !list.is_empty() {
                self.prefixes = list;
                prefixes_configured = true;
            }
        }
        prefixes_configured
    }
}

/// 取字符串键，兼容数字写法的值。
fn string(section: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        if let Some(text) = value.as_str() {
            return Some(text.to_owned());
        }
        value.as_f64().map(|number| number.to_string())
    })
}

/// 取数字键，兼容字符串写法的数字。
fn number(section: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

/// 取布尔键，兼容 JSON 布尔与 `"true"` / `"1"` / `"on"` 这类字符串写法。
fn boolean(section: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        if let Some(flag) = value.as_bool() {
            return Some(flag);
        }
        match value.as_str()?.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "on" | "yes" | "是" => Some(true),
            "false" | "0" | "off" | "no" | "否" => Some(false),
            _ => None,
        }
    })
}
