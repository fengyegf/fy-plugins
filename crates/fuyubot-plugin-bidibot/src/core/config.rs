//! 运行期配置：全部来自宿主 [plugin.fengyegf.bidibot] 段（fuyubot.toml）。
//!
//! 缺省值对一份"什么都没配"的 toml 也安全：group_id = 0 时不会往任何地方中转；
//! 主人来自宿主 `[roles]`，`owner_id = 0` 只是「不额外加人」。键名同时接受
//! snake_case 与全大写写法。
//!
//! 可变的封禁 / 警告 / 名片头映射**不在这里**——那些进本地库（core::store）。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::Value;

use crate::PLUGIN_NAME;

/// 广告判定模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdMode {
    /// 不拦（只用双向量能做提示）。
    Off,
    /// 纯关键词匹配：主人自己往 ad_keywords 里加标记。
    Keyword,
    /// 大模型 + skills/ad.md 判定；失败自动退回关键词。
    Llm,
}

impl AdMode {
    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "0" => Some(Self::Off),
            "keyword" | "keywords" | "match" => Some(Self::Keyword),
            "llm" | "ai" | "auto" => Some(Self::Llm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Keyword => "keyword",
            Self::Llm => "llm",
        }
    }
}

/// 大模型接入参数（OpenAI 兼容）。
#[derive(Debug, Clone, PartialEq)]
pub struct AiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
    pub temperature: f64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_owned(),
            api_key: String::new(),
            model: "gpt-4o-mini".to_owned(),
            timeout: Duration::from_millis(20_000),
            temperature: 0.2,
        }
    }
}

/// 内置广告关键词：可以用 ad_keywords 整份替换，或用 ad_keywords_extra 追加。
///
/// 刻意保守：只收"广告话术 + 引流渠道"这种在私聊里几乎不会正常出现的片段。
pub const DEFAULT_AD_KEYWORDS: &[&str] = &[
    "加微信",
    "加v信",
    "加vx",
    "加薇信",
    "vx:",
    "薇信",
    "刷单",
    "日入",
    "返利",
    "博彩",
    "棋牌",
    "投资群",
    "带单",
    "https://t.me/",
    "t.me/",
];

const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// Telegram 允许的话题图标颜色（createForumTopic 的 icon.color 只认这六个）。
pub const TOPIC_COLORS: [i64; 6] = [0x6FB9F0, 0xFFD67E, 0xCB86DB, 0x8EEE98, 0xFF93B2, 0xFB6F5F];

/// 运行期配置（构造后由 Ctx 用 Mutex<Arc<Config>> 持有，改配置整体换一份）。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 论坛超级群 id（负数）。0 = 未配置。
    pub group_id: i64,
    /// **额外**的主人 user id（向后兼容老配置）；0 = 不加人。
    ///
    /// 真正的主人来自宿主全局角色（`fuyubot.toml` 的 `[roles].owners`），
    /// 由 [`crate::host::roles`] 查询，这里只是再多认一个人。
    pub owner_id: i64,
    pub ad_mode: AdMode,
    pub ai: AiConfig,
    /// 警告到多少次自动熔断（最少 1）。
    pub auto_ban_after: u32,
    /// 同一用户两次警告的最小间隔（秒）。
    pub warn_cooldown_secs: u64,
    /// 话题名截断到多少字符（Telegram 上限 128）。
    pub topic_name_max: usize,
    /// 新话题的图标颜色（Telegram 只认 6 个值）。
    pub topic_icon_color: i64,
    /// 新建话题时是否发一条用户名片（ID / 用户名 / 语言）。
    pub topic_card: bool,
    /// 名片优先用 fuyubot.html2img 渲染成图片（装了才生效，没装自动降级文字）。
    pub card_image: bool,
    /// 名片图宽度（CSS px，输出按 2 倍图）。默认 460 = 紧凑卡片（头像 + 三行）。
    pub card_width: u32,
    /// 本地库文件（相对项目根）。
    pub store_path: String,
    /// skills 目录（相对项目根），文件缺失时用内置。
    pub skills_dir: String,
    /// 单条中转内容的最大字符数。
    pub max_relay_chars: usize,
    /// 单次发送的等待超时（毫秒）。离线回放 / 弱网排错时可以调小。
    pub send_timeout_ms: u64,
    /// 单用户每分钟最多中转多少次；0 = 不限制。
    pub rate_limit_per_min: u32,
    /// 被拦用户最短多少秒才重复提示一次。
    pub notify_blocked_every_secs: u64,
    /// 是否把命令登记进宿主命令表（/help 的数据源）。
    pub command_table: bool,
    /// 关键词表：非空则**替换**内置表。
    pub ad_keywords: Vec<String>,
    /// 追加到（内置或替换后的）关键词表。
    pub ad_keywords_extra: Vec<String>,
    pub prefixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            group_id: 0,
            owner_id: 0,
            ad_mode: AdMode::Keyword,
            ai: AiConfig::default(),
            auto_ban_after: 3,
            warn_cooldown_secs: 30,
            topic_name_max: 28,
            topic_icon_color: 0x6FB9F0,
            topic_card: true,
            card_image: true,
            card_width: 460,
            store_path: "data/bidibot/db.json".to_owned(),
            skills_dir: "skills/bidibot".to_owned(),
            max_relay_chars: 3_500,
            send_timeout_ms: 30_000,
            rate_limit_per_min: 12,
            notify_blocked_every_secs: 3_600,
            command_table: true,
            ad_keywords: Vec::new(),
            ad_keywords_extra: Vec::new(),
            prefixes: DEFAULT_PREFIXES.iter().map(|p| (*p).to_owned()).collect(),
        }
    }
}

