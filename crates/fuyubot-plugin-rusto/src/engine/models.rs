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
//! `model_dir` 为空时依次尝试（都先转绝对路径）：
//!
//! 1. `<工作目录>/plugins/fengyegf.rusto/models` —— 随包分发的离线模型；
//! 2. 动态库自身位置推出的 `<插件目录>/models`，以及再往上 1–2 层的 `models`
//!    （从 `/proc/self/maps` 读真实路径）——宿主的工作目录不等于插件目录时，
//!    靠这条兜住；
//! 3. `<工作目录>/data/plugins/rusto/models` —— 历史下载落点；
//! 4. `<工作目录>/models`；
//! 5. rusto 自身缓存目录。
//!
//! 注意工作目录不一定是插件根（Linux 守护进程常常是 `/`）：直接拼相对路径会拼出
//! `/data/plugins/...` 这种系统目录，`create_dir_all` 直接 EACCES——所以候选目录
//! 必须包含「动态库自身位置」这条。
//!
//! 格式取舍：.mnn 优先。两种格式都由 `rusto-nn` 的纯 Rust 解析器读，
//! 不需要打开 `rten` feature；性能基准与选型记录见提交历史。
//!
//! 下载：自己用 ureq（纯 Rust + rustls）走 HTTPS，**不起** curl / wget / powershell
//! 子进程——Linux 上不一定装了 curl / wget，起进程的下载器在精简环境里只会失败。
//! URL 是 `{源}/{preset}/{det.mnn|rec.mnn|dict.txt}`：配置了 `mirror` 就只用它，
//! 否则先试官方源 `https://huggingface.co/byrizki/rusto-models/resolve/main`，
//! 连不上再试 `https://hf-mirror.com`（路径一致，国内一般可达）。下载先落
//! `<模型目录>/<preset>/.staging/`，三件套齐了才整体搬进目标目录，避免半套模型。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::config::PLUGIN_NAME;
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

/// 官方源连不上时的备用镜像（路径与 HF 一致，国内一般可达）。
pub const FALLBACK_MODELS_BASE: &str = "https://hf-mirror.com";

/// 缺省预设名（与 `core::config::DEFAULT_MODEL` 一致）。
pub const DEFAULT_PRESET: &str = "ppocrv6-tiny-int8";

/// 动态库文件名里的 stem（与 `Cargo.toml` 的 `[lib] name = "rusto"` 一致）。
#[cfg(target_os = "linux")]
const LIB_STEM: &str = "rusto";

/// 模型文件候选扩展名，按优先级（.mnn 快路径优先）。
const MODEL_EXTS: [&str; 2] = ["mnn", "rten"];

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
    let hint_target = bases
        .first()
        .cloned()
        .unwrap_or_else(current_dir)
        .join(&preset);
    let Some(base) = pick_download_base(&bases) else {
        return Err(EngineError::ModelMissing(format!(
            "找不到模型（预设 {preset}），候选目录都不可写：{}；{}",
            tried(&bases),
            hint(&hint_target)
        )));
    };
    download(config, &preset, &base)
}

/// 选一个能写的目录当下载落点：按候选顺序试建目录，第一个成功的就是它。
///
/// cwd 指向系统目录时（例如守护进程从 `/` 启动），`/plugins/...` 这类候选必然建不
/// 出来；能写的通常是插件目录，也就是动态库位置推出来的那几条。
fn pick_download_base(bases: &[PathBuf]) -> Option<PathBuf> {
    bases
        .iter()
        .find(|base| fs::create_dir_all(base).is_ok())
        .cloned()
}

/// 已经试过的目录清单（错误信息用；最多列 6 个，避免刷屏）。
fn tried(bases: &[PathBuf]) -> String {
    let list = bases
        .iter()
        .take(6)
        .map(|base| base.display().to_string())
        .collect::<Vec<_>>()
        .join("、");
    if bases.len() > 6 {
        format!("已查找 {list} 等 {} 个目录", bases.len())
    } else {
        format!("已查找 {list}")
    }
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
    let cwd = current_dir();
    let mut dirs = Vec::with_capacity(8);
    dirs.extend(bundled_dirs(&cwd));
    dirs.extend(library_model_bases());
    dirs.push(cwd.join(DATA_SUBDIR));
    dirs.push(cwd.join("models"));
    // 用户此前跑过 rusto CLI 时，模型可能已在它的缓存目录里（只读复用）。
    let cache = rusto::models::get_cache_dir(None);
    if !cache.as_os_str().is_empty() {
        dirs.push(absolute(&cache));
    }
    dirs.dedup();
    dirs
}

