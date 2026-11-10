//! 插件配置。
//!
//! 全部配置来自宿主的 `[plugin.ppxfy]` 段（`fuyubot.toml`），键名同时接受
//! snake_case 与大写写法。
//!
//! ```toml
//! [plugin.ppxfy]
//! cache_dir = "cache"                # 运行目录下的缓存目录
//! listen_timeout_ms = 600000         # 等待用户回复的超时
//! unzip_timeout_ms = 1800000         # 单次解压（7z 兜底路径）的超时
//! archive_size_threshold_mb = 1843.2 # 大于该值只记日志，流程不变
//! download_concurrency = 4           # 并发下载数
//! split_threshold_mb = 1900          # 打包分卷阈值
//! seven_zip = "7z"                   # 7z 可执行文件（PATH 里没有就写绝对路径）
//! telegraph_token = ""               # Telegraph 账号 token，必填（galgame 页面用）
//! telegraph_author = "@ppxfy - Galgame"
//! telegraph_author_url = "https://t.me/ppxfy"
//! dlsite_proxy = ""                  # DLSite 日本线路代理，留空则直连
//! mongo_collection = "ppxfy_galgame" # galgame 记录集合（经 storage.mongo 服务读写）
//! mongodb_db = ""                    # 覆盖 mongo-plugin 的默认库名，留空用服务端配置
//! ```
//!
//! 两项凭据（Telegraph token / DLSite 代理）按「配置段 > 环境变量 >
//! 工作目录下的 `.env`」取值，环境变量名是 `PPXFY_TELEGRAPH_TOKEN` 与
//! `PPXFY_DLSITE_PROXY`。`.env` 已写进 `.gitignore`，代码里不留任何缺省凭据。
//!
//! galgame 记录入库依赖 mongo-plugin 提供的 `storage.mongo` 服务；连接串与默认库在
//! `[plugin.mongo-plugin]` 段配置（`uri` / `db`）。没装 mongo-plugin 时 `/p` 照常
//! 出图分享，只是不入库，日志会写清原因。

use std::path::{Path, PathBuf};
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Map, Value};

/// 插件段名：`[plugin.ppxfy]`。
pub const PLUGIN_NAME: &str = "ppxfy";

/// 默认命令前缀。
const DEFAULT_PREFIXES: [&str; 5] = ["/", "!", "！", ".", "#"];

/// Telegraph 账号 token 缺省留空：这是凭据，只从配置 / 环境 / `.env` 读，不进代码与版本库。
const DEFAULT_TELEGRAPH_TOKEN: &str = "";
/// 凭据的环境变量名（`.env` 里也用同样两个键）。
const TELEGRAPH_TOKEN_ENV: &str = "PPXFY_TELEGRAPH_TOKEN";
const DLSITE_PROXY_ENV: &str = "PPXFY_DLSITE_PROXY";
/// 页面上展示的作者信息。
const DEFAULT_TELEGRAPH_AUTHOR: &str = "@ppxfy - Galgame";
const DEFAULT_TELEGRAPH_AUTHOR_URL: &str = "https://t.me/ppxfy";

/// Telegraph 账号信息。
#[derive(Debug, Clone)]
pub struct TelegraphConfig {
    /// 账号 access token。
    pub token: String,
    /// 页面上显示的作者名。
    pub author_name: String,
    /// 点击作者名打开的链接。
    pub author_url: String,
}

impl Default for TelegraphConfig {
    fn default() -> Self {
        Self {
            token: DEFAULT_TELEGRAPH_TOKEN.to_owned(),
            author_name: DEFAULT_TELEGRAPH_AUTHOR.to_owned(),
            author_url: DEFAULT_TELEGRAPH_AUTHOR_URL.to_owned(),
        }
    }
}

/// 运行期配置（构造一次，之后只读）。
#[derive(Debug, Clone)]
pub struct Config {
    /// 下载 / 解压 / 打包的根目录，其下固定使用 `Output` 与 `ZipOutput` 两个子目录。
    pub cache_dir: PathBuf,
    /// 等待用户回复（链接、密码、编号、包名）的超时。
    pub listen_timeout: Duration,
    /// 7z 兜底解压的超时。
    pub unzip_timeout: Duration,
    /// 压缩包大小阈值（字节），仅用于日志提示。
    pub archive_size_threshold: u64,
    /// 同时下载的链接数。
    pub download_concurrency: usize,
    /// 打包时的分卷阈值（MB）。
    pub split_threshold_mb: u64,
    /// 7z 可执行文件路径。
    pub seven_zip: PathBuf,
    /// 命令前缀列表。
    pub prefixes: Vec<String>,
    /// 解压后自动修复乱码文件名（CP437 误解 Shift-JIS / GBK 的情形）。
    pub fix_encoding: bool,
    /// galgame 预览页用的 Telegraph 账号。
    pub telegraph: TelegraphConfig,
    /// DLSite 请求走的代理；`None` 直连。
    pub dlsite_proxy: Option<String>,
    /// galgame 记录写入的 MongoDB 集合名；库由 mongo-plugin 决定。
    pub mongo_collection: String,
    /// 覆盖 mongo-plugin 的默认库名；`None` 表示用服务端配置的库。
    pub mongodb_db: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cache_dir: std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("cache"),
            listen_timeout: Duration::from_secs(600),
            unzip_timeout: Duration::from_secs(1800),
            archive_size_threshold: (1843.2 * 1024.0 * 1024.0) as u64,
            download_concurrency: 4,
            split_threshold_mb: 1900,
            seven_zip: PathBuf::from("7z"),
            prefixes: DEFAULT_PREFIXES.iter().map(|p| p.to_string()).collect(),
            fix_encoding: true,
            telegraph: TelegraphConfig::default(),
            // 代理也是凭据（多半带账号口令），缺省留空 = 直连。
            dlsite_proxy: None,
            // 使用既有集合名，便于直接接上已有数据。
            mongo_collection: "ppxfy_galgame".to_owned(),
            mongodb_db: None,
        }
    }
}

