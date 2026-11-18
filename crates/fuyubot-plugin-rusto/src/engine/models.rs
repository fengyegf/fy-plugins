//! 模型解析：预设名 → 三个模型文件（det / rec / dict）的本地路径。
//!
//! 目录约定：`<model_dir>/<preset>/{det,rec,dict}`，预设默认 `ppocrv6-tiny-int8`：
//!
//! ```text
//! data/plugins/rusto/models/ppocrv6-tiny-int8/
//!     det.mnn    rec.mnn    dict.txt      # 快路径（优先）
//!     det.rten   rec.rten                 # 慢路径（兜底）
//! ```
//!
//! 探测顺序（每目录内）：`det.mnn/rec.mnn → det.rten/rec.rten` → 文件名模糊匹配
//! （`*det*.{mnn,rten}` / `*rec*.{mnn,rten}` / `*dict*.{txt,json}`）。
//! det 与 rec 必须同格式，否则视为不完整。
//!
//! `model_dir` 为空时依次尝试（相对进程工作目录，返回前转绝对路径）：
//! `./data/plugins/rusto/models` → `./models` → rusto 自身缓存目录。
//!
//! 格式取舍：.mnn 优先。两种格式都由 `rusto-nn` 的纯 Rust 解析器读，
//! 不需要打开 `rten` feature；性能基准与选型记录见提交历史。
//!
//! 下载：rusto 的下载器只认进程级环境变量 `RUSTO_MODELS_URL` 做前缀替换
//! （`{值}/{asset_filename}`），且内置「文件名 → HF 路径」映射只覆盖 .rten，
//! 因此这里把 `RUSTO_MODELS_URL` 设成 `{mirror|官方源}/{preset}` 后调
//! `download_asset("det.mnn", 目标)`，得到
//! `https://huggingface.co/byrizki/rusto-models/resolve/main/ppocrv6-tiny-int8/det.mnn`。
//! 下载器内部用 curl / wget / powershell 子进程 + `.part` + `rename`（原子）；
//! 本模块再套一层 staging 目录，三件套齐了才整体搬进目标目录，避免半套模型。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::engine::{EngineConfig, EngineError};

/// 一次识别需要的三个文件。
#[derive(Debug, Clone)]
pub struct ModelPaths {
    /// 文本检测模型。
    pub det: PathBuf,
    /// 文本识别模型。
    pub rec: PathBuf,
    /// 字符字典。
    pub dict: PathBuf,
}

/// 模型目录的相对约定：宿主的持久数据目录。
pub const DATA_SUBDIR: &str = "data/plugins/rusto/models";

/// 官方模型仓库根（与 rusto 内置常量一致）。
pub const DEFAULT_MODELS_BASE: &str = "https://huggingface.co/byrizki/rusto-models/resolve/main";

/// 缺省预设名（与 `core::config::DEFAULT_MODEL` 一致）。
pub const DEFAULT_PRESET: &str = "ppocrv6-tiny-int8";

/// 模型文件候选扩展名，按优先级（.mnn 快路径优先）。
const MODEL_EXTS: [&str; 2] = ["mnn", "rten"];

/// 串行化「写环境变量 + 下载」，避免两个线程同时改 `RUSTO_MODELS_URL`。
static DOWNLOAD_LOCK: Mutex<()> = Mutex::new(());

impl ModelPaths {
    /// 三个文件所在目录（错误信息与状态展示用）。
    pub fn dir(&self) -> PathBuf {
        self.det
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 实际使用的模型格式：`mnn` / `rten` / `unknown`。
    pub fn format(&self) -> &'static str {
        match extension(&self.det).as_deref() {
            Some("mnn") => "mnn",
            Some("rten") => "rten",
            _ => "unknown",
        }
    }
}

/// 把配置解析成文件路径；缺文件时尝试自动下载。
pub fn resolve(config: &EngineConfig) -> Result<ModelPaths, EngineError> {
    let preset = preset_of(config);
    let bases = candidate_dirs(config);
    for base in &bases {
        if let Some(paths) = find_in(base, &preset) {
            return Ok(paths);
        }
    }
    let base = bases
        .first()
        .cloned()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    download(config, &preset, &base)
}