/// 进程工作目录（拿不到时回退 `.`，保证错误信息里总有可读路径）。
fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// 随包分发的模型目录：`<工作目录>/plugins/<插件 id>/models`。
fn bundled_dirs(cwd: &Path) -> Vec<PathBuf> {
    vec![cwd.join("plugins").join(PLUGIN_NAME).join("models")]
}

/// 由动态库路径推出模型目录候选。
///
/// 发行布局是 `<插件目录>/bin/librusto.so` + `<插件目录>/models/<preset>/`：插件目录
/// 优先（宿主把插件解压到 app 私有目录，模型查找与下载落点都该在这里），再看动态库
/// 同级，最后逐级向上两层（宿主也可能把模型集中放在上层）；每层都试 `<层>/models`
/// 与这层本身，兼容三件套直接摊开的部署。纯路径运算，不碰文件系统（便于单测）。
pub fn model_bases_of(lib: &Path) -> Vec<PathBuf> {
    let Some(bin) = lib.parent() else {
        return Vec::new();
    };
    let mut bases = Vec::new();
    if let Some(root) = bin.parent() {
        bases.push(root.join("models"));
        bases.push(root.to_path_buf());
        if let Some(up) = root.parent() {
            bases.push(up.join("models"));
            bases.push(up.to_path_buf());
            if let Some(top) = up.parent() {
                bases.push(top.join("models"));
                bases.push(top.to_path_buf());
            }
        }
    }
    bases.push(bin.join("models"));
    bases.push(bin.to_path_buf());
    bases
}

/// 动态库位置推出的模型目录候选（cwd 不等于插件目录时靠它）。
fn library_model_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    for lib in library_paths() {
        bases.extend(model_bases_of(&lib));
    }
    bases
}

/// 宿主 `dlopen` 进来的本插件动态库路径。
///
/// `std::env::current_exe()` 在宿主进程里指向宿主自身，没法用来定位插件目录；
/// `/proc/self/maps` 里能读到 `librusto.so` 被映射的真实路径。
#[cfg(target_os = "linux")]
fn library_paths() -> Vec<PathBuf> {
    let Ok(maps) = fs::read_to_string("/proc/self/maps") else {
        return Vec::new();
    };
    library_paths_from_maps(&maps, &library_file_name())
}

#[cfg(not(target_os = "linux"))]
fn library_paths() -> Vec<PathBuf> {
    Vec::new()
}

/// 从 `/proc/self/maps` 文本里挑出指定动态库的路径（同一库的多段映射去重）。
///
/// maps 行是「地址 权限 偏移 设备 inode 路径」，前五个字段里没有斜杠，
/// 所以第一个斜杠就是路径起点（路径含空格也能整段取到）。抽成纯函数是为了
/// 能在非 Linux 的构建机上用假 maps 验证这条解析路径。
#[cfg(any(target_os = "linux", test))]
fn library_paths_from_maps(maps: &str, name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in maps.lines() {
        let Some(start) = line.find('/') else {
            continue;
        };
        let path = line[start..].trim_end();
        if !path.ends_with(name) {
            continue;
        }
        paths.push(PathBuf::from(path));
    }
    paths.dedup();
    paths
}

/// 动态库文件名（与 `[lib] name` 拼出来的 `librusto.so`）。
#[cfg(target_os = "linux")]
fn library_file_name() -> String {
    format!("lib{LIB_STEM}.so")
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

/// 下载源顺序：配置了 `mirror` 就只用它；没配则官方源 + 备用镜像各试一遍。
fn model_sources(config: &EngineConfig) -> Vec<String> {
    if let Some(mirror) = config
        .mirror
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return vec![mirror.trim_end_matches('/').to_owned()];
    }
    vec![
        DEFAULT_MODELS_BASE.to_owned(),
        FALLBACK_MODELS_BASE.to_owned(),
    ]
}

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

    let mut notes: Vec<String> = Vec::new();
    // 不做跨精度回退：ppocrv6-tiny 找不到时绝不自动换成 -int8（反之亦然），
    // 否则用户以为换了精度却拿到别的权重。
    for source in model_sources(config) {
        let prefix = format!("{source}/{preset}");
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

/// 用 ureq 把三个文件取到 staging（纯 Rust HTTPS，不依赖外部命令）。
fn fetch_into(prefix: &str, files: &[&str; 3], staging: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("fuyubot-plugin-rusto/", env!("CARGO_PKG_VERSION")))
        .build();
    for name in files {
        let url = format!("{prefix}/{name}");
        let target = staging.join(name);
        let _ = fs::remove_file(&target);
        let response = agent.get(&url).call().map_err(describe)?;
        let declared = response
            .header("content-length")
            .and_then(|text| text.trim().parse::<u64>().ok());
        let mut reader = response.into_reader();
        let mut file = fs::File::create(&target)
            .map_err(|error| format!("建 {} 失败：{error}", target.display()))?;
        let written = std::io::copy(&mut reader, &mut file)
            .map_err(|error| format!("写 {} 失败：{error}", target.display()))?;
        drop(file);
        if written == 0 {
            let _ = fs::remove_file(&target);
            return Err(format!("{url} 返回空文件"));
        }
        if let Some(declared) = declared
            && declared != written
        {
            let _ = fs::remove_file(&target);
            return Err(format!("{url} 只收到 {written}/{declared} 字节"));
        }
    }
    Ok(())
}