impl Config {
    /// 从宿主读配置：start() 给的 JSON → get_config 兜底。
    pub fn load(host: &HostApi, config_json: &str) -> Self {
        let mut config = Self::default();
        if let Ok(value) = serde_json::from_str::<Value>(config_json)
            && value.is_object()
        {
            config.apply(&value);
        }
        if let Some(value) = host.config_value(PLUGIN_NAME, "") {
            config.apply(&value);
        }
        // 前缀跟宿主顶层 [config].PREFIXES 走（Rust 宿主没有这个键就继续用缺省）。
        if config.prefixes.is_empty()
            && let Some(value) = host
                .config_value("", "config.PREFIXES")
                .or_else(|| host.config_value("", "config.prefixes"))
            && let Some(list) = string_list(&value)
        {
            config.prefixes = list;
        }
        config
    }

    /// 把一段配置对象合进当前值；类型不对的键忽略（不 panic、不回退整份配置）。
    pub fn apply(&mut self, section: &Value) {
        if let Some(id) = integer(section, &["owner_id", "OWNER_ID", "owner"]) {
            self.owner_id = id;
        }
        if let Some(mode) = string(section, &["ad_mode", "AD_MODE"])
            .as_deref()
            .and_then(AdMode::parse)
        {
            self.ad_mode = mode;
        }
        if let Some(url) = string(section, &["ai_base_url", "AI_BASE_URL", "base_url"])
            && url.trim().starts_with("http")
        {
            self.ai.base_url = url.trim().to_owned();
        }
        if let Some(key) = string(section, &["ai_api_key", "AI_API_KEY", "api_key"]) {
            self.ai.api_key = key.trim().to_owned();
        }
        if let Some(model) = string(section, &["ai_model", "AI_MODEL", "model"])
            && !model.trim().is_empty()
        {
            self.ai.model = model.trim().to_owned();
        }
        if let Some(ms) = number(section, &["ai_timeout_ms", "AI_TIMEOUT_MS"]) {
            self.ai.timeout = Duration::from_millis(ms.max(1_000.0) as u64);
        }
        if let Some(temperature) = number(section, &["ai_temperature", "AI_TEMPERATURE"]) {
            self.ai.temperature = temperature.clamp(0.0, 2.0);
        }
        if let Some(count) = number(section, &["auto_ban_after", "AUTO_BAN_AFTER"]) {
            self.auto_ban_after = (count as u32).max(1);
        }
        if let Some(secs) = number(section, &["warn_cooldown_secs", "WARN_COOLDOWN_SECS"]) {
            self.warn_cooldown_secs = secs.max(0.0) as u64;
        }
        if let Some(id) = integer(section, &["group_id", "GROUP_ID", "group"]) {
            self.group_id = id;
        }
        if let Some(count) = number(section, &["topic_name_max", "TOPIC_NAME_MAX"]) {
            self.topic_name_max = (count as usize).clamp(1, 128);
        }
        if let Some(color) = integer(section, &["topic_icon_color", "TOPIC_ICON_COLOR"]) {
            // 只认 Telegram 允许的 6 个颜色，写错就退回缺省（否则建话题会被 TDLib 拒）。
            self.topic_icon_color = if TOPIC_COLORS.contains(&color) {
                color
            } else {
                0x6FB9F0
            };
        }
        if let Some(flag) = boolean(section, &["topic_card", "TOPIC_CARD"]) {
            self.topic_card = flag;
        }
        if let Some(flag) = boolean(section, &["card_image", "CARD_IMAGE"]) {
            self.card_image = flag;
        }
        if let Some(width) = number(section, &["card_width", "CARD_WIDTH"]) {
            self.card_width = (width as u32).clamp(320, 1_200);
        }
        if let Some(path) = string(section, &["store_path", "STORE_PATH"])
            && !path.trim().is_empty()
        {
            self.store_path = path.trim().to_owned();
        }
        if let Some(dir) = string(section, &["skills_dir", "SKILLS_DIR"]) {
            self.skills_dir = dir.trim().to_owned();
        }
        if let Some(chars) = number(section, &["max_relay_chars", "MAX_RELAY_CHARS"]) {
            self.max_relay_chars = (chars as usize).clamp(200, 4_096);
        }
        if let Some(ms) = number(section, &["send_timeout_ms", "SEND_TIMEOUT_MS"]) {
            self.send_timeout_ms = (ms as u64).clamp(1_000, 120_000);
        }
        if let Some(limit) = number(section, &["rate_limit_per_min", "RATE_LIMIT_PER_MIN"]) {
            self.rate_limit_per_min = limit.max(0.0) as u32;
        }
        if let Some(secs) = number(
            section,
            &["notify_blocked_every_secs", "NOTIFY_BLOCKED_EVERY_SECS"],
        ) {
            self.notify_blocked_every_secs = secs.max(0.0) as u64;
        }
        if let Some(flag) = boolean(section, &["command_table", "COMMAND_TABLE"]) {
            self.command_table = flag;
        }
        if let Some(list) = section
            .get("ad_keywords")
            .or_else(|| section.get("AD_KEYWORDS"))
            .and_then(string_list)
        {
            self.ad_keywords = list;
        }
        if let Some(list) = section
            .get("ad_keywords_extra")
            .or_else(|| section.get("AD_KEYWORDS_EXTRA"))
            .and_then(string_list)
        {
            self.ad_keywords_extra = list;
        }
        if let Some(list) = section
            .get("prefixes")
            .or_else(|| section.get("PREFIXES"))
            .and_then(string_list)
            && !list.is_empty()
        {
            self.prefixes = list;
        }
    }

