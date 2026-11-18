impl RustoBackend {
    pub fn new(config: &EngineConfig) -> Result<Self, EngineError> {
        Ok(Self {
            config: config.clone(),
            slot: Mutex::new(Slot::default()),
        })
    }

    fn load_into(config: &EngineConfig, slot: &mut Slot) -> Result<(), EngineError> {
        if slot.loaded.is_some() {
            return Ok(());
        }
        if let Some(message) = slot.failure.clone() {
            return Err(EngineError::Unavailable(message));
        }
        match load_engine(config) {
            Ok(loaded) => {
                slot.loaded = Some(loaded);
                Ok(())
            }
            Err(error) => {
                slot.failure = Some(error.to_string());
                Err(error)
            }
        }
    }
}

fn load_engine(config: &EngineConfig) -> Result<Loaded, EngineError> {
    let paths = models::resolve(config)?;
    let preset = models::preset_of(config);
    let det = paths.det.clone();
    let rec = paths.rec.clone();
    let dict = paths.dict.clone();
    let mut init = if preset.contains("v5") {
        rusto::InitializeConfig::ppv5(det, rec, dict)
    } else if preset.contains("v4") {
        rusto::InitializeConfig::ppv4(det, rec, dict)
    } else if preset.contains("v3") {
        rusto::InitializeConfig::ppv3(det, rec, dict)
    } else if preset.contains("indra") {
        rusto::InitializeConfig::indra(det, rec, dict)
    } else {
        rusto::InitializeConfig::ppv6(det, rec, dict)
    };
    if config.rec_workers > 0 {
        init = init
            .with_rec_workers(config.rec_workers)
            .with_max_rec_workers(config.rec_workers);
    }
    let engine = rusto::RustO::initialize(init).map_err(|error| {
        EngineError::Unavailable(format!(
            "加载模型失败（{}，格式 {}）：{error}",
            paths.dir().display(),
            paths.format()
        ))
    })?;
    Ok(Loaded { engine, paths })
}

use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::sync::Mutex;
use std::time::Instant;

use image::DynamicImage;
use image::imageops::FilterType;
use serde_json::Value;
use serde_json::json;

use crate::engine::models;
use crate::engine::models::ModelPaths;
use crate::engine::{
    EngineConfig, EngineError, ImageInput, OcrBackend, OcrLine, OcrOutput, OcrRequest,
};

const MAX_PIXELS: u64 = 40_000_000;

struct Loaded {
    engine: rusto::RustO,
    paths: ModelPaths,
}

#[derive(Default)]
struct Slot {
    loaded: Option<Loaded>,
    failure: Option<String>,
}

pub struct RustoBackend {
    config: EngineConfig,
    slot: Mutex<Slot>,
}

