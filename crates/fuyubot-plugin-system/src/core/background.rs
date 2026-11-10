//! 卡片背景图的获取与缓存。
//!
//! TS 版 `system.vue` 直接把一张 B 站图当 `<img>` 背景。hyper-render 0.2.2 不加载
//! `<img>`（也不认 data URI），所以背景改由 Rust 下载后交给 [`crate::render`] 合成。
//!
//! 设计上**永远不阻塞卡片的主流程**：下载失败 / 超时 / 缓存坏了都只记一条日志，
//! 渲染退回纯渐变背景。
//!
//! 缓存落在系统临时目录（`fuyubot-system-bg-<url哈希>.bin`），按 URL 区分；
//! 关掉缓存（`background_cache = false`）时每次命令都会重新下载。

use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use crate::core::config::Config;

/// 单次下载超时。
const FETCH_TIMEOUT: Duration = Duration::from_secs(6);
/// 背景图体积上限（防止配置写错拉回一个几百 MB 的文件）。
const MAX_BYTES: u64 = 24 * 1024 * 1024;

/// 按配置拿一张背景图；任何一步失败都返回 `None`（调用方用渐变兜底）。
pub fn load(config: &Config) -> Option<Vec<u8>> {
    let url = config.background_url.as_deref()?.trim();
    if url.is_empty() {
        return None;
    }

    let cache_path = cache_path(url);
    if config.background_cache
        && let Some(bytes) = read_cache(&cache_path)
    {
        return Some(bytes);
    }

    match download(url) {
        Ok(bytes) => {
            if config.background_cache {
                write_cache(&cache_path, &bytes);
            }
            Some(bytes)
        }
        Err(error) => {
            eprintln!("system: 背景图下载失败（{url}）: {error}");
            None
        }
    }
}

/// 缓存文件名：URL 哈希（避免把 URL 里的特殊字符当路径）。
fn cache_path(url: &str) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    std::env::temp_dir().join(format!("fuyubot-system-bg-{:016x}.bin", hasher.finish()))
}

fn read_cache(path: &PathBuf) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(bytes)
}

fn write_cache(path: &PathBuf, bytes: &[u8]) {
    // 先写临时文件再改名：并发命令不会读到半截文件。
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, bytes).is_ok() && std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::AgentBuilder::new()
        .timeout(FETCH_TIMEOUT)
        .build()
        .get(url)
        .set("User-Agent", "Mozilla/5.0 (compatible; fuyubot-system)")
        .call()
        .map_err(|error| error.to_string())?;
    let length = response
        .header("content-length")
        .and_then(|value| value.trim().parse::<u64>().ok());
    if length.is_some_and(|length| length > MAX_BYTES) {
        return Err(format!("背景图超过 {} MiB", MAX_BYTES / 1024 / 1024));
    }
    let mut reader = response.into_reader().take(MAX_BYTES + 1);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Err("响应为空".to_owned());
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("背景图超过 {} MiB", MAX_BYTES / 1024 / 1024));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_depends_on_url() {
        let first = cache_path("https://example.com/a.jpg");
        let second = cache_path("https://example.com/b.jpg");
        assert_ne!(first, second);
        assert_eq!(
            first,
            cache_path("https://example.com/a.jpg"),
            "同一个 URL 必须命中同一个缓存文件"
        );
    }

    #[test]
    fn load_returns_none_without_url() {
        let config = Config {
            background_url: None,
            ..Config::default()
        };
        assert!(load(&config).is_none());
    }
}