    /// 有没有可用的 AI 凭据。
    pub fn ai_enabled(&self) -> bool {
        !self.ai.api_key.trim().is_empty()
            && !self.ai.base_url.trim().is_empty()
            && !self.ai.model.trim().is_empty()
    }

    /// 生效的广告关键词：ad_keywords 非空就替换内置表，否则用内置表；再加 extra。
    pub fn effective_ad_keywords(&self) -> Vec<String> {
        let mut list: Vec<String> = if self.ad_keywords.is_empty() {
            DEFAULT_AD_KEYWORDS
                .iter()
                .map(|word| (*word).to_owned())
                .collect()
        } else {
            self.ad_keywords.clone()
        };
        for extra in &self.ad_keywords_extra {
            let word = extra.trim();
            if !word.is_empty() && !list.iter().any(|item| item == word) {
                list.push(word.to_owned());
            }
        }
        list
    }

    /// 生效的广告模式：配了 llm 但没 AI 凭据就退回关键词。
    pub fn effective_ad_mode(&self) -> AdMode {
        match self.ad_mode {
            AdMode::Llm if !self.ai_enabled() => AdMode::Keyword,
            mode => mode,
        }
    }

    /// 发送超时。
    pub fn send_timeout(&self) -> Duration {
        Duration::from_millis(self.send_timeout_ms)
    }

