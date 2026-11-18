//! 运行期配置。
//!
//! 全部来自宿主的 `[plugin.fengyegf.trending]` 段（`fuyubot.toml`）。
//!
//! ```toml
//! [plugin.fengyegf.trending]
//! trending_url = "https://github.com/trending" # 榜单页地址
//! width        = 1600    # 卡片宽度（px），夹到 1200–1920
//! timeout_ms   = 15000   # 抓取超时
//! max_items    = 15      # 最多渲染几条
//! avatar       = true    # 是否抓仓库作者头像（关掉用首字母兜底，出图更快）
//! avatar_timeout_ms = 6000  # 头像抓取的整体预算
//! prefixes     = ["/", "!", "！", ".", "#"]  # 不写就用缺省前缀
//! ```
//!
//! 读取顺序：`start()` 传进来的配置 JSON → `get_config("fengyegf.trending", "")` 兜底；
//! 两者缺什么就各自退回缺省值。命令前缀的第三层兜底是宿主顶层
//! `[config].PREFIXES`。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::Value;

/// 插件段名：`[plugin.fengyegf.trending]`。
pub const PLUGIN_NAME: &str = "fengyegf.trending";

/// 榜单页缺省地址（`?since=daily|weekly|monthly` 由抓取层拼）。
pub const DEFAULT_TRENDING_URL: &str = "https://github.com/trending";

/// 命令前缀缺省值，与宿主 `[config].PREFIXES` 的缺省一致。
const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// 卡片宽度缺省值：1600px 刚好排 4 列 372px 的卡片。
const DEFAULT_WIDTH: u32 = 1600;

/// 单次抓取（含连接 + 读取）的超时缺省值。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 最多渲染几条：GitHub 一页就是 15 条。
const DEFAULT_MAX_ITEMS: usize = 15;

/// 是否抓取仓库作者头像的缺省值：开（头像比首字母好看得多）。
const DEFAULT_AVATAR: bool = true;

/// 头像抓取的整体预算缺省值：15 个 40px 头像并发抓，6 秒足够。
const DEFAULT_AVATAR_TIMEOUT: Duration = Duration::from_secs(6);

/// 头像预算的允许范围：太短等于半途而废，太长会把 `/trending` 拖慢。
const MIN_AVATAR_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_AVATAR_TIMEOUT: Duration = Duration::from_secs(20);

/// 宽度下限：再窄 4 列每张只剩 ~272px，简介会挤成三行。
const MIN_WIDTH: u32 = 1200;
/// 宽度上限：与 html2img 的 `max_width` 默认值对齐。
const MAX_WIDTH: u32 = 1920;

/// 宽度夹取范围（`render` 与配置共用）。
pub const WIDTH_RANGE: std::ops::RangeInclusive<u32> = MIN_WIDTH..=MAX_WIDTH;

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 榜单页地址（不带查询串）。
    pub trending_url: String,
    /// 卡片宽度（px）。
    pub width: u32,
    /// 抓取超时。
    pub timeout: Duration,
    /// 最多渲染几条（1–30）。
    pub max_items: usize,
    /// 是否抓取仓库作者头像；关掉后卡片用首字母兜底，出图更快。
    pub avatar: bool,
    /// 头像抓取的整体预算（并发抓完为止）。
    pub avatar_timeout: Duration,
    /// 命令前缀。
    pub prefixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trending_url: DEFAULT_TRENDING_URL.to_owned(),
            width: DEFAULT_WIDTH,
            timeout: DEFAULT_TIMEOUT,
            max_items: DEFAULT_MAX_ITEMS,
            avatar: DEFAULT_AVATAR,
            avatar_timeout: DEFAULT_AVATAR_TIMEOUT,
            prefixes: DEFAULT_PREFIXES.iter().map(|p| (*p).to_owned()).collect(),
        }
    }
}

