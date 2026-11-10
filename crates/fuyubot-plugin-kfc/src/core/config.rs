//! 运行期配置。
//!
//! 全部来自宿主的 `[plugin.kfc]` 段（`fuyubot.toml`），键名同时接受 snake_case
//! 与 TS 版风格的大写写法；缺省值与 TS 版写死的常量一致。
//!
//! ```toml
//! [plugin.kfc]
//! api_url   = "https://kfc-crazy-thursday.vercel.app/api/index" # 文案来源
//! width     = 800        # 卡片宽度（px），夹到 460–1600
//! timeout_ms = 15000     # 拉文案的超时
//! prefixes  = ["/", "!", "！", ".", "#"]  # 不写就用缺省前缀
//! ```
//!
//! 读取顺序：`start()` 传进来的配置 JSON（宿主可能已经把本插件段序列化好了）→
//! `get_config("kfc", "")` 再读一次兜底；两者缺什么就各自退回缺省值。
//!
//! 命令前缀的第三层兜底是宿主顶层 `[config].PREFIXES`：那是 TS 宿主的写法，
//! Rust 宿主没有这个键，读到 `None` 就继续用缺省值——所以别在这里假设它一定存在。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::Value;

/// 插件段名：`[plugin.kfc]`。
pub const PLUGIN_NAME: &str = "kfc";

/// 文案接口。TS 版里写死在 `index.ts`。
pub const DEFAULT_API_URL: &str = "https://kfc-crazy-thursday.vercel.app/api/index";

/// 命令前缀缺省值，与宿主 `[config].PREFIXES` 的缺省一致。
const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// 卡片宽度缺省值，与 TS 版 `generateImage({ width: 800 })` 一致。
const DEFAULT_WIDTH: u32 = 800;

/// 拉文案的超时缺省值。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 宽度下限：再窄内容列就放不下「头像 + 昵称 + 蓝标 + 句柄」，
/// 名字行进入换行兜底后卡片底部还会多出空白带（实测 400–456）。
/// 与 `render` 的下限保持一致。
const MIN_WIDTH: u32 = 460;
/// 宽度上限：Telegram 再大也没必要，防止配置写错渲出巨图。
const MAX_WIDTH: u32 = 1600;

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 文案接口地址。
    pub api_url: String,
    /// 卡片宽度（px）。
    pub width: u32,
    /// 拉文案的超时。
    pub timeout: Duration,
    /// 命令前缀。
    pub prefixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_url: DEFAULT_API_URL.to_owned(),
            width: DEFAULT_WIDTH,
            timeout: DEFAULT_TIMEOUT,
            prefixes: DEFAULT_PREFIXES.iter().map(|p| (*p).to_owned()).collect(),
        }
    }
}

impl Config {
    /// 从宿主读配置；缺失或类型不对就退回缺省值并记一条日志。
    pub fn load(host: &HostApi, config_json: &str) -> Self {
        let mut config = Self::default();

        // 宿主在 start() 里给的那份，通常就是本插件段；老宿主可能给空串。
        if let Ok(value) = serde_json::from_str::<Value>(config_json)
            && value.is_object()
        {
            config.apply(&value);
        }
        // 再走一遍配置读接口兜底（start 传空串时就需要它）。
        if let Some(value) = host.config_value(PLUGIN_NAME, "") {
            config.apply(&value);
        }

        // 前缀：插件段里写了就用插件的，否则跟随宿主顶层 [config].PREFIXES，
        // 与 TS 版「沿用宿主命令前缀」的行为一致。
        if config.prefixes.is_empty()
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
            "kfc: 配置 api={} 宽度={}px 超时={}s 前缀={:?}",
            config.api_url,
            config.width,
            config.timeout.as_secs(),
            config.prefixes
        ));
        config
    }

    /// 把一段配置对象合进当前值；类型不对的键忽略。
    fn apply(&mut self, section: &Value) {
        if let Some(url) = string(section, &["api_url", "API_URL"])
            && url.trim().starts_with("http")
        {
            self.api_url = url.trim().to_owned();
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
            }
        }
    }
}

/// 取字符串键，兼容 `"16"` 这种数字字符串。
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn applies_snake_case_keys() {
        let mut config = Config::default();
        config.apply(&json!({
            "api_url": "https://example.com/kfc",
            "width": 900,
            "timeout_ms": 5000,
            "prefixes": ["/", "kfc!"],
        }));
        assert_eq!(config.api_url, "https://example.com/kfc");
        assert_eq!(config.width, 900);
        assert_eq!(config.timeout, Duration::from_millis(5000));
        assert_eq!(config.prefixes, vec!["/".to_owned(), "kfc!".to_owned()]);
    }

    #[test]
    fn accepts_legacy_uppercase_and_clamps_width() {
        let mut config = Config::default();
        config.apply(&json!({ "WIDTH": 99999, "TIMEOUT_MS": "1000" }));
        assert_eq!(config.width, MAX_WIDTH);
        assert_eq!(config.timeout, Duration::from_millis(1000));
    }

    #[test]
    fn ignores_bad_values() {
        let mut config = Config::default();
        config.apply(&json!({ "api_url": "", "width": "abc", "prefixes": [] }));
        assert_eq!(config.api_url, DEFAULT_API_URL);
        assert_eq!(config.width, DEFAULT_WIDTH);
        assert_eq!(config.prefixes.len(), DEFAULT_PREFIXES.len());
    }
}