    /// 单条内容按配置截断，末尾加省略号。
    pub fn truncate_relay(&self, text: &str) -> String {
        if text.chars().count() <= self.max_relay_chars {
            return text.to_owned();
        }
        let mut out: String = text.chars().take(self.max_relay_chars).collect();
        out.push_str("…（已截断）");
        out
    }
}

fn string(section: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        if let Some(text) = value.as_str() {
            return Some(text.to_owned());
        }
        value.as_f64().map(|number| number.to_string())
    })
}

fn number(section: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

fn integer(section: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

fn boolean(section: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        if let Some(flag) = value.as_bool() {
            return Some(flag);
        }
        match value.as_str()?.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        }
    })
}

fn string_list(value: &Value) -> Option<Vec<String>> {
    let items = value.as_array()?;
    let list: Vec<String> = items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    Some(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_safe_without_any_config() {
        let config = Config::default();
        assert_eq!(config.owner_id, 0);
        assert_eq!(config.ad_mode, AdMode::Keyword);
        assert_eq!(config.auto_ban_after, 3);
        assert_eq!(config.prefixes.first().map(String::as_str), Some("/"));
    }

    #[test]
    fn applies_snake_case_keys() {
        let mut config = Config::default();
        config.apply(&json!({
            "owner_id": 123456789,
            "ad_mode": "llm",
            "ai_api_key": "sk-x",
            "auto_ban_after": 5,
            "warn_cooldown_secs": 60,
            "group_id": -1_001_234_567_890i64,
            "topic_name_max": 20,
            "topic_icon_color": 16_766_590,
            "topic_card": false,
            "store_path": "data/custom.json",
            "ad_keywords": ["spam", "广告"],
            "ad_keywords_extra": ["more"],
            "prefixes": ["/", "!"],
        }));
        assert_eq!(config.owner_id, 123_456_789);
        assert_eq!(config.ad_mode, AdMode::Llm);
        assert_eq!(config.auto_ban_after, 5);
        assert_eq!(config.warn_cooldown_secs, 60);
        assert_eq!(config.group_id, -1_001_234_567_890);
        assert_eq!(config.topic_name_max, 20);
        assert_eq!(config.topic_icon_color, 0xFFD67E);
        assert!(!config.topic_card);
        assert_eq!(config.store_path, "data/custom.json");
        assert_eq!(config.effective_ad_keywords(), vec!["spam", "广告", "more"]);
        assert_eq!(config.prefixes, vec!["/", "!"]);
    }

    #[test]
    fn topic_settings_are_clamped_and_validated() {
        let mut config = Config::default();
        // 非 Telegram 允许的颜色要退回缺省，否则建话题会被 TDLib 拒。
        config.apply(&json!({ "topic_icon_color": 12345, "topic_name_max": 9999 }));
        assert_eq!(config.topic_icon_color, 0x6FB9F0);
        assert_eq!(config.topic_name_max, 128);

        let mut config = Config::default();
        config.apply(&json!({ "topic_icon_color": "16766590" }));
        assert_eq!(config.topic_icon_color, 0xFFD67E);
    }

    #[test]
    fn numbers_written_as_strings_are_accepted() {
        let mut config = Config::default();
        config.apply(&json!({ "owner_id": "42", "auto_ban_after": "2" }));
        assert_eq!(config.owner_id, 42);
        assert_eq!(config.auto_ban_after, 2);
    }

    #[test]
    fn llm_ad_mode_without_key_degrades_to_keywords() {
        let mut config = Config::default();
        config.apply(&json!({ "ad_mode": "llm" }));
        assert_eq!(config.effective_ad_mode(), AdMode::Keyword);

        let mut config = Config::default();
        config.apply(&json!({ "ad_mode": "llm", "ai_api_key": "sk-x" }));
        assert_eq!(config.effective_ad_mode(), AdMode::Llm);
    }

    #[test]
    fn builtin_keywords_are_used_until_overridden() {
        let config = Config::default();
        assert!(
            config
                .effective_ad_keywords()
                .iter()
                .any(|word| word == "加微信")
        );
    }

    #[test]
    fn relay_text_is_truncated_by_chars() {
        let config = Config {
            max_relay_chars: 5,
            ..Config::default()
        };
        assert_eq!(config.truncate_relay("12345"), "12345");
        assert_eq!(config.truncate_relay("1234567890"), "12345…（已截断）");
    }
}
