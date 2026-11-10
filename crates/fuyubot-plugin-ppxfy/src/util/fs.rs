//! 文件系统工具：清理、统计、目录列举、同名目录折叠、文件名净化。
//!
//! 这里只有纯本地操作，不碰宿主、不碰消息，便于单独复用与推理。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 缓存目录下的两个固定子目录名。
pub const OUTPUT_DIR: &str = "Output";
pub const ZIP_OUTPUT_DIR: &str = "ZipOutput";

/// 批量删除的结果。
#[derive(Debug, Default)]
pub struct Cleanup {
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

/// 删除文件或目录（目录递归删除）；单个失败不影响其余。
pub fn remove_all(paths: &[PathBuf]) -> Cleanup {
    let mut cleanup = Cleanup::default();
    for path in paths {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                let result = if metadata.is_dir() {
                    fs::remove_dir_all(path)
                } else {
                    fs::remove_file(path)
                };
                match result {
                    Ok(()) => cleanup.removed.push(path.clone()),
                    Err(error) => cleanup.failed.push((path.clone(), error.to_string())),
                }
            }
            // 不存在视为已删除。
            Err(_) => cleanup.removed.push(path.clone()),
        }
    }
    cleanup
}

/// 删除失败时重试。
///
/// Windows 上刚上传完的文件常被占用一小会儿（TDLib 还拿着句柄），
/// 一次 `remove_file` 失败非常常见，**不重试就等于静默留下垃圾**。
pub fn remove_with_retry(paths: &[PathBuf], attempts: u32, delay: Duration) -> Cleanup {
    let attempts = attempts.max(1);
    let mut report = Cleanup::default();
    let mut pending: Vec<PathBuf> = paths.to_vec();
    let mut last_failures: Vec<(PathBuf, String)> = Vec::new();

    for attempt in 0..attempts {
        if pending.is_empty() {
            break;
        }
        let result = remove_all(&pending);
        report.removed.extend(result.removed);
        pending = result.failed.iter().map(|(path, _)| path.clone()).collect();
        last_failures = result.failed;
        if !pending.is_empty() && attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }

    report.failed = last_failures;
    report
}

/// 清空目录内容但保留目录本身；目录不存在时什么也不做。
pub fn clear_dir(dir: &Path) -> Cleanup {
    let mut targets = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            targets.push(entry.path());
        }
    }
    remove_with_retry(&targets, 3, Duration::from_millis(500))
}

/// 清理一次下载流程的全部产物：`Output/`、`ZipOutput/`（连同目录本身）与缓存根目录下的文件。
///
/// 两个子目录整体删除，下一次流程会重新创建；
/// `help.jpg` 之类的其它插件缓存不动，这里只对「根目录下的普通文件」动手。
pub fn clear_download_artifacts(cache_dir: &Path) -> Cleanup {
    let mut cleanup = Cleanup::default();
    for name in [OUTPUT_DIR, ZIP_OUTPUT_DIR] {
        let dir = cache_dir.join(name);
        let report = remove_with_retry(&[dir], 3, Duration::from_millis(500));
        cleanup.removed.extend(report.removed);
        cleanup.failed.extend(report.failed);
    }

    if let Ok(entries) = fs::read_dir(cache_dir) {
        let files: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .filter(|path| path.file_name().is_none_or(|name| name != "help.jpg"))
            .collect();
        let report = remove_with_retry(&files, 3, Duration::from_millis(500));
        cleanup.removed.extend(report.removed);
        cleanup.failed.extend(report.failed);
    }
    cleanup
}

/// 递归统计文件或目录的字节数（读不到的条目按 0 计）。
pub fn size_of(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().map(|entry| size_of(&entry.path())).sum()
}

/// 列出目录内容：目录在前、按名称排序，只返回一层。
pub fn list_entries(dir: &Path) -> Vec<(String, bool)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().ok()?.is_dir();
            Some((name, is_dir))
        })
        .collect();
    items.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    items
}

/// 人类可读的字节数（用于进度文案）。
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// 规范化为可比较的绝对路径（拿不到就返回原路径）。
pub fn normalize(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 折叠同名套娃目录：`X/X/内容` → `X/内容`。
///
/// 嵌套解压常见这种结构。只在「父目录下恰好有一个同名子目录」且没有重名冲突时折叠，
/// 有冲突就整体跳过，绝不覆盖已有条目。返回折叠掉的层数。
pub fn flatten_same_name_dirs(root: &Path) -> usize {
    /// 防御异常深度的目录结构。
    const MAX_DEPTH: usize = 20;
    /// 单层连续同名嵌套的上限。
    const MAX_FOLD_PER_DIR: usize = 10;

    fn fold_once(dir: &Path) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        let entries: Vec<(String, bool)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                Some((name, entry.file_type().ok()?.is_dir()))
            })
            .collect();

        let parent_name = dir
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if parent_name.is_empty() {
            return false;
        }
        let same_name: Vec<&String> = entries
            .iter()
            .filter(|(name, is_dir)| *is_dir && name.to_lowercase() == parent_name)
            .map(|(name, _)| name)
            .collect();
        if same_name.len() != 1 {
            return false;
        }

        let inner_dir = dir.join(same_name[0]);
        let Ok(inner) = fs::read_dir(&inner_dir) else {
            return false;
        };
        let inner: Vec<String> = inner
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();

        // 内外重名就放弃，避免覆盖。
        let outer: Vec<String> = entries
            .iter()
            .map(|(name, _)| name.to_lowercase())
            .collect();
        if inner
            .iter()
            .any(|name| outer.contains(&name.to_lowercase()))
        {
            return false;
        }

        for name in inner {
            if fs::rename(inner_dir.join(&name), dir.join(&name)).is_err() {
                return false;
            }
        }
        fs::remove_dir(&inner_dir).is_ok()
    }

    fn walk(dir: &Path, depth: usize, folded: &mut usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for _ in 0..MAX_FOLD_PER_DIR {
            if !fold_once(dir) {
                break;
            }
            *folded += 1;
        }
        for (name, is_dir) in list_entries(dir) {
            if is_dir {
                walk(&dir.join(name), depth + 1, folded);
            }
        }
    }

    let mut folded = 0;
    walk(root, 1, &mut folded);
    folded
}

/// 净化文件名：去掉路径、URL 查询串与 Windows 非法字符。
pub fn sanitize_filename(name: &str) -> String {
    let decoded = percent_decode(name);
    let trimmed = decoded.split('?').next().unwrap_or(&decoded);
    let base = Path::new(trimmed)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| trimmed.to_owned());

    let cleaned: String = base
        .trim()
        .trim_matches('"')
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            ch if (ch as u32) < 0x20 => '_',
            ch => ch,
        })
        .collect();

    let cleaned = cleaned.trim().trim_matches('.').trim().to_owned();
    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned
    }
}

/// 仅解 `%XX` 序列的百分号编码（不引入额外依赖）。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
