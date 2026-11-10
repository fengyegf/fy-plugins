//! 插件配置：全部来自宿主 `[plugin.musiccard]` 段与顶层 `[config]`。
//!
//! ```toml
//! [plugin.musiccard]
//! temp_dir = "C:/Temp/fuyubot-musiccard"  # 卡片原图的临时目录，缺省放系统临时目录
//! request_timeout_ms = 15000              # 普通接口超时（秒级精度，缺省 15s）
//!
//! [config]
//! PREFIXES = ["/", "!", "！", ".", "#"]
//! ```
//!
//! 任何一项缺失或类型不对都退回缺省值，不让插件因为配置写错就起不来。

use std::path::PathBuf;
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Map, Value};

/// 插件段名：`[plugin.musiccard]`。
pub const PLUGIN_NAME: &str = "musiccard";

/// 默认命令前缀。
const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// 临时目录的缺省名：`<系统临时目录>/fuyubot-musiccard`。
pub const DEFAULT_TEMP_DIR_NAME: &str = "fuyubot-musiccard";

/// 系统临时目录不可写时的备用目录（相对宿主工作目录）。
pub const FALLBACK_TEMP_DIR_NAME: &str = ".musiccard-tmp";

/// 普通接口的缺省超时。
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 命令前缀列表。
    pub prefixes: Vec<String>,
    /// 生成卡片时的临时目录。
    pub temp_dir: PathBuf,
    /// 普通接口超时。
    pub request_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefixes: DEFAULT_PREFIXES.iter().map(|p| p.to_string()).collect(),
            temp_dir: std::env::temp_dir().join(DEFAULT_TEMP_DIR_NAME),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

impl Config {
    /// 从宿主读取配置；缺失或类型不对的键退回缺省值。
    pub fn load(host: &HostApi) -> Self {
        let mut config = Self::default();

        // `[plugin.musiccard]` 整段：键名同时接受 snake_case 与大写写法。
        let section = host
            .config_value(PLUGIN_NAME, "")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();

        if let Some(dir) = text(&section, &["temp_dir", "TEMP_DIR"]) {
            let dir = dir.trim();
            // 显式写空串视同没写：空路径没法写文件。
            if !dir.is_empty() {
                config.temp_dir = PathBuf::from(dir);
            }
        }
        if let Some(ms) = number(&section, &["request_timeout_ms", "REQUEST_TIMEOUT_MS"]) {
            config.request_timeout = Duration::from_millis(ms.max(1.0) as u64);
        }
        // 前缀位于宿主的顶层 `[config]` 段。
        if let Some(prefixes) = prefixes(host) {
            config.prefixes = prefixes;
        }

        // 临时目录先建出来：命令线程里再建一次也不贵，但这里能提前把权限问题写进日志。
        // 受限环境（宿主被沙箱 / 低完整性令牌拉起来）里系统临时目录可能不允许新建，
        // 这时退到工作目录下的 `.musiccard-tmp`，免得整条命令到落盘时才失败。
        if let Err(error) = std::fs::create_dir_all(&config.temp_dir) {
            let configured = config.temp_dir.clone();
            let fallback = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(FALLBACK_TEMP_DIR_NAME);
            match std::fs::create_dir_all(&fallback) {
                Ok(()) => {
                    config.temp_dir = fallback;
                    host.warn(&format!(
                        "musiccard: 临时目录 {} 不可用（{error}），改用 {}",
                        configured.display(),
                        config.temp_dir.display()
                    ));
                }
                Err(fallback_error) => {
                    host.warn(&format!(
                        "musiccard: 创建临时目录 {} 失败（{error}）；备用目录 {} 也失败：{fallback_error}",
                        configured.display(),
                        fallback.display()
                    ));
                }
            }
        }

        host.info(&format!(
            "musiccard: 配置 前缀={:?} 临时目录={} 超时={}s",
            config.prefixes,
            config.temp_dir.display(),
            config.request_timeout.as_secs()
        ));
        config
    }
}

/// 从顶层 `[config]` 读命令前缀；缺失或为空返回 `None`（调用方保留缺省值）。
fn prefixes(host: &HostApi) -> Option<Vec<String>> {
    let value = host
        .config_value("", "config.PREFIXES")
        .or_else(|| host.config_value("", "config.prefixes"))?;
    let list: Vec<String> = value
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(|prefix| prefix.trim().to_owned())
        .filter(|prefix| !prefix.is_empty())
        .collect();
    (!list.is_empty()).then_some(list)
}

/// 按候选键名依次取值，命中第一个能转成数字的。
fn number(section: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        value.as_f64().or_else(|| value.as_u64().map(|n| n as f64))
    })
}

/// 按候选键名依次取字符串值。
fn text(section: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| section.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}