impl Config {
    /// 从宿主读配置；缺失或类型不对就退回缺省值并记一条日志。
    pub fn load(host: &HostApi, config_json: &str) -> Self {
        let mut config = Self::default();
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
            "trending: 配置 地址={} 宽度={}px 超时={}s 条数={} 前缀={:?}",
            config.trending_url,
            config.width,
            config.timeout.as_secs(),
            config.max_items,
            config.prefixes
        ));
        config
    }

    /// 应用一个配置对象（插件段）；返回「有没有显式写 prefixes」。
    fn apply(&mut self, value: &Value) -> bool {
        let mut prefixes_configured = false;

        if let Some(url) = pick_str(value, &["trending_url", "TRENDING_URL", "url"])
            && url.starts_with("http")
        {
            self.trending_url = url;
        }
        if let Some(width) = pick_u64(value, &["width", "WIDTH"])
            && width > 0
        {
            self.width = clamp_width(u32::try_from(width).unwrap_or(DEFAULT_WIDTH));
        }
        if let Some(timeout) = pick_u64(value, &["timeout_ms", "TIMEOUT_MS"])
            && timeout > 0
        {
            self.timeout = Duration::from_millis(timeout);
        }
        if let Some(max_items) = pick_u64(value, &["max_items", "MAX_ITEMS"])
            && max_items > 0
        {
            self.max_items = usize::try_from(max_items)
                .unwrap_or(DEFAULT_MAX_ITEMS)
                .clamp(1, 30);
        }
        if let Some(avatar) = pick_value(value, &["avatar", "AVATAR"]).and_then(Value::as_bool) {
            self.avatar = avatar;
        }
        if let Some(timeout) = pick_u64(value, &["avatar_timeout_ms", "AVATAR_TIMEOUT_MS"])
            && timeout > 0
        {
            self.avatar_timeout =
                Duration::from_millis(timeout).clamp(MIN_AVATAR_TIMEOUT, MAX_AVATAR_TIMEOUT);
        }
        if let Some(items) = pick_value(value, &["prefixes", "PREFIXES"]).and_then(Value::as_array)
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

/// 宽度夹取到 [`MIN_WIDTH`, `MAX_WIDTH`]。
pub fn clamp_width(width: u32) -> u32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

fn pick_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key))
}

fn pick_str(value: &Value, keys: &[&str]) -> Option<String> {
    pick_value(value, keys)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn pick_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    pick_value(value, keys).and_then(|value| match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(json: &str) -> Config {
        let mut config = Config::default();
        config.apply(&serde_json::from_str(json).unwrap());
        config
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let config = Config::default();
        assert_eq!(config.trending_url, DEFAULT_TRENDING_URL);
        assert_eq!(config.width, 1600);
        assert_eq!(config.max_items, 15);
        assert_eq!(config.prefixes, ["/", "!", "！", ".", "#"]);
    }

    #[test]
    fn width_is_clamped_to_the_renderer_range() {
        assert_eq!(apply(r#"{"width": 400}"#).width, MIN_WIDTH);
        assert_eq!(apply(r#"{"width": 9000}"#).width, MAX_WIDTH);
        assert_eq!(apply(r#"{"width": 1400}"#).width, 1400);
        assert_eq!(apply(r#"{"WIDTH": "1500"}"#).width, 1500);
    }

    #[test]
    fn prefixes_are_only_flagged_when_present() {
        let mut config = Config::default();
        let written = config.apply(&serde_json::from_str(r#"{"width": 1300}"#).unwrap());
        assert!(!written);
        let written = config.apply(&serde_json::from_str(r#"{"prefixes": [";"]}"#).unwrap());
        assert!(written);
        assert_eq!(config.prefixes, [";"]);
    }

    #[test]
    fn max_items_is_clamped() {
        assert_eq!(apply(r#"{"max_items": 0}"#).max_items, 15);
        assert_eq!(apply(r#"{"max_items": 999}"#).max_items, 30);
        assert_eq!(apply(r#"{"max_items": 12}"#).max_items, 12);
    }

    #[test]
    fn bad_types_fall_back_to_defaults() {
        let config = apply(r#"{"width": "wide", "timeout_ms": [], "url": 5}"#);
        assert_eq!(config.width, 1600);
        assert_eq!(config.timeout, DEFAULT_TIMEOUT);
        assert_eq!(config.trending_url, DEFAULT_TRENDING_URL);
    }
}
