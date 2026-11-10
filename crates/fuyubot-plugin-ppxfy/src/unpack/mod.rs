//! 解压流程：HatchZip 就地解压优先，遇到不支持的格式退回 7z；密码按需询问；
//! 解压完继续处理嵌套压缩包，最后折叠同名套娃目录。
//!
//! 解压策略是**先无密码试一次**，只有确实需要密码时才向用户询问。
//! HatchZip 在动盘之前就验证密码，失败的代价只是一次本地解析，交互更少。
//!
//! 整个流程只占用一条消息：进度板、密码提示、最终汇总都编辑同一条，避免刷屏。

mod hatch;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::core::archive;
use crate::host::Ctx;
use crate::host::messenger::Sent;
use crate::host::progress::{ProgressBoard, RowHandle};
use crate::host::reply::Wait;
use crate::host::task::Task;
use crate::util::{fs as fsutil, mojibake, seven};

/// 嵌套解压的最大层数。
const MAX_NESTED_DEPTH: usize = 5;
/// 最多向用户索要几次密码（含第一次）。
const MAX_PASSWORD_ATTEMPTS: usize = 2;
/// 进度刷新间隔。
const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum ExtractError {
    /// 缺少密码或密码不正确，且用户没有提供可用的密码。
    #[error("缺少密码或密码不正确")]
    Password,
    /// 用户在密码环节放弃。
    #[error("用户放弃解压")]
    Skipped,
    /// 该格式 HatchZip 不支持（已尝试 7z 兜底）。
    #[error("不支持的压缩格式: {0}")]
    Unsupported(String),
    /// 压缩包损坏或解压中途失败。
    #[error("{0}")]
    Damaged(String),
    /// 外部工具问题（找不到 7z、写盘失败等）。
    #[error("{0}")]
    Tool(String),
    /// 7z 兜底路径超时。
    #[error("解压超时")]
    TimedOut,
}

/// 解压结果。
pub struct Outcome {
    pub output_dir: PathBuf,
    pub entries: Vec<String>,
}

/// 解压入口：把 `archive` 解到缓存目录的 `Output/`，返回最终条目。
pub fn run(
    ctx: &Arc<Ctx>,
    task: &Arc<Task>,
    tips: Sent,
    archive: &Path,
) -> Result<Outcome, ExtractError> {
    let output_dir = ctx.config.cache_dir.join(fsutil::OUTPUT_DIR);
    fs::create_dir_all(&output_dir).map_err(|error| ExtractError::Tool(error.to_string()))?;

    let board = ProgressBoard::start(ctx.messenger.clone(), tips, "解压中", PROGRESS_INTERVAL);
    let row = board.row(label(archive));
    ctx.messenger.edit_text(
        tips.chat_id,
        tips.message_id,
        &format!("解压中: {}，请稍等...", label(archive)),
    );

    let mut password = extract_with_retry(ctx, task, tips, &row, archive, &output_dir, None)?;

    // 7z 路径不会自己删压缩包；HatchZip 已边解压边释放，这里通常为 0。
    let mut removed = archive::remove_set(archive);

    // 嵌套压缩包：复用外层密码，失败时重新询问，最多 MAX_NESTED_DEPTH 层。
    let mut processed: HashSet<PathBuf> = HashSet::new();
    processed.insert(fsutil::normalize(archive));
    let mut nested_count = 0usize;

    for depth in 1..=MAX_NESTED_DEPTH {
        let nested = archive::find_nested(&output_dir, &processed);
        if nested.is_empty() {
            break;
        }
        for inner in nested {
            processed.insert(fsutil::normalize(&inner));
            ctx.messenger.edit_text(
                tips.chat_id,
                tips.message_id,
                &format!(
                    "检测到嵌套压缩包（第 {depth} 层）: {}，请稍等...",
                    label(&inner)
                ),
            );

            let target = inner
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| output_dir.clone());
            match extract_with_retry(ctx, task, tips, &row, &inner, &target, password.clone()) {
                Ok(found) => {
                    password = found;
                    nested_count += 1;
                    removed += archive::remove_set(&inner);
                }
                Err(error) => {
                    ctx.error(&format!(
                        "ppxfy: 嵌套压缩包解压失败 {}: {error}",
                        inner.display()
                    ));
                }
            }
        }
    }

    // 收尾：先停刷新，再写最终文案，避免 ticker 把汇总覆盖回进度条。
    board.finish();

    // 先修乱码名，再折叠同名目录：折叠是按名字判断的，顺序反了会白做一遍。
    let repaired = if ctx.config.fix_encoding {
        mojibake::repair_tree(&output_dir)
    } else {
        0
    };
    if repaired > 0 {
        ctx.info(&format!("ppxfy: 已修复 {repaired} 个乱码文件名"));
    }

    let folded = fsutil::flatten_same_name_dirs(&output_dir);
    let entries: Vec<String> = fsutil::list_entries(&output_dir)
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    let mut notes = Vec::new();
    if repaired > 0 {
        notes.push(format!("已修复 {repaired} 个乱码文件名"));
    }
    if nested_count > 0 {
        notes.push(format!("另自动解压了 {nested_count} 个嵌套压缩包"));
    }
    if folded > 0 {
        notes.push(format!("已折叠 {folded} 层同名嵌套目录"));
    }
    if removed > 0 {
        notes.push(format!("已移除 {removed} 个压缩包文件以节省空间"));
    }

    let note_text = if notes.is_empty() {
        String::new()
    } else {
        format!("\n{}", notes.join("，"))
    };
    ctx.messenger.edit_text(
        tips.chat_id,
        tips.message_id,
        &format!(
            "✅ 解压完成: {}\n包含 {} 个条目{note_text}",
            output_dir.display(),
            entries.len()
        ),
    );
    ctx.info(&format!(
        "ppxfy: 解压完成 {}，共 {} 个条目",
        output_dir.display(),
        entries.len()
    ));

    Ok(Outcome {
        output_dir,
        entries,
    })
}

