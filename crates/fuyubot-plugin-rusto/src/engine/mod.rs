//! 引擎接缝：命令层与服务层只认这里的类型，不认 `rusto`。
//!
//! 为什么要这一层：识别引擎将来可能换（`ocrs` / 云端 HTTP / mock），
//! 而命令层、服务协议、配置不该跟着动。后端只要实现 [`OcrBackend`]。

pub mod models;
pub mod rusto_backend;

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::core::Config;

/// 一次识别请求（引擎无关）。
#[derive(Debug, Clone)]
pub struct OcrRequest {
    /// 图片来源。
    pub image: ImageInput,
    /// 是否要求行级明细（文本、置信度、四点坐标）。
    pub detail: bool,
    /// 识别前长边限幅。
    pub max_side: u32,
    /// 排队 + 执行的预算。
    pub timeout: Duration,
}

/// 图片来源：路径优先（Telegram 文件已落盘），其次内存字节（服务 `data` 入参）。
#[derive(Debug, Clone)]
pub enum ImageInput {
    /// 本机图片路径。
    Path(PathBuf),
    /// 原始图片字节（PNG / JPEG / WebP / GIF）。
    Bytes(Arc<Vec<u8>>),
}

/// 一行识别结果。
#[derive(Debug, Clone)]
pub struct OcrLine {
    /// 行文本。
    pub text: String,
    /// 置信度 0–1。
    pub score: f32,
    /// 四点坐标（左上、右上、右下、左下），已换算到原图尺度。
    pub quad: [[f32; 2]; 4],
}

/// 识别结果。
#[derive(Debug, Clone)]
pub struct OcrOutput {
    /// 按行拼接的纯文本。
    pub text: String,
    /// 行级明细（`detail` 为 false 时可以为空）。
    pub lines: Vec<OcrLine>,
    /// 引擎侧耗时（毫秒）。
    pub elapsed_ms: u64,
    /// 文本是否被截断（超长保护）。
    pub truncated: bool,
}

/// 引擎错误：服务 / 命令层把它映射成 `{ok:false, code}`。
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// 模型文件缺失（未下载 / 路径写错）。
    #[error("模型缺失：{0}")]
    ModelMissing(String),
    /// 引擎不可用（加载失败、后端未实现）。
    #[error("引擎不可用：{0}")]
    Unavailable(String),
    /// 入参不合法。
    #[error("入参不合法：{0}")]
    BadInput(String),
    /// 图片过大。
    #[error("图片过大")]
    TooLarge,
    /// 超时 / 排队超时。
    #[error("识别超时")]
    Timeout,
    /// 并发闸门已满。
    #[error("引擎繁忙")]
    Busy,
    /// 其它内部错误。
    #[error("内部错误：{0}")]
    Internal(String),
}

impl EngineError {
    /// 服务协议里的错误码（字段只增不改）。
    pub fn code(&self) -> &'static str {
        match self {
            Self::ModelMissing(_) => "model_missing",
            Self::Unavailable(_) => "engine_unavailable",
            Self::BadInput(_) => "bad_input",
            Self::TooLarge => "too_large",
            Self::Timeout => "timeout",
            Self::Busy => "engine_busy",
            Self::Internal(_) => "internal",
        }
    }
}

/// 引擎后端：唯一接缝。实现必须线程安全（服务与命令可能并发调用）。
pub trait OcrBackend: Send + Sync {
    /// 后端名，进结果元信息（`rusto` / `http` / …）。
    fn name(&self) -> &str;
    /// 模型名，进结果元信息。
    fn model(&self) -> &str;
    /// 预加载模型（幂等）。
    fn warm(&self) -> Result<(), EngineError>;
    /// 识别一张图。
    fn recognize(&self, request: &OcrRequest) -> Result<OcrOutput, EngineError>;
    /// 后端状态（进 `/ocr status` 与服务 `status`）。
    fn status(&self) -> Value;
}

/// 引擎配置：由 [`crate::core::Config`] 换算。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// 模型预设名。
    pub model: String,
    /// 模型目录；空 = 后端自己解析（内置 / 数据目录）。
    pub model_dir: Option<PathBuf>,
    /// 下载镜像。
    pub mirror: Option<String>,
    /// 识别线程数；0 = 引擎自选。
    pub rec_workers: usize,
    /// 并发识别数。
    pub concurrency: usize,
}

impl From<&Config> for EngineConfig {
    fn from(config: &Config) -> Self {
        Self {
            model: config.model.clone(),
            model_dir: config.model_dir.clone(),
            mirror: config.mirror.clone(),
            rec_workers: config.rec_workers,
            concurrency: config.concurrency,
        }
    }
}

