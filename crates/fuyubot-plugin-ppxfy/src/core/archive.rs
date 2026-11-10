//! 压缩包识别：从下载结果里挑出该解压的那一个，以及找出嵌套压缩包。
//!
//! 支持普通压缩包与分卷（`.001` / `.part1.rar` / `.r00`）。分卷时永远从**编号最小**
//! 的一卷开始——7z 与 HatchZip 都会自己顺着分卷往下读。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::util::fs as fsutil;

/// 压缩包命名的几种形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `xxx.part1.rar`
    Part,
    /// `xxx.r00`
    RarVolume,
    /// `xxx.001`
    Numbered,
    /// 普通压缩包
    Archive,
}

/// 嵌套解压的最大层数，防止无限套娃 / zip bomb。
const MAX_NESTED_DEPTH: usize = 5;

/// 判断一个文件名是否是可解压的压缩包，返回它的类型与分卷编号。
fn classify(name: &str) -> Option<(Kind, Option<u64>)> {
    let lower = name.to_ascii_lowercase();

    if let Some(stem) = lower.strip_suffix(".rar") {
        if let Some(position) = stem.rfind("part") {
            let digits = &stem[position + 4..];
            let separator_ok =
                position > 0 && matches!(stem.as_bytes()[position - 1], b'.' | b'_' | b'-');
            if separator_ok && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                return Some((Kind::Part, digits.parse().ok()));
            }
        }
        return Some((Kind::Archive, None));
    }

    if let Some(position) = lower.rfind(".r") {
        let digits = &lower[position + 2..];
        if digits.len() >= 2 && digits.bytes().all(|b| b.is_ascii_digit()) {
            return Some((Kind::RarVolume, digits.parse().ok()));
        }
    }

    let (_, extension) = lower.rsplit_once('.')?;
    if !extension.is_empty() && extension.bytes().all(|b| b.is_ascii_digit()) {
        return Some((Kind::Numbered, extension.parse().ok()));
    }
    if matches!(
        extension,
        "zip" | "rar" | "7z" | "7zip" | "tar" | "gz" | "bz2"
    ) {
        return Some((Kind::Archive, None));
    }
    None
}

/// 文件名看起来是不是压缩包。
pub fn is_archive(name: &str) -> bool {
    classify(name).is_some()
}

/// 从下载结果里挑出要解压的压缩包。
///
/// 只有一个文件时直接返回它（用户可能下了一个无扩展名的包）；
/// 多个文件时优先取分卷编号最小的那一卷，其次取普通压缩包。
pub fn pick(paths: &[PathBuf]) -> Option<PathBuf> {
    if paths.len() == 1 {
        return paths.first().cloned();
    }

    let mut candidates: Vec<(PathBuf, Kind, Option<u64>)> = Vec::new();
    for path in paths {
        let name = path.file_name()?.to_string_lossy().into_owned();
        if let Some((kind, number)) = classify(&name) {
            candidates.push((path.clone(), kind, number));
        }
    }
    if candidates.is_empty() {
        return None;
    }

    let first_volume = candidates
        .iter()
        .filter(|(_, _, number)| number.is_some())
        .min_by(|left, right| left.2.cmp(&right.2));
    if let Some((path, _, _)) = first_volume {
        return Some(path.clone());
    }

    candidates
        .iter()
        .find(|(_, kind, _)| *kind == Kind::Archive)
        .or_else(|| candidates.first())
        .map(|(path, _, _)| path.clone())
}

/// 递归扫描目录，找出还没处理过的嵌套压缩包（每层目录只取一卷作为起点）。
pub fn find_nested(root: &Path, exclude: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let mut found = Vec::new();

    fn walk(dir: &Path, depth: usize, exclude: &HashSet<PathBuf>, found: &mut Vec<PathBuf>) {
        if depth > MAX_NESTED_DEPTH {
            return;
        }
        let mut archives = Vec::new();
        let mut subdirs = Vec::new();
        for (name, is_dir) in fsutil::list_entries(dir) {
            let path = dir.join(&name);
            if is_dir {
                subdirs.push(path);
            } else if is_archive(&name) {
                archives.push(path);
            }
        }

        if let Some(first) = pick(&archives)
            && !exclude.contains(&fsutil::normalize(&first))
        {
            found.push(first);
        }
        for dir in subdirs {
            walk(&dir, depth + 1, exclude, found);
        }
    }

    walk(root, 1, exclude, &mut found);
    found
}

/// 压缩包集合的主键：去掉分卷后缀与扩展名后的名字（小写）。
///
/// `foo.rar` / `foo.r00` / `foo.part1.rar` / `foo.001` 都归到 `foo`。
fn archive_key(name: &str) -> String {
    let mut key = name.to_ascii_lowercase();

    if let Some(stem) = key.strip_suffix(".rar")
        && let Some(position) = stem.rfind("part")
    {
        let digits = &stem[position + 4..];
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            key = stem[..position]
                .trim_end_matches(['.', '_', '-'])
                .to_owned();
            return key;
        }
    }
    if let Some(position) = key.rfind(".r") {
        let digits = key[position + 2..].to_owned();
        if digits.len() >= 2 && digits.bytes().all(|b| b.is_ascii_digit()) {
            return key[..position].to_owned();
        }
    }
    if let Some((stem, extension)) = key.rsplit_once('.')
        && (extension.bytes().all(|b| b.is_ascii_digit())
            || matches!(
                extension,
                "zip" | "rar" | "7z" | "7zip" | "tar" | "gz" | "bz2"
            ))
    {
        return stem.to_owned();
    }
    key
}

/// 收集与某个压缩包同套的全部文件（含所有分卷）。
pub fn collect_set(archive: &Path) -> Vec<PathBuf> {
    let mut set = vec![archive.to_path_buf()];
    let Some(dir) = archive.parent() else {
        return set;
    };
    let key = archive
        .file_name()
        .map(|name| archive_key(&name.to_string_lossy()))
        .unwrap_or_default();
    let Ok(entries) = fs::read_dir(dir) else {
        return set;
    };

    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_archive(&name) && archive_key(&name) == key {
            let path = entry.path();
            if path != archive {
                set.push(path);
            }
        }
    }
    set
}

/// 删除压缩包及其全部分卷，返回**实际存在过**的文件数。
///
/// HatchZip 就地解压会把压缩包本身吃掉，这时这里返回 0，统计不会被重复计数。
pub fn remove_set(archive: &Path) -> usize {
    let existing: Vec<PathBuf> = collect_set(archive)
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    fsutil::remove_all(&existing).removed.len()
}