/// 解一次压，密码问题按需询问（最多 [`MAX_PASSWORD_ATTEMPTS`] 次）。
///
/// 成功时返回可复用的密码（嵌套压缩包通常与外层一致）。
fn extract_with_retry(
    ctx: &Arc<Ctx>,
    task: &Arc<Task>,
    tips: Sent,
    row: &RowHandle,
    archive: &Path,
    out_dir: &Path,
    mut password: Option<String>,
) -> Result<Option<String>, ExtractError> {
    for attempt in 0..=MAX_PASSWORD_ATTEMPTS {
        // 每次尝试都从 0 开始，避免上一次的残值把进度条顶到 100%。
        row.reset(label(archive));
        match extract_once(ctx, archive, out_dir, password.as_deref(), row) {
            Ok(()) => {
                row.complete();
                return Ok(password);
            }
            Err(ExtractError::Password) => {
                if attempt == MAX_PASSWORD_ATTEMPTS {
                    return Err(ExtractError::Password);
                }
                let Some(input) = ask_password(ctx, task, tips, attempt > 0) else {
                    return Err(ExtractError::Skipped);
                };
                password = Some(input);
            }
            Err(error) => return Err(error),
        }
    }
    Err(ExtractError::Password)
}

/// 单次尝试：HatchZip 优先，格式不支持再交给 7z。
fn extract_once(
    ctx: &Arc<Ctx>,
    archive: &Path,
    out_dir: &Path,
    password: Option<&str>,
    row: &RowHandle,
) -> Result<(), ExtractError> {
    match hatch::extract(archive, out_dir, password, row) {
        Ok(()) => Ok(()),
        Err(ExtractError::Unsupported(reason)) => {
            ctx.info(&format!(
                "ppxfy: HatchZip 不支持 {}（{reason}），改用 7z",
                label(archive)
            ));
            seven::extract(
                &ctx.config.seven_zip,
                archive,
                out_dir,
                password,
                ctx.config.unzip_timeout,
            )
            .map_err(map_seven_error)
        }
        Err(error) => Err(error),
    }
}

/// 7z 错误 -> 解压错误。
fn map_seven_error(error: seven::Error) -> ExtractError {
    match error {
        seven::Error::Password { .. } => ExtractError::Password,
        seven::Error::Timeout => ExtractError::TimedOut,
        seven::Error::Spawn(message) => ExtractError::Tool(message),
        seven::Error::Failed { code, log } => {
            ExtractError::Damaged(format!("7z 退出码 {code}: {log}"))
        }
    }
}

/// 询问密码；回复空或 `SKIP` 视为放弃。
fn ask_password(ctx: &Ctx, task: &Task, tips: Sent, retry: bool) -> Option<String> {
    let prompt = if retry {
        "解压失败：该压缩包需要密码（或密码不正确）。\n请回复密码重试，回复 SKIP 放弃本次解压。"
    } else {
        "该压缩包需要密码。\n请回复密码，回复 SKIP 放弃本次解压。"
    };
    ctx.messenger
        .edit_text(tips.chat_id, tips.message_id, prompt);

    match ctx.wait_reply(
        task,
        tips.chat_id,
        tips.message_id,
        ctx.config.listen_timeout,
    ) {
        Wait::Reply(text) => {
            let text = text.trim();
            if text.is_empty() || text.eq_ignore_ascii_case("SKIP") {
                None
            } else {
                Some(text.to_owned())
            }
        }
        _ => None,
    }
}

/// 取文件名用于提示。
fn label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
