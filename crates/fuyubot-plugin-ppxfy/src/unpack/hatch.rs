//! HatchZip 适配层：**就地解压**（解压多少就释放多少压缩包空间）。
//!
//! 与「解开再删」的区别：磁盘峰值只需要输出大小，也不再需要解压完再清一遍压缩包。
//! 代价是压缩包在解压过程中就没了，所以密码必须在动任何字节之前验证——
//! `Job::check_password` 正是干这个的，失败时压缩包保持完整，可以安全重试。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use hatchzip_core::archive::storage::Reads;
use hatchzip_core::archive::{Damaged, PasswordNeeded, WrongPassword};
use hatchzip_core::engine::{self, Outcome, Progress as EngineProgress, Run};
use hatchzip_core::replace::Choices;
use hatchzip_core::session::{self, State};

use super::ExtractError;
use crate::host::progress::RowHandle;

/// 就地解压 `archive` 到 `out_dir`。
///
/// 出错时通过 [`ExtractError::Unsupported`] 告诉上层「这不是密码问题，换 7z 试试」。
pub fn extract(
    archive: &Path,
    out_dir: &Path,
    password: Option<&str>,
    progress: &RowHandle,
) -> Result<(), ExtractError> {
    let (primary, paths) = session::locate(archive).map_err(classify)?;
    let reads = Arc::new(Reads::default());
    let state = session::open(
        &primary,
        &paths,
        Some(out_dir.to_path_buf()),
        true,
        password,
        &reads,
    )
    .map_err(classify)?;

    let (job, journal) = match state {
        State::Fresh(job, output) => {
            // 先验证密码，再动盘：错密码不会毁掉压缩包。
            job.check_password().map_err(classify)?;
            let existing = session::existing(&job, &output).map_err(classify)?;
            // 输出目录里已有同名文件时允许覆盖。
            let replace = (!existing.is_empty()).then(Choices::default);
            let journal =
                session::begin(&job, &paths, output, false, replace.as_ref()).map_err(classify)?;
            (job, journal)
        }
        // 上次中断留下的进度：接着解。
        State::Resume(job, journal) => (job, journal),
        // 压缩包已经删干净，只剩收尾工作。
        State::Orphan(journal) => {
            engine::finish_orphaned(&journal, &paths).map_err(classify)?;
            return Ok(());
        }
    };

    progress.set_total(journal.total_size);
    let sink = progress.clone();
    let stop = AtomicBool::new(false);
    let run = Run {
        job,
        journal,
        paths,
        stop: &stop,
        progress: EngineProgress {
            on_bytes: Box::new(move |bytes| sink.add(bytes)),
            on_commit: Box::new(|_| {}),
        },
        keep_free: 0,
    };

    match run.execute().map_err(classify)? {
        Outcome::Finished => Ok(()),
        Outcome::Paused => Err(ExtractError::Damaged("解压被中断".to_owned())),
    }
}

/// 把 HatchZip 的 `anyhow::Error` 归类成本插件自己的错误。
fn classify(error: anyhow::Error) -> ExtractError {
    if error.is::<PasswordNeeded>() || error.is::<WrongPassword>() {
        return ExtractError::Password;
    }
    if error.is::<Damaged>() {
        return ExtractError::Damaged(format!("{error:#}"));
    }
    let text = format!("{error:#}");
    let lower = text.to_lowercase();
    // 这些是「格式不支持」，不是「压缩包坏了」——应该退回 7z 而不是判死刑。
    if lower.contains("rar support")
        || lower.contains("not supported yet")
        || lower.contains("unknown archive format")
    {
        return ExtractError::Unsupported(text);
    }
    ExtractError::Damaged(text)
}