/// 把 ureq 的错误压成一句人话（服务器答了 vs 根本没连上）。
fn describe(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, _response) => format!("HTTP {status}"),
        ureq::Error::Transport(transport) => transport.to_string(),
    }
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

    #[test]
    fn library_layout_models_are_discovered() {
        let temp = TempDir::new("library-layout");
        let plugin = temp.0.join(PLUGIN_NAME);
        let lib = plugin.join("bin").join("librusto.so");
        touch(&lib);
        let models = plugin.join("models").join(DEFAULT_PRESET);
        touch(&models.join("det.mnn"));
        touch(&models.join("rec.mnn"));
        touch(&models.join("dict.txt"));
        let paths = model_bases_of(&lib)
            .iter()
            .find_map(|base| find_in(base, DEFAULT_PRESET))
            .expect("动态库所在插件目录应能解析到模型");
        assert_eq!(paths.format(), "mnn");
    }

    #[test]
    fn library_bases_prefer_plugin_root() {
        let bases = model_bases_of(Path::new("/data/plugins/fengyegf.rusto/bin/librusto.so"));
        assert_eq!(
            bases[0],
            PathBuf::from("/data/plugins/fengyegf.rusto/models")
        );
        assert!(bases.contains(&PathBuf::from("/data/plugins/fengyegf.rusto/bin/models")));
        assert!(bases.contains(&PathBuf::from("/data/plugins/fengyegf.rusto")));
    }

    #[test]
    fn download_base_skips_unwritable_candidates() {
        let temp = TempDir::new("download-base");
        let blocker = temp.0.join("blocked");
        fs::write(&blocker, b"x").expect("写占位文件");
        let candidates = vec![
            blocker.join("models"), // 祖先不是目录：create_dir_all 必然失败
            temp.0.join("writable").join("models"),
        ];
        let base = pick_download_base(&candidates).expect("应选到可写目录");
        assert_eq!(base, candidates[1]);
    }

    #[test]
    fn model_sources_prefer_configured_mirror() {
        assert_eq!(
            model_sources(&config(None)),
            vec![
                DEFAULT_MODELS_BASE.to_owned(),
                FALLBACK_MODELS_BASE.to_owned()
            ]
        );
        let mut custom = config(None);
        custom.mirror = Some(" https://hf-mirror.com/ ".to_owned());
        assert_eq!(
            model_sources(&custom),
            vec!["https://hf-mirror.com".to_owned()]
        );
    }

    #[test]
    fn maps_lines_yield_plugin_library() {
        // 真实 /proc/self/maps 的形态：同一个 .so 有多段映射，旁边还有无关的 .so 与匿名段。
        let maps = "\
7c1a0000-7c1a1000 r--p 00000000 fd:00 1234 /data/data/com.example.fuyubot/files/plugins/fengyegf.rusto/bin/librusto.so
7c1a1000-7c1a2000 r-xp 00010000 fd:00 1234 /data/data/com.example.fuyubot/files/plugins/fengyegf.rusto/bin/librusto.so
7c1a3000-7c1a4000 r-xp 00000000 fd:00 9999 /system/lib64/libc.so
7c1a5000-7c1a6000 rw-p 00000000 00:00 0 [anon:dalvik]
";
        let paths = library_paths_from_maps(maps, "librusto.so");
        assert_eq!(paths.len(), 1, "同一库的多段映射应去重");
        let plugin = PathBuf::from("/data/data/com.example.fuyubot/files/plugins/fengyegf.rusto");
        assert_eq!(paths[0], plugin.join("bin/librusto.so"));
        assert!(
            model_bases_of(&paths[0]).contains(&plugin.join("models")),
            "插件根目录下的 models 必须进候选"
        );
    }
}
