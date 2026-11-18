//! 运行期配置（`[plugin.rusto]`），构造一次，之后只读。
//!
//! 读取顺序与仓库其它插件一致：`start()` 传进来的 JSON → `config_value("rusto", "")` 兜底；
//! 缺键退回缺省值。宿主没有配置热重载通道，改 `fuyubot.toml` 需要重载插件。
//!
//! ```toml
//! [plugin.rusto]
//! auto = false                  # 私聊直接发图是否自动识别（默认只认命令）
//! max_side = 960                # 识别前长边限幅（越小越快）
//! concurrency = 1               # 同时识别数（引擎重，默认 1）
//! rec_workers = 1               # rusto 识别线程数；0 = 引擎按核数自选
//! model = "ppocrv6-tiny-int8"   # 模型预设
//! model_dir = ""                # 空 = 内置/缓存目录
//! mirror = ""                   # 模型下载镜像，空 = 官方源
//! detail_default = false        # 默认是否带行/置信度
//! reply_placeholder = true      # 先回「识别中…」再编辑为结果
//! chunk_size = 3800             # 单条回复的最大字符数（Telegram 4096 上限留余量）
//! timeout_ms = 20000            # 单次识别总预算
//! ```

use std::path::PathBuf;
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::Value;

/// 配置段名：`[plugin.rusto]`，同时是插件 id。
pub const PLUGIN_NAME: &str = "rusto";

/// 缺省模型预设。
pub const DEFAULT_MODEL: &str = "ppocrv6-tiny-int8";

/// 缺省长边限幅：实测 960 档单张约 1.1 s、640 档约 0.45 s（900×220 图），
/// 960 是「截图文字还认得出」和「别让用户等太久」的折中。
const DEFAULT_MAX_SIDE: u32 = 960;

/// 缺省并发识别数：单引擎峰值 RSS 约 300–400 MiB，默认串行。
const DEFAULT_CONCURRENCY: usize = 1;

/// 缺省识别线程数：1 = 不启用 rusto-nn 的常驻线程池。
///
/// 默认 0 会让引擎按 CPU 核数起常驻 worker（本机 16 核 = 15 个线程），
/// 一个 bot 进程里没必要；单图串行 1 个线程反而更可预测。
const DEFAULT_REC_WORKERS: usize = 1;

/// 缺省单次识别预算。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// 缺省回复分块长度（Telegram 上限 4096）。
const DEFAULT_CHUNK: usize = 3800;

/// 运行期配置。
#[derive(Debug, Clone)]
pub struct Config {
    /// 私聊直接发图是否自动识别（默认关：只认显式命令）。
    pub auto: bool,
    /// 识别前长边限幅（像素）。
    pub max_side: u32,
    /// 同时识别数。
    pub concurrency: usize,
    /// rusto 识别线程数；0 = 引擎自选。
    pub rec_workers: usize,
    /// 模型预设名。
    pub model: String,
    /// 模型目录；空 = 按内置目录/数据目录解析。
    pub model_dir: Option<PathBuf>,
    /// 模型下载镜像。
    pub mirror: Option<String>,
    /// 默认是否返回行级明细。
    pub detail_default: bool,
    /// 是否先回「识别中…」占位。
    pub reply_placeholder: bool,
    /// 单条回复最大字符数。
    pub chunk_size: usize,
    /// 单次识别总预算。
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto: false,
            max_side: DEFAULT_MAX_SIDE,
            concurrency: DEFAULT_CONCURRENCY,
            rec_workers: DEFAULT_REC_WORKERS,
            model: DEFAULT_MODEL.to_owned(),
            model_dir: None,
            mirror: None,
            detail_default: false,
            reply_placeholder: true,
            chunk_size: DEFAULT_CHUNK,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl Config {
    /// 从宿主读配置；缺失/类型不对退回缺省值。
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
        config.normalize();
        config
    }

    /// 把一段 JSON 覆盖到配置上（大小写不敏感；也接受 camelCase）。
    fn apply(&mut self, value: &Value) {
        let get = |keys: &[&str]| -> Option<&Value> { keys.iter().find_map(|key| value.get(*key)) };
        if let Some(item) = get(&["auto", "AUTO"]).and_then(Value::as_bool) {
            self.auto = item;
        }
        if let Some(item) = get(&["max_side", "maxSide", "MAX_SIDE"]).and_then(Value::as_u64) {
            self.max_side = item.clamp(320, 4096) as u32;
        }
        if let Some(item) = get(&["concurrency", "CONCURRENCY"]).and_then(Value::as_u64) {
            self.concurrency = item.clamp(1, 8) as usize;
        }
        if let Some(item) =
            get(&["rec_workers", "recWorkers", "REC_WORKERS"]).and_then(Value::as_u64)
        {
            self.rec_workers = (item as usize).min(8);
        }
        if let Some(item) = get(&["model", "MODEL"]).and_then(Value::as_str)
            && !item.trim().is_empty()
        {
            self.model = item.trim().to_owned();
        }
        if let Some(item) = get(&["model_dir", "modelDir", "MODEL_DIR"]).and_then(Value::as_str) {
            let text = item.trim();
            self.model_dir = (!text.is_empty()).then(|| PathBuf::from(text));
        }
        if let Some(item) = get(&["mirror", "MIRROR"]).and_then(Value::as_str) {
            let text = item.trim();
            self.mirror = (!text.is_empty()).then(|| text.to_owned());
        }
        if let Some(item) = get(&["detail_default", "detailDefault"]).and_then(Value::as_bool) {
            self.detail_default = item;
        }
        if let Some(item) = get(&["reply_placeholder", "replyPlaceholder"]).and_then(Value::as_bool)
        {
            self.reply_placeholder = item;
        }
        if let Some(item) = get(&["chunk_size", "chunkSize"]).and_then(Value::as_u64) {
            self.chunk_size = (item as usize).clamp(200, 4000);
        }
        if let Some(item) = get(&["timeout_ms", "timeoutMs"]).and_then(Value::as_u64) {
            self.timeout = Duration::from_millis(item.clamp(1_000, 120_000));
        }
    }

    /// 夹一遍边界，避免配置写错把引擎拖死。
    fn normalize(&mut self) {
        self.max_side = self.max_side.clamp(320, 4096);
        self.concurrency = self.concurrency.clamp(1, 8);
        self.chunk_size = self.chunk_size.clamp(200, 4000);
        if self.timeout < Duration::from_secs(1) {
            self.timeout = DEFAULT_TIMEOUT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert!(!config.auto, "默认不抢图");
        assert_eq!(config.concurrency, 1);
        assert_eq!(config.max_side, 960);
        assert_eq!(config.rec_workers, 1);
        assert_eq!(config.model, DEFAULT_MODEL);
        assert!(config.reply_placeholder);
    }

    #[test]
    fn apply_clamps_out_of_range_values() {
        let mut config = Config::default();
        config.apply(&serde_json::json!({
            "max_side": 99999,
            "concurrency": 99,
            "chunk_size": 10,
            "timeout_ms": 5
        }));
        config.normalize();
        assert_eq!(config.max_side, 4096);
        assert_eq!(config.concurrency, 8);
        assert_eq!(config.chunk_size, 200);
        assert!(config.timeout >= Duration::from_secs(1));
    }

    #[test]
    fn apply_accepts_camel_case() {
        let mut config = Config::default();
        config.apply(&serde_json::json!({ "maxSide": 800, "replyPlaceholder": false }));
        assert_eq!(config.max_side, 800);
        assert!(!config.reply_placeholder);
    }
}