/// 预设名归一化（只用于目录名，不做版本推断）。
pub fn preset_of(config: &EngineConfig) -> String {
    normalize_preset(&config.model)
}

/// 预设名里允许保留的标点（其余换成连字符）。
const PRESET_SAFE: &str = "._-";

/// 非法字符的替换符（避免 clippy 的 single-char push_str 告警）。
const DASH: &str = "-";

/// 预设名清洗：保留 ASCII 字母数字与点/下划线/连字符，其余换成连字符。
pub fn normalize_preset(model: &str) -> String {
    let mut out = String::new();
    for ch in model.trim().chars() {
        if ch.is_ascii_alphanumeric() || PRESET_SAFE.contains(ch) {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push_str(DASH);
        }
    }
    let out = out.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    if out.is_empty() {
        DEFAULT_PRESET.to_owned()
    } else {
        out.to_owned()
    }
}

/// 候选模型根目录（绝对路径，按优先级）。
pub fn candidate_dirs(config: &EngineConfig) -> Vec<PathBuf> {
    if let Some(dir) = &config.model_dir {
        return vec![absolute(dir)];
    }
    let mut dirs = Vec::with_capacity(3);
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(DATA_SUBDIR));
        dirs.push(cwd.join("models"));
    }
    // 用户此前跑过 rusto CLI 时，模型可能已在它的缓存目录里（只读复用）。
    let cache = rusto::models::get_cache_dir(None);
    if !cache.as_os_str().is_empty() {
        dirs.push(absolute(&cache));
    }
    dirs.dedup();
    dirs
}

/// 相对路径转绝对路径（失败时原样返回，至少错误信息能看懂）。
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// 在一个根目录下找齐三个文件：先 `<base>/<preset>/`，再 `<base>/`。
fn find_in(base: &Path, preset: &str) -> Option<ModelPaths> {
    find_in_dir(&base.join(preset)).or_else(|| find_in_dir(base))
}

/// 在单个目录里按「固定名 .mnn → .rten → 模糊匹配」找三个文件。
fn find_in_dir(dir: &Path) -> Option<ModelPaths> {
    if !dir.is_dir() {
        return None;
    }
    let dict = existing(&dir.join("dict.txt"));
    let mut fixed = None;
    for ext in MODEL_EXTS {
        let det = existing(&dir.join(format!("det.{ext}")));
        let rec = existing(&dir.join(format!("rec.{ext}")));
        if let (Some(det), Some(rec)) = (det, rec) {
            fixed = Some((det, rec));
            break;
        }
    }
    if let (Some((det, rec)), Some(dict)) = (fixed, dict.clone()) {
        return Some(ModelPaths { det, rec, dict });
    }
    // 兜底：扫目录模糊匹配（rusto 下载器落盘的 ppocrv6_tiny_det.rten 之类会命中）。
    let names = list_files(dir);
    let det = pick(dir, &names, &["det", "detection"], &MODEL_EXTS)?;
    let rec = pick(dir, &names, &["rec", "recognition"], &MODEL_EXTS)?;
    let dict = match dict {
        Some(path) => path,
        None => pick(dir, &names, &["dict", "keys", "vocab"], &["txt", "json"])?,
    };
    if extension(&det) != extension(&rec) {
        return None;
    }
    Some(ModelPaths { det, rec, dict })
}

/// 目录内的文件名（排序，保证确定性）。
fn list_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort();
    names
}

/// 文件存在且非空（0 字节视为不存在，避免把断点残留当模型）。
fn existing(path: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() > 0).then(|| path.to_path_buf())
}

/// 按关键字 + 扩展名挑一个已存在的文件名（大小写不敏感）。
fn pick(dir: &Path, names: &[String], keywords: &[&str], exts: &[&str]) -> Option<PathBuf> {
    for name in names {
        let lower = name.to_ascii_lowercase();
        let ext_ok = exts.iter().any(|ext| lower.ends_with(&format!(".{ext}")));
        if !ext_ok {
            continue;
        }
        if !keywords.iter().any(|keyword| lower.contains(keyword)) {
            continue;
        }
        if let Some(found) = existing(&dir.join(name)) {
            return Some(found);
        }
    }
    None
}

