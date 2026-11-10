//! 7z 命令行封装（解压兜底 / 打包）。
//!
//! 两条硬要求：
//!
//! - **不能挂死**：stdin 直接接空设备，密码一律显式传 `-p`（无密码传空 `-p`），
//!   否则 7z 会交互式索要密码把进程永久卡住；
//! - **不能死锁**：输出重定向到临时文件而不是管道，避免「子进程写满管道、父进程没在读」。
//!
//! 超时靠轮询 `try_wait`，到点直接 kill。

use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;

/// 轮询子进程状态的间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Error)]
pub enum Error {
    #[error("启动 7z 失败: {0}")]
    Spawn(String),
    #[error("7z 执行超时")]
    Timeout,
    #[error("7z 退出码 {code}: {log}")]
    Failed { code: i32, log: String },
    #[error("7z 需要密码: {log}")]
    Password { log: String },
}

/// 一次 7z 运行的结果。
#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub log: String,
}

/// 运行 7z；`cwd` 用于「以某目录为工作目录打包其内容」。
pub fn run(
    exe: &Path,
    args: &[String],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<Output, Error> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let log_path = std::env::temp_dir().join(format!(
        "ppxfy-7z-{}-{}.log",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let log_file = File::create(&log_path).map_err(|error| Error::Spawn(error.to_string()))?;
    let stderr = log_file
        .try_clone()
        .map_err(|error| Error::Spawn(error.to_string()))?;

    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr));
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    let mut child = command
        .spawn()
        .map_err(|error| Error::Spawn(format!("{}: {error}", exe.display())))?;

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status.code().unwrap_or(-1)),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = std::fs::remove_file(&log_path);
                return Err(Error::Spawn(error.to_string()));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&log_path);
            return Err(Error::Timeout);
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);
    Ok(Output {
        code: status.unwrap_or(-1),
        log,
    })
}

/// 解压压缩包到 `out_dir`（分卷只需传第一卷）。
pub fn extract(
    exe: &Path,
    archive: &Path,
    out_dir: &Path,
    password: Option<&str>,
    timeout: Duration,
) -> Result<(), Error> {
    let mut args = vec![
        "x".to_owned(),
        archive.display().to_string(),
        format!("-o{}", out_dir.display()),
        "-y".to_owned(),
        // 显式给密码参数（哪怕为空），杜绝交互式等待。
        password.map_or_else(|| "-p".to_owned(), |value| format!("-p{value}")),
    ];
    args.push("-bso0".to_owned());

    let output = run(exe, &args, None, timeout)?;
    if output.code == 0 {
        return Ok(());
    }
    Err(classify(output))
}

/// 把非零退出码归类：密码问题单独一类，便于上层重新询问密码。
pub fn classify(output: Output) -> Error {
    let log = detail(&output.log);
    let lower = output.log.to_lowercase();
    if lower.contains("wrong password")
        || lower.contains("enter password")
        || lower.contains("encrypted")
        || lower.contains("can not open encrypted archive")
    {
        return Error::Password { log };
    }
    Error::Failed {
        code: output.code,
        log,
    }
}

/// 只保留 7z 的 ERROR 行，避免把整段 banner 塞进聊天消息。
pub fn detail(log: &str) -> String {
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| line.starts_with("ERROR:") || line.contains("Error"))
        .collect();
    if lines.is_empty() {
        let tail: String = log.trim().chars().take(300).collect();
        return if tail.is_empty() {
            "7z 未输出错误详情".to_owned()
        } else {
            tail
        };
    }
    lines.join("\n")
}
