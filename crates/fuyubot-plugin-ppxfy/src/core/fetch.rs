//! 单链接下载：流式写盘 + 进度写进共享进度板 + 可取消。
//!
//! 这里**不发任何消息**：进度只往 [`RowHandle`] 里累加，由进度板的 ticker 线程
//! 统一刷新那一条消息。并发下载多个文件时也只会有一次编辑。

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::host::Ctx;
use crate::host::progress::RowHandle;
use crate::host::task::Task;
use crate::util::fs as fsutil;

/// 读缓冲大小。
const CHUNK_SIZE: usize = 64 * 1024;

/// 下载一个链接到缓存目录，返回落盘路径；失败或取消返回 `None`。
pub fn download(ctx: &Ctx, task: &Task, url: &str, row: &RowHandle) -> Option<PathBuf> {
    let guess = guess_name(url);
    ctx.info(&format!("ppxfy: 开始下载 {url}"));

    let cache_dir = ctx.config.cache_dir.clone();
    if let Err(error) = std::fs::create_dir_all(&cache_dir) {
        ctx.error(&format!("ppxfy: 创建缓存目录失败: {error}"));
        row.rename(format!("❌ {guess}（目录不可写）"));
        return None;
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .build();
    // 明确要求不压缩：否则 Content-Length 是压缩后大小，进度百分比会失真。
    let response = match agent.get(url).set("Accept-Encoding", "identity").call() {
        Ok(response) => response,
        Err(error) => {
            let reason = match error {
                ureq::Error::Status(code, response) => {
                    format!("{code} {}", response.status_text())
                }
                other => other.to_string(),
            };
            ctx.warn(&format!("ppxfy: 请求失败 {url}: {reason}"));
            row.rename(format!("❌ {guess}（{reason}）"));
            return None;
        }
    };

    let name = fsutil::sanitize_filename(&file_name(&response, url));
    let path = cache_dir.join(&name);
    let total = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    ctx.info(&format!(
        "ppxfy: 保存到 {}（{total} bytes）",
        path.display()
    ));

    row.rename(name);
    row.set_total(total);

    let mut file = match File::create(&path) {
        Ok(file) => file,
        Err(error) => {
            ctx.error(&format!("ppxfy: 创建文件失败 {}: {error}", path.display()));
            row.rename("❌ 无法写入缓存目录".to_owned());
            return None;
        }
    };

    let mut reader = response.into_reader();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut failure: Option<&str> = None;

    loop {
        if task.is_cancelled() {
            failure = Some("已取消");
            break;
        }
        if ctx.is_shutdown() {
            failure = Some("插件停止");
            break;
        }
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                ctx.error(&format!("ppxfy: 下载读取失败 {url}: {error}"));
                failure = Some("读取失败");
                break;
            }
        };
        if read == 0 {
            break;
        }
        if let Err(error) = file.write_all(&buffer[..read]) {
            ctx.error(&format!("ppxfy: 写入失败 {}: {error}", path.display()));
            failure = Some("写入失败");
            break;
        }
        row.add(read as u64);
    }

    match failure {
        None => {
            row.complete();
            ctx.info(&format!("ppxfy: 下载完成 {}", path.display()));
            Some(path)
        }
        Some(reason) => {
            drop(file);
            let _ = std::fs::remove_file(&path);
            row.rename(format!("❌ {reason}"));
            None
        }
    }
}

/// 推断文件名：优先 `Content-Disposition`，其次 URL 末段。
fn file_name(response: &ureq::Response, url: &str) -> String {
    if let Some(disposition) = response.header("content-disposition")
        && let Some(name) = from_disposition(disposition)
    {
        return name;
    }
    guess_name(url)
}

/// 从 `Content-Disposition` 里取 `filename=` / `filename*=UTF-8''…`。
fn from_disposition(value: &str) -> Option<String> {
    for part in value.split(';') {
        let part = part.trim();
        for key in ["filename*=", "filename="] {
            if let Some(rest) = part.strip_prefix(key) {
                let rest = rest.trim().trim_matches('"');
                // `UTF-8''name` 形式：取最后一个 `''` 之后的部分。
                let rest = rest.rsplit("''").next().unwrap_or(rest);
                if !rest.is_empty() {
                    return Some(rest.to_owned());
                }
            }
        }
    }
    None
}

/// 从 URL 末段取名字（去掉查询串与片段）。进度板先用它当占位标签。
pub fn guess_name(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let last = without_fragment.rsplit('/').next().unwrap_or("");
    let name = last.split('?').next().unwrap_or("");
    if name.is_empty() {
        "downloaded.file".to_owned()
    } else {
        name.to_owned()
    }
}