/// 小写扩展名。
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

/// 下载时依次尝试的 (格式, 三个文件名)。
const DOWNLOAD_PLAN: [(&str, [&str; 3]); 2] = [
    ("mnn", ["det.mnn", "rec.mnn", "dict.txt"]),
    ("rten", ["det.rten", "rec.rten", "dict.txt"]),
];

/// 下载三件套到 `<base>/<preset>/`（staging → 原子搬入）。
fn download(config: &EngineConfig, preset: &str, base: &Path) -> Result<ModelPaths, EngineError> {
    let target = base.join(preset);
    let staging = target.join(".staging");
    if let Err(error) = fs::create_dir_all(&staging) {
        return Err(EngineError::ModelMissing(format!(
            "无法创建模型目录 {}：{error}；{}",
            staging.display(),
            hint(&target)
        )));
    }

    let _guard = DOWNLOAD_LOCK
        .lock()
        .map_err(|_| EngineError::Internal("模型下载锁中毒".to_owned()))?;

    let mirror = config
        .mirror
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(DEFAULT_MODELS_BASE)
        .trim_end_matches('/')
        .to_owned();

    let mut notes: Vec<String> = Vec::new();
    // 不做跨精度回退：ppocrv6-tiny 找不到时绝不自动换成 -int8（反之亦然），
    // 否则用户以为换了精度却拿到别的权重。
    let candidates = [preset.to_owned()];

    for candidate in &candidates {
        let prefix = format!("{mirror}/{candidate}");
        for (format, files) in DOWNLOAD_PLAN {
            if let Err(error) = fetch_into(&prefix, &files, &staging) {
                notes.push(format!("{prefix}/{} [{}] {error}", files[0], format));
                continue;
            }
            let det = move_file(&staging.join(files[0]), &target.join(files[0]))?;
            let rec = move_file(&staging.join(files[1]), &target.join(files[1]))?;
            let dict = move_file(&staging.join(files[2]), &target.join(files[2]))?;
            let _ = fs::remove_dir_all(&staging);
            return Ok(ModelPaths { det, rec, dict });
        }
    }

    let _ = fs::remove_dir_all(&staging);
    Err(EngineError::ModelMissing(format!(
        "模型自动下载失败（需要联网，或手动放文件）。尝试过：{}；{}",
        notes.join(" / "),
        hint(&target)
    )))
}