/// 引擎池：懒加载 + 并发闸门 + 最近耗时。
///
/// 懒加载是硬要求：模型加载是秒级、几百 MiB 的事，绝不能放在 `plugin_start` 里，
/// 否则拖慢宿主启动、失败还会连累插件加载。
pub struct EnginePool {
    config: EngineConfig,
    backend: OnceLock<Result<Arc<dyn OcrBackend>, String>>,
    slots: Mutex<usize>,
    gate: Condvar,
    limit: usize,
    last_elapsed_ms: Mutex<Option<u64>>,
}

impl EnginePool {
    /// 建池；此时不加载任何模型。
    pub fn new(config: EngineConfig) -> Arc<Self> {
        let limit = config.concurrency.clamp(1, 8);
        Arc::new(Self {
            config,
            backend: OnceLock::new(),
            slots: Mutex::new(0),
            gate: Condvar::new(),
            limit,
            last_elapsed_ms: Mutex::new(None),
        })
    }

    /// 懒加载后端（只尝试一次；失败保留错误，避免反复重试拖慢每条命令）。
    fn backend(&self) -> Result<Arc<dyn OcrBackend>, EngineError> {
        let result = self
            .backend
            .get_or_init(|| build_backend(&self.config).map_err(|error| error.to_string()));
        match result {
            Ok(backend) => Ok(backend.clone()),
            Err(message) => Err(EngineError::Unavailable(message.clone())),
        }
    }

    /// 预加载引擎。
    pub fn warm(&self) -> Result<(), EngineError> {
        let backend = self.backend()?;
        backend.warm()
    }

    /// 识别；超出并发上限时按 `request.timeout` 排队，排不到就回 [`EngineError::Busy`]。
    pub fn recognize(&self, request: OcrRequest) -> Result<OcrOutput, EngineError> {
        let backend = self.backend()?;
        let _slot = self.acquire(request.timeout)?;
        let started = Instant::now();
        let output = backend.recognize(&request)?;
        let elapsed = started.elapsed().as_millis() as u64;
        if let Ok(mut slot) = self.last_elapsed_ms.lock() {
            *slot = Some(elapsed);
        }
        Ok(output)
    }

    /// 取一个并发槽位；带超时，避免无限等待。
    fn acquire(&self, timeout: Duration) -> Result<SlotGuard<'_>, EngineError> {
        let deadline = Instant::now() + timeout;
        let mut active = self
            .slots
            .lock()
            .map_err(|_| EngineError::Internal("并发闸门中毒".to_owned()))?;
        while *active >= self.limit {
            let now = Instant::now();
            if now >= deadline {
                return Err(EngineError::Busy);
            }
            let (guard, wait) = self
                .gate
                .wait_timeout(active, deadline - now)
                .map_err(|_| EngineError::Internal("并发闸门中毒".to_owned()))?;
            active = guard;
            if wait.timed_out() && *active >= self.limit {
                return Err(EngineError::Busy);
            }
        }
        *active += 1;
        Ok(SlotGuard { pool: self })
    }

    /// 状态快照（`/ocr status` 与服务 `status`）。
    pub fn status(&self) -> Value {
        let loaded = matches!(self.backend.get(), Some(Ok(_)));
        let backend = self.backend.get().and_then(|result| result.as_ref().ok());
        let active = self.slots.lock().map(|slot| *slot).unwrap_or(0);
        let last = self.last_elapsed_ms.lock().ok().and_then(|slot| *slot);
        json!({
            "ok": true,
            "loaded": loaded,
            "backend": backend.map(|backend| backend.name().to_owned()),
            "model": backend.map(|backend| backend.model().to_owned()),
            "model_config": self.config.model,
            "concurrency": self.limit,
            "active": active,
            "rec_workers": self.config.rec_workers,
            "last_elapsed_ms": last,
        })
    }
}

/// 释放槽位时唤醒等待者。
struct SlotGuard<'a> {
    pool: &'a EnginePool,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.pool.slots.lock() {
            *active = active.saturating_sub(1);
        }
        self.pool.gate.notify_one();
    }
}

/// 构造默认后端（RustO!）。换后端只改这一处。
pub fn build_backend(config: &EngineConfig) -> Result<Arc<dyn OcrBackend>, EngineError> {
    Ok(Arc::new(rusto_backend::RustoBackend::new(config)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(EngineError::Busy.code(), "engine_busy");
        assert_eq!(
            EngineError::ModelMissing("x".into()).code(),
            "model_missing"
        );
        assert_eq!(EngineError::TooLarge.code(), "too_large");
    }

    #[test]
    fn pool_status_is_available_before_load() {
        let pool = EnginePool::new(EngineConfig {
            model: "ppocrv6-tiny-int8".to_owned(),
            model_dir: None,
            mirror: None,
            rec_workers: 0,
            concurrency: 1,
        });
        let status = pool.status();
        assert_eq!(status["ok"], true);
        assert_eq!(status["loaded"], false);
        assert_eq!(status["concurrency"], 1);
    }
}
