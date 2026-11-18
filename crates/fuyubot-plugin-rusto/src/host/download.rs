//! 下载图片到本地：`downloadFile(synchronous = true)`，读 `file.local.path`。
//!
//! 为什么不用「等 updateFile 通知」：小文件常在响应前就下完，通知会漏
//! （本仓 system 插件的 README 记过这个坑）。`synchronous = true` 让 TDLib
//! 在返回前落盘。宿主自带的 `download_file` op 有同一个坑，所以这里直接用 invoke。

use std::path::PathBuf;
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt, RequestId};
use serde_json::{Value, json};

/// 下载一个 TDLib 文件，返回本地路径。
pub fn file_path(host: &HostApi, file_id: i64, timeout: Duration) -> Result<PathBuf, String> {
    let request = json!({
        "file_id": file_id,
        "priority": 1,
        "offset": 0,
        "limit": 0,
        "synchronous": true,
    });
    let request_id = host
        .invoke_value("downloadFile", request)
        .map_err(|error| format!("downloadFile 发送失败：{error}"))?;
    let file = await_response(host, request_id, timeout, "downloadFile")?;
    local_path(&file).ok_or_else(|| {
        format!(
            "downloadFile 返回但没有 local.path（文件可能未下载完成）：{}",
            truncate(&file.to_string(), 400)
        )
    })
}

/// 等一条 `response`，返回 `extra` 或整个响应体。
pub fn await_response(
    host: &HostApi,
    request_id: RequestId,
    timeout: Duration,
    what: &str,
) -> Result<Value, String> {
    let filter = json!({ "event_type": "response", "request_id": request_id.id });
    let response = host
        .wait_for_update_json(filter, timeout.as_millis() as u64)
        .ok_or_else(|| format!("{what} 等待响应超时（{}ms）", timeout.as_millis()))?;
    if response.get("@type").and_then(Value::as_str) == Some("error") {
        let code = response.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("未提供错误信息");
        return Err(format!("{what} 失败：{code} {message}"));
    }
    Ok(response.get("extra").cloned().unwrap_or(response))
}

/// 从 `file` 对象里取本地路径。
fn local_path(file: &Value) -> Option<PathBuf> {
    file.get("local")
        .and_then(|local| local.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
}

/// 日志/错误里截断大 JSON。
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_local_path() {
        let file = json!({ "local": { "path": "C:/tmp/a.jpg" }, "size": 1 });
        assert_eq!(local_path(&file), Some(PathBuf::from("C:/tmp/a.jpg")));
    }

    #[test]
    fn empty_path_is_none() {
        assert_eq!(local_path(&json!({ "local": { "path": "" } })), None);
        assert_eq!(local_path(&json!({})), None);
    }
}