/// 用 rusto 的下载器把三个文件取到 staging。
fn fetch_into(prefix: &str, files: &[&str; 3], staging: &Path) -> Result<(), String> {
    // rusto 的下载器没有 per-call 源地址参数，只认进程级环境变量做前缀拼接：
    // get_asset_download_urls() 在值不以斜杠结尾时拼成 “{值}/{asset_filename}”。
    // SAFETY: 写环境变量在多线程进程里不安全（Rust 2024 起 set_var 为 unsafe）。
    // 这里由 DOWNLOAD_LOCK 串行化，且只在「确实缺模型、即将下载」的冷路径上写。
    unsafe { std::env::set_var("RUSTO_MODELS_URL", prefix) };
    for name in files {
        let target = staging.join(name);
        let _ = fs::remove_file(&target);
        rusto::download::download_asset(name, &target).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 把 staging 里的文件搬到目标位置（同盘 rename；跨盘退回复制）。
fn move_file(from: &Path, to: &Path) -> Result<PathBuf, EngineError> {
    if fs::rename(from, to).is_err() {
        fs::copy(from, to).map_err(|error| {
            EngineError::ModelMissing(format!(
                "无法把模型 {} 放到 {}：{error}",
                from.display(),
                to.display()
            ))
        })?;
        let _ = fs::remove_file(from);
    }
    Ok(to.to_path_buf())
}

/// 统一的修复提示。
fn hint(target: &Path) -> String {
    format!(
        "需要 det/rec/dict 三个文件放在 {}（.mnn 快路径优先，.rten 兜底）；\
         或把配置里的 model_dir 指向已有目录后重载插件；手动放好后也可执行 /ocr warm。\
         可用预设：ppocrv6-tiny-int8 / ppocrv6-tiny / ppocrv6-tiny-fp16 /\
         ppocrv6-small / ppocrv6-small-int8 / ppocrv6-medium / ppocrv6-medium-int8\
         （不会跨精度自动回退）",
        target.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(model_dir: Option<PathBuf>) -> EngineConfig {
        EngineConfig {
            model: DEFAULT_PRESET.to_owned(),
            model_dir,
            mirror: None,
            rec_workers: 0,
            concurrency: 1,
        }
    }

    /// 测试用临时根目录：放在 target/ 下（系统临时目录在受限沙箱里可能不可写）。
    fn temp_root() -> PathBuf {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent().and_then(|path| path.parent())
        {
            let root = dir.join("rusto-models-test");
            let _ = fs::create_dir_all(&root);
            return root;
        }
        PathBuf::from("target/rusto-models-test")
    }

    /// 临时目录：Drop 时清理。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = format!(
                "rusto-models-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or(0)
            );
            let path = temp_root().join(unique);
            fs::create_dir_all(&path).expect("建立临时目录");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("建立父目录");
        }
        fs::write(path, b"x").expect("写文件");
    }

    #[test]
    fn normalize_preset_strips_unsafe_characters() {
        assert_eq!(
            normalize_preset(" PP-OCRv6 Tiny/Int8 "),
            "pp-ocrv6-tiny-int8"
        );
        assert_eq!(normalize_preset("   "), DEFAULT_PRESET);
    }

    #[test]
    fn mnn_is_preferred_over_rten() {
        let temp = TempDir::new("prefer-mnn");
        let dir = temp.0.join(DEFAULT_PRESET);
        touch(&dir.join("det.rten"));
        touch(&dir.join("rec.rten"));
        touch(&dir.join("det.mnn"));
        touch(&dir.join("rec.mnn"));
        touch(&dir.join("dict.txt"));
        let paths = find_in(&temp.0, DEFAULT_PRESET).expect("应解析成功");
        assert_eq!(paths.format(), "mnn");
    }

    #[test]
    fn falls_back_to_rten() {
        let temp = TempDir::new("fallback-rten");
        let dir = temp.0.join(DEFAULT_PRESET);
        touch(&dir.join("det.rten"));
        touch(&dir.join("rec.rten"));
        touch(&dir.join("dict.txt"));
        let paths = find_in(&temp.0, DEFAULT_PRESET).expect("应解析成功");
        assert_eq!(paths.format(), "rten");
    }

    #[test]
    fn mixed_formats_are_rejected() {
        let temp = TempDir::new("mixed");
        let dir = temp.0.join(DEFAULT_PRESET);
        touch(&dir.join("det.mnn"));
        touch(&dir.join("rec.rten"));
        touch(&dir.join("dict.txt"));
        assert!(find_in_dir(&dir).is_none());
    }

    #[test]
    fn rusto_downloader_file_names_are_recognized() {
        let temp = TempDir::new("rusto-flat");
        let dir = temp.0.join(DEFAULT_PRESET);
        touch(&dir.join("ppocrv6_tiny_int8_det.rten"));
        touch(&dir.join("ppocrv6_tiny_int8_rec.rten"));
        touch(&dir.join("ppocrv6_dict.txt"));
        let paths = find_in(&temp.0, DEFAULT_PRESET).expect("应解析成功");
        assert!(paths.det.to_string_lossy().contains("det"));
        assert!(paths.dict.to_string_lossy().contains("dict"));
    }

    #[test]
    fn empty_files_are_ignored() {
        let temp = TempDir::new("empty");
        let dir = temp.0.join(DEFAULT_PRESET);
        fs::create_dir_all(&dir).expect("建目录");
        fs::write(dir.join("det.mnn"), b"").expect("写空文件");
        touch(&dir.join("rec.mnn"));
        touch(&dir.join("dict.txt"));
        assert!(find_in_dir(&dir).is_none());
    }

    #[test]
    fn model_dir_is_absolutized() {
        let dirs = candidate_dirs(&config(Some(PathBuf::from("relative/models"))));
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].is_absolute());
    }
}