impl OcrBackend for RustoBackend {
    fn name(&self) -> &str {
        "rusto"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn warm(&self) -> Result<(), EngineError> {
        let mut slot = self
            .slot
            .lock()
            .map_err(|_| EngineError::Internal("引擎状态锁中毒".to_owned()))?;
        Self::load_into(&self.config, &mut slot)
    }

    fn recognize(&self, request: &OcrRequest) -> Result<OcrOutput, EngineError> {
        match catch_unwind(AssertUnwindSafe(|| self.run(request))) {
            Ok(result) => result,
            Err(_) => Err(EngineError::Internal("引擎内部 panic（已捕获）".to_owned())),
        }
    }

    fn status(&self) -> Value {
        let (loaded, format, dir, failure) = match self.slot.lock() {
            Ok(slot) => (
                slot.loaded.is_some(),
                slot.loaded
                    .as_ref()
                    .map(|loaded| loaded.paths.format().to_owned()),
                slot.loaded
                    .as_ref()
                    .map(|loaded| loaded.paths.dir().display().to_string()),
                slot.failure.clone(),
            ),
            Err(_) => (false, None, None, Some("引擎状态锁中毒".to_owned())),
        };
        json!({
            "backend": "rusto",
            "model": self.config.model,
            "loaded": loaded,
            "format": format,
            "model_dir": dir,
            "failure": failure,
        })
    }
}

impl RustoBackend {
    fn run(&self, request: &OcrRequest) -> Result<OcrOutput, EngineError> {
        let started = Instant::now();
        let (image, origin_w, origin_h) = decode(&request.image)?;
        if origin_w.saturating_mul(origin_h) > MAX_PIXELS {
            return Err(EngineError::TooLarge);
        }
        let (image, factor_x, factor_y) = fit(image, request.max_side);
        let rgb = image.to_rgb8();
        let (width, height) = rgb.dimensions();
        let stride = width as usize * 3;
        let pixels = rgb.into_raw();

        let mut slot = self
            .slot
            .lock()
            .map_err(|_| EngineError::Internal("引擎状态锁中毒".to_owned()))?;
        Self::load_into(&self.config, &mut slot)?;
        let loaded = slot
            .loaded
            .as_mut()
            .ok_or_else(|| EngineError::Internal("引擎状态异常".to_owned()))?;
        let options = rusto::OcrRunOptions::new();
        let result = loaded
            .engine
            .detect_text_rgb(&pixels, width, height, stride, &options)
            .map_err(|error| EngineError::Internal(format!("识别失败：{error}")))?;

        let mut parts: Vec<String> = Vec::new();
        let mut lines = Vec::new();
        if let rusto::DetectTextResult::Structured(items) = result {
            for item in items {
                parts.push(item.text.trim().to_owned());
                if request.detail {
                    lines.push(OcrLine {
                        text: item.text,
                        score: item.score,
                        quad: item.box_points.map(|(x, y)| [x * factor_x, y * factor_y]),
                    });
                }
            }
        }
        Ok(OcrOutput {
            text: parts.join("\n"),
            lines,
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated: false,
        })
    }
}

fn decode(input: &ImageInput) -> Result<(DynamicImage, u64, u64), EngineError> {
    match input {
        ImageInput::Path(path) => {
            if !path.is_file() {
                return Err(EngineError::BadInput(format!(
                    "图片不存在：{}",
                    path.display()
                )));
            }
            let bytes = std::fs::read(path)
                .map_err(|error| EngineError::BadInput(format!("读取图片失败：{error}")))?;
            decode_bytes(&bytes)
        }
        ImageInput::Bytes(bytes) => decode_bytes(bytes),
    }
}

fn decode_bytes(bytes: &[u8]) -> Result<(DynamicImage, u64, u64), EngineError> {
    let image = image::load_from_memory(bytes)
        .map_err(|error| EngineError::BadInput(format!("无法解码图片：{error}")))?;
    let width = u64::from(image.width());
    let height = u64::from(image.height());
    if width == 0 || height == 0 {
        return Err(EngineError::BadInput("图片尺寸为 0".to_owned()));
    }
    Ok((image, width, height))
}

fn fit(image: DynamicImage, max_side: u32) -> (DynamicImage, f32, f32) {
    let width = image.width();
    let height = image.height();
    let limit = max_side.max(1);
    if width <= limit && height <= limit {
        return (image, 1.0, 1.0);
    }
    let resized = image.resize(limit, limit, FilterType::Triangle);
    let factor_x = width as f32 / resized.width() as f32;
    let factor_y = height as f32 / resized.height() as f32;
    (resized, factor_x, factor_y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn env_path(name: &str) -> Option<PathBuf> {
        let value = std::env::var(name).ok()?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    }

    fn config(model_dir: PathBuf) -> EngineConfig {
        EngineConfig {
            model: "ppocrv6-tiny-int8".to_owned(),
            model_dir: Some(model_dir),
            mirror: None,
            rec_workers: 0,
            concurrency: 1,
        }
    }

    #[test]
    fn new_does_not_load_models() {
        let backend = RustoBackend::new(&config(PathBuf::from("/nonexistent/rusto-models")))
            .expect("new 不应失败");
        let status = backend.status();
        assert_eq!(status["loaded"], false);
        assert_eq!(status["backend"], "rusto");
        assert_eq!(status["model"], "ppocrv6-tiny-int8");
    }

    #[test]
    fn missing_models_report_actionable_error() {
        let backend = RustoBackend::new(&config(PathBuf::from("/nonexistent/rusto-models")))
            .expect("new 不应失败");
        let error = backend.warm().expect_err("模型目录为空应当失败");
        let message = error.to_string();
        assert!(
            message.contains("det") || message.contains("模型"),
            "错误信息应提示缺什么：{message}"
        );
    }

    /// 真识别冒烟测试：只有显式给了模型目录与图片才跑（CI 下自动跳过）。
    #[test]
    fn recognizes_synthetic_image_when_models_present() {
        let Some(models) = env_path("RUSTO_TEST_MODELS") else {
            return;
        };
        let Some(image) = env_path("RUSTO_TEST_IMAGE") else {
            return;
        };
        let backend = RustoBackend::new(&config(models)).expect("new");
        let started = Instant::now();
        let output = backend
            .recognize(&OcrRequest {
                image: ImageInput::Path(image),
                detail: true,
                max_side: 1600,
                timeout: Duration::from_secs(120),
            })
            .expect("识别应当成功");
        println!(
            "backend elapsed_ms={} total={started:?} lines={} text={:?}",
            output.elapsed_ms,
            output.lines.len(),
            output.text
        );
        assert!(
            output.text.to_uppercase().contains("HELLO"),
            "text={:?}",
            output.text
        );
        assert_eq!(output.lines.len(), 2);
        assert!(output.lines[0].score > 0.5);
    }
}
