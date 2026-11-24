//! 运行期配置。
//!
//! 全部来自宿主的 `[plugin.fengyegf.system]` 段（`fuyubot.toml`）；缺省值与 TS 版写死的常量一致。
//!
//! ```toml
//! [plugin.fengyegf.system]
//! super_admin   = 123456789      # 可选：显式指定管理员；不写就用宿主 [roles].owners
//! background_url = "https://…"   # 卡片背景图（公网 URL，由 html2img 拉取）；留空 = 纯渐变
//! timeout_ms     = 5000          # 单次 TDLib 调用超时
//! prefixes       = ["/", "!", "！", ".", "#"]  # 不写就用缺省前缀
//! ```
//!
//! 读取顺序：`start()` 传进来的配置 JSON（宿主给的 `plugins_dir` 等）→
//! `get_config("fengyegf.system", "")` 兜底；两者缺什么就各自退回缺省值。
//!
//! 管理员（卡片 admin 一栏）**不再要求插件里重复配置**：没写 `super_admin` 时
//! 自动取宿主全局 `[roles].owners` 的第一个人，没有主人再取 `[roles].admins` 的第一个；
//! 都为空且宿主说「自己是主人」时（user 账户）取账号自己。`super_admin` 写了就以它为准。

use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

/// 插件段名：`[plugin.fengyegf.system]`。
pub const PLUGIN_NAME: &str = "fengyegf.system";

/// TS 版 `system.vue` 里写死的背景图；默认沿用它，作为公网 URL 直接写进 HTML。
pub const DEFAULT_BACKGROUND_URL: &str =
    "https://i0.hdslb.com/bfs/article/cc4a3e435ac47a6190aebf4f74a8675b88a183bb.jpg";

/// 命令前缀缺省值（与其它迁移插件一致）。
const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// 单次 TDLib 调用的缺省超时。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// 头像下载的缺省超时：`downloadFile(synchronous = true)` 要等整张图下载完，
/// 5 秒的普通查询超时不够（弱网 / 大图时尤其明显）。
const DEFAULT_AVATAR_TIMEOUT: Duration = Duration::from_secs(20);

/// 宿主预置的命令表服务：`role` 方法查全局角色。
const COMMANDS_SERVICE: &str = "fuyubot.host.commands";
/// 宿主预置的身份服务：`me` 方法拿自己的 user id。
const IDENTITY_SERVICE: &str = "fuyubot.host.identity";

/// 宿主全局的管理员：`[roles].owners` 的第一个，没有主人就取 `[roles].admins` 的第一个；
/// 两者都为空时退回「自己就是主人」的 user 账户（`fuyubot.host.identity.me` + `role`）。
///
/// 出卡片时会再问一次（`start()` 那一刻可能还没登录，`me` 还拿不到）。
pub(crate) fn host_owner(host: &HostApi) -> Option<i64> {
    if let Some(id) = host_role_ids(host, "owners").first() {
        return Some(*id);
    }
    if let Some(id) = host_role_ids(host, "admins").first() {
        return Some(*id);
    }
    let me = self_id(host)?;
    (role_of(host, me).as_deref() == Some("owner")).then_some(me)
}

/// 宿主的 `[roles].<field>`（如 owners / admins）；读不到（老宿主 / 裁剪版）就是空表。
fn host_role_ids(host: &HostApi, field: &str) -> Vec<i64> {
    let dotted = format!("roles.{field}");
    for (section, key) in [("roles", field), ("roles", ""), ("", dotted.as_str())] {
        let Some(value) = host.config_value(section, key) else {
            continue;
        };
        let list = ids(value.get(field).unwrap_or(&value));
        if !list.is_empty() {
            return list;
        }
    }
    Vec::new()
}

/// 问宿主这个人的全局角色（`user` / `admin` / `owner`）；不可用时 `None`。
fn role_of(host: &HostApi, user_id: i64) -> Option<String> {
    let input = json!({ "user_id": user_id }).to_string();
    role_from_reply(&host.call_service(COMMANDS_SERVICE, "role", &input).ok()?)
}

/// `role` 方法的回包解析：`{"global":"owner"}`（也接受 `role` 字段）。
fn role_from_reply(reply: &str) -> Option<String> {
    let value: Value = serde_json::from_str(reply).ok()?;
    let text = value
        .get("global")
        .or_else(|| value.get("role"))?
        .as_str()?;
    Some(text.trim().to_ascii_lowercase())
}