impl Config {
    /// 从宿主读取配置；任何一项缺失或类型不对都退回缺省值并记一条警告。
    pub fn load(host: &HostApi) -> Self {
        let mut config = Self::default();
        // 凭据先取环境（环境变量 > 工作目录的 `.env`）；配置段里显式写了就覆盖。
        if let Some(token) = credential(TELEGRAPH_TOKEN_ENV) {
            config.telegraph.token = token;
        }
        if let Some(proxy) = credential(DLSITE_PROXY_ENV) {
            config.dlsite_proxy = Some(proxy);
        }
        let section = host
            .config_value(PLUGIN_NAME, "")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();

        if let Some(ms) = number(&section, &["listen_timeout_ms", "LISTEN_TIMEOUT_MS"]) {
            config.listen_timeout = Duration::from_millis(ms as u64);
        }
        if let Some(ms) = number(&section, &["unzip_timeout_ms", "UNZIP_TIMEOUT_MS"]) {
            config.unzip_timeout = Duration::from_millis(ms as u64);
        }
        if let Some(mb) = number(
            &section,
            &["archive_size_threshold_mb", "ARCHIVE_SIZE_THRESHOLD_MB"],
        ) {
            config.archive_size_threshold = (mb * 1024.0 * 1024.0) as u64;
        }
        if let Some(n) = number(&section, &["download_concurrency", "DOWNLOAD_CONCURRENCY"]) {
            config.download_concurrency = (n as usize).clamp(1, 16);
        }
        if let Some(mb) = number(&section, &["split_threshold_mb", "SPLIT_THRESHOLD_MB"]) {
            config.split_threshold_mb = (mb as u64).max(1);
        }
        if let Some(dir) = text(&section, &["cache_dir", "CACHE_DIR"]) {
            config.cache_dir = PathBuf::from(dir);
        }
        if let Some(exe) = text(&section, &["seven_zip", "SEVEN_ZIP"]) {
            config.seven_zip = PathBuf::from(exe);
        }
        if let Some(flag) = section
            .get("fix_encoding")
            .or_else(|| section.get("FIX_ENCODING"))
            .and_then(Value::as_bool)
        {
            config.fix_encoding = flag;
        }
        if let Some(token) = text(&section, &["telegraph_token", "TELEGRAPH_TOKEN"]) {
            config.telegraph.token = token;
        }
        if let Some(name) = text(&section, &["telegraph_author", "TELEGRAPH_AUTHOR"]) {
            config.telegraph.author_name = name;
        }
        if let Some(url) = text(&section, &["telegraph_author_url", "TELEGRAPH_AUTHOR_URL"]) {
            config.telegraph.author_url = url;
        }
        // 显式配成空串 = 不走代理。
        if let Some(proxy) = text(&section, &["dlsite_proxy", "DLSITE_PROXY"]) {
            config.dlsite_proxy = (!proxy.trim().is_empty()).then(|| proxy.trim().to_owned());
        }
        // 集合名留空没有意义（mongo 会拒绝空集合名），显式写空串视同没写。
        if let Some(name) = text(
            &section,
            &["mongo_collection", "MONGO_COLLECTION", "galgame_collection"],
        ) {
            let name = name.trim();
            if !name.is_empty() {
                config.mongo_collection = name.to_owned();
            }
        }
        // 库名覆盖：留空 = 用 mongo-plugin 自己配置的默认库。
        if let Some(db) = text(&section, &["mongodb_db", "MONGODB_DB"]) {
            config.mongodb_db = (!db.trim().is_empty()).then(|| db.trim().to_owned());
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
            "ppxfy: 配置 cache={} 并发={} 监听超时={}s 分卷阈值={}MB 集合={}{}",
            config.cache_dir.display(),
            config.download_concurrency,
            config.listen_timeout.as_secs(),
            config.split_threshold_mb,
            config.mongo_collection,
            config
                .mongodb_db
                .as_deref()
                .map(|db| format!("（库 {db}）"))
                .unwrap_or_default()
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

/// 按候选键名依次取字符串值。
fn text(section: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| section.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}

/// 凭据取值：先看环境变量，再看工作目录下的 `.env`；都没有返回 `None`。
fn credential(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| dotenv(name))
}

/// 从 `.env` 里读一个键（`KEY=VALUE`，支持 `#` 注释与成对引号）。
///
/// 自己解析而不引入 dotenv 依赖：宿主不一定加载环境文件，插件也没权限改宿主
/// 的环境变量（edition 2024 里 `set_var` 是 unsafe），所以只按键取值用。
///
/// 候选位置有两个：宿主的工作目录，以及「工作目录/plugins/ppxfy」——宿主从自
/// 身根目录启动时，插件目录里的 `.env` 就不在 cwd 下。
fn dotenv(name: &str) -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    [
        cwd.join(".env"),
        cwd.join("plugins").join(PLUGIN_NAME).join(".env"),
    ]
    .iter()
    .find_map(|path| dotenv_at(path, name))
}

/// 读单个 `.env` 文件里的一个键。
fn dotenv_at(path: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| {
            value
                .trim()
                .trim_matches(|ch| ch == '"' || ch == '\'')
                .to_owned()
        })
        .filter(|value| !value.is_empty())
}
