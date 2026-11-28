//! 插件配置：全部来自宿主 `[plugin.fengyegf.musiccard]` 段。
//!
//! ```toml
//! [plugin.fengyegf.musiccard]
//! request_timeout_ms = 15000              # 普通接口超时（秒级精度，缺省 15s）
//! ```
//!
//! 任何一项缺失或类型不对都退回缺省值，不让插件因为配置写错就起不来。
//!
//! 卡片不再落盘到本地（渲染产物由 html2img 服务写在它自己的临时目录），所以这里
//! 没有临时目录配置；老配置里残留的 `temp_dir` 会被安静忽略。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Map, Value};

/// 插件段名：`[plugin.fengyegf.musiccard]`。
pub const PLUGIN_NAME: &str = "fengyegf.musiccard";

/// 普通接口的缺省超时。
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 普通接口超时。
    pub request_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

impl Config {
    /// 从宿主读取配置；缺失或类型不对的键退回缺省值。
    pub fn load(host: &HostApi) -> Self {
        let mut config = Self::default();

        // `[plugin.fengyegf.musiccard]` 整段：键名同时接受 snake_case 与大写写法。
        let section = host
            .config_value(PLUGIN_NAME, "")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();

        if let Some(ms) = number(&section, &["request_timeout_ms", "REQUEST_TIMEOUT_MS"]) {
            config.request_timeout = Duration::from_millis(ms.max(1.0) as u64);
        }
        host.info(&format!(
            "musiccard: 配置 超时={}s",
            config.request_timeout.as_secs()
        ));
        config
    }
}

/// 按候选键名依次取值，命中第一个能转成数字的。
fn number(section: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        value.as_f64().or_else(|| value.as_u64().map(|n| n as f64))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values() {
        let config = Config::default();
        assert_eq!(config.request_timeout, Duration::from_secs(15));
    }
}