/// 宿主自己的 user id（`getMe`）；没登录就是 `None`。
fn self_id(host: &HostApi) -> Option<i64> {
    let reply = host.call_service(IDENTITY_SERVICE, "me", "{}").ok()?;
    let value: Value = serde_json::from_str(&reply).ok()?;
    id(value.get("id")?)
}

/// JSON 里的 id 列表：数组、单个数字、数字字符串都认；0 与非法值丢弃。
fn ids(value: &Value) -> Vec<i64> {
    match value {
        Value::Array(items) => items.iter().filter_map(id).collect(),
        other => id(other).into_iter().collect(),
    }
}

/// 单个 id：数字或数字字符串，0 / 非法值 → `None`。
fn id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .filter(|id| *id != 0)
}

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 超级管理员 user id；`None` = 卡片里的 admin 一栏显示「未配置」。
    pub super_admin: Option<i64>,
    /// 背景图 URL（公网 `http(s)`）；`None` = 纯渐变背景。
    ///
    /// 不再下载 / 缓存：URL 直接写进卡片 HTML，由 `fuyubot.html2img` 加载。
    pub background_url: Option<String>,
    /// 单次 TDLib 调用（等 `response` 通道）的超时。
    pub td_timeout: Duration,
    /// 头像下载超时（同步 `downloadFile` 的等待预算）。
    pub avatar_timeout: Duration,
    /// 命令前缀。
    pub prefixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            super_admin: None,
            background_url: Some(DEFAULT_BACKGROUND_URL.to_owned()),
            td_timeout: DEFAULT_TIMEOUT,
            avatar_timeout: DEFAULT_AVATAR_TIMEOUT,
            prefixes: DEFAULT_PREFIXES.iter().map(|p| (*p).to_owned()).collect(),
        }
    }
}

impl Config {
    /// 从宿主读配置；缺失或类型不对就退回缺省值并记一条日志。
    pub fn load(host: &HostApi, config_json: &str) -> Self {
        let mut config = Self::default();

        if let Ok(value) = serde_json::from_str::<Value>(config_json)
            && value.is_object()
        {
            config.apply(&value);
        }
        // 再走一遍配置读接口兜底（start 传空串时就需要它）。
        if let Some(value) = host.config_value(PLUGIN_NAME, "") {
            config.apply(&value);
        }

        // 管理员归宿主管：插件段没写 super_admin 就用 `[roles].owners`，
        // 这样新部署只写一处主人，不用每个插件各配一遍。
        if config.super_admin.is_none() {
            config.super_admin = host_owner(host);
        }

        // 前缀：插件段里没写就跟随宿主顶层 `[config].PREFIXES`。
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
            "system: 配置 super_admin={:?} 背景={} 超时={}ms 头像超时={}ms 前缀={:?}",
            config.super_admin,
            config.background_url.as_deref().unwrap_or("（无）"),
            config.td_timeout.as_millis(),
            config.avatar_timeout.as_millis(),
            config.prefixes
        ));
        config
    }

    /// 把一段配置对象合进当前值；类型不对的键忽略。
    fn apply(&mut self, section: &Value) {
        if let Some(id) = integer(section, &["super_admin", "SUPER_ADMIN"])
            && id > 0
        {
            self.super_admin = Some(id);
        }
        if let Some(url) = string(section, &["background_url", "BACKGROUND_URL"]) {
            let url = url.trim();
            self.background_url = (!url.is_empty()).then(|| url.to_owned());
        }
        // 兼容两种写法：timeout_ms（毫秒）优先，其次 timeout（秒）。
        if let Some(ms) = number(section, &["timeout_ms", "TIMEOUT_MS"]) {
            self.td_timeout = Duration::from_millis(ms as u64);
        } else if let Some(seconds) = number(section, &["timeout", "TIMEOUT"]) {
            self.td_timeout = Duration::from_secs(seconds as u64);
        }
        // 头像下载单独一个更宽松的预算。
        if let Some(ms) = number(section, &["avatar_timeout_ms", "AVATAR_TIMEOUT_MS"]) {
            self.avatar_timeout = Duration::from_millis(ms as u64);
        } else if let Some(seconds) = number(section, &["avatar_timeout", "AVATAR_TIMEOUT"]) {
            self.avatar_timeout = Duration::from_secs(seconds as u64);
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

/// 取字符串键，兼容数字写法。
fn string(section: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        if let Some(text) = value.as_str() {
            return Some(text.to_owned());
        }
        value.as_f64().map(|number| number.to_string())
    })
}

/// 取整数键，兼容字符串写法的 id。
fn integer(section: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = section.get(*key)?;
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
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
