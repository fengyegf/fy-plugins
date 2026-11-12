//! 插件配置。
//!
//! 全部来自宿主的 `[plugin.epic]` 段（`fuyubot.toml`），命令前缀复用顶层
//! `[config] PREFIXES`。
//!
//! ```toml
//! [plugin.epic]
//! locale = "zh-CN"                    # 接口语言
//! country = "CN"                      # 接口国家
//! timezone_offset_hours = 8           # 日期换算时区（北京时间）
//! api_url = "https://…/freeGamesPromotions"  # 促销接口地址（可换区域/镜像）
//! user_agent = "Mozilla/5.0 …"        # 请求 UA
//! proxy = ""                          # 形如 http://127.0.0.1:7890，留空直连
//! timeout_ms = 20000                  # 单次 HTTP 超时
//! scale = 2.0                         # 输出缩放（2.0 = 2 倍图）
//! max_per_row = 4                     # 每行卡片数
//! max_cards = 8                       # 一张图最多展示的游戏数
//! card_width = 280                    # 卡片宽度（CSS 像素）
//! cover_height = 157                  # 封面高度（CSS 像素）
//! caption = ""                        # 图片说明，留空则不带说明
//! ```

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Map, Value};

/// 插件段名：`[plugin.epic]`。
pub const PLUGIN_NAME: &str = "epic";

/// Epic 官方促销接口；允许配置覆盖（换区域 / 镜像）。
pub const DEFAULT_API_URL: &str =
    "https://store-site-backend-static.ak.epicgames.com/freeGamesPromotions";

/// 默认命令前缀。
const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 促销接口地址。
    pub api_url: String,
    /// 接口 `locale` 参数。
    pub locale: String,
    /// 接口 `country` / `allowCountries` 参数。
    pub country: String,
    /// 接口返回的 UTC 时间换算成本地日期时的偏移小时数。
    pub timezone_offset_hours: i64,
    /// HTTP 代理；`None` 直连。
    pub proxy: Option<String>,
    /// 单次 HTTP 超时。
    pub timeout: Duration,
    /// 命令前缀。
    pub prefixes: Vec<String>,
    /// 输出缩放倍数。
    pub scale: f32,
    /// 每行卡片数。
    pub max_per_row: u32,
    /// 一张图最多展示多少个游戏。
    pub max_cards: usize,
    /// 卡片宽度（CSS 像素）。
    pub card_width: u32,
    /// 封面高度（CSS 像素）。
    pub cover_height: u32,
    /// 图片说明；空串表示不带说明。
    pub caption: String,
    /// 请求 User-Agent。
    pub user_agent: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_url: DEFAULT_API_URL.to_owned(),
            locale: "zh-CN".to_owned(),
            country: "CN".to_owned(),
            timezone_offset_hours: 8,
            proxy: None,
            timeout: Duration::from_millis(20_000),
            prefixes: DEFAULT_PREFIXES.iter().map(|p| (*p).to_owned()).collect(),
            scale: 2.0,
            max_per_row: 4,
            max_cards: 8,
            card_width: 280,
            cover_height: 157,
            caption: String::new(),
            user_agent: "Mozilla/5.0 (compatible; fuyubot-epic/0.1)".to_owned(),
        }
    }
}

impl Config {
    /// 从宿主读配置；缺失或类型不对的项退回缺省值。
    pub fn load(host: &HostApi) -> Self {
        let mut config = Self::default();
        let section = host
            .config_value(PLUGIN_NAME, "")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();

        if let Some(url) = text(&section, &["api_url", "API_URL"]) {
            config.api_url = url;
        }
        if let Some(locale) = text(&section, &["locale", "LOCALE"]) {
            config.locale = locale;
        }
        if let Some(country) = text(&section, &["country", "COUNTRY"]) {
            config.country = country;
        }
        if let Some(hours) = number(
            &section,
            &["timezone_offset_hours", "TIMEZONE_OFFSET_HOURS"],
        ) {
            config.timezone_offset_hours = hours as i64;
        }
        if let Some(ms) = number(&section, &["timeout_ms", "TIMEOUT_MS"]) {
            config.timeout = Duration::from_millis((ms as u64).max(1_000));
        }
        if let Some(scale) = number(&section, &["scale", "SCALE"]) {
            config.scale = (scale as f32).clamp(0.5, 3.0);
        }
        if let Some(n) = number(&section, &["max_per_row", "MAX_PER_ROW"]) {
            config.max_per_row = (n as u32).clamp(1, 8);
        }
        if let Some(n) = number(&section, &["max_cards", "MAX_CARDS"]) {
            config.max_cards = (n as usize).clamp(1, 16);
        }
        if let Some(width) = number(&section, &["card_width", "CARD_WIDTH"]) {
            config.card_width = (width as u32).clamp(120, 800);
        }
        if let Some(height) = number(&section, &["cover_height", "COVER_HEIGHT"]) {
            config.cover_height = (height as u32).clamp(60, 600);
        }
        if let Some(caption) = text(&section, &["caption", "CAPTION"]) {
            config.caption = caption;
        }
        if let Some(agent) = text(&section, &["user_agent", "USER_AGENT"]) {
            config.user_agent = agent;
        }
        // 显式配成空串 = 直连。
        if let Some(proxy) = text(&section, &["proxy", "PROXY"]) {
            config.proxy = (!proxy.trim().is_empty()).then(|| proxy.trim().to_owned());
        }
        // 前缀位于宿主的顶层 `[config]` 段。
        if let Some(value) = host
            .config_value("", "config.PREFIXES")
            .or_else(|| host.config_value("", "config.prefixes"))
        {
            let list: Vec<String> = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !list.is_empty() {
                config.prefixes = list;
            }
        }

        host.info(&format!(
            "epic: 配置 locale={} country={} 时区=UTC{:+} 缩放={} 每行={} 上限={}",
            config.locale,
            config.country,
            config.timezone_offset_hours,
            config.scale,
            config.max_per_row,
            config.max_cards
        ));
        config
    }
}

fn number(section: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        value.as_f64().or_else(|| value.as_u64().map(|n| n as f64))
    })
}

fn text(section: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| section.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}
