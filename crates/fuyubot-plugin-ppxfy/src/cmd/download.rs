//! 下载流程编排：收集链接 → 并发下载 → 定位压缩包 → 解压 → 目录树交互 → 打包。
//!
//! 并发下载共用**一条**进度消息（多行进度条），见 [`ProgressBoard`]。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::{archive, fetch, tree};
use crate::host::progress::ProgressBoard;
use crate::host::reply::Wait;
use crate::host::task::Task;
use crate::host::{Ctx, Msg, map_concurrent};
use crate::{package, unpack};

const LINK_TIPS: &str = "请回复此消息，并提供下载链接\n- 直接回复下载链接\n- 支持多个链接，逐条回复即可\n- 回复 'OK' 结束输入";

/// 下载进度刷新间隔：比解压宽松，少给 TDLib 添麻烦。
const DOWNLOAD_INTERVAL: Duration = Duration::from_secs(5);

/// 下载命令入口。出错只影响本次流程，不向外传播。
pub fn run(ctx: &Arc<Ctx>, msg: Msg) {
    let cleanup_ctx = ctx.clone();
    let task = ctx.tasks.register(
        msg.chat_id,
        Box::new(move || {
            let report = crate::util::fs::clear_download_artifacts(&cleanup_ctx.config.cache_dir);
            cleanup_ctx.info(&format!(
                "ppxfy: 取消清理完成，共移除 {} 项",
                report.removed.len()
            ));
        }),
    );

    if let Err(error) = flow(ctx, &task, msg) {
        ctx.error(&format!("ppxfy: 下载流程失败: {error}"));
        ctx.messenger
            .try_send_text(msg.chat_id, &format!("❌ 流程中断: {error}"), Some(msg.id));
    }
    ctx.tasks.end(&task);
}

/// 主流程。返回 `Err` 只用于「需要告知用户的异常」，用户主动取消不算错误。
fn flow(ctx: &Arc<Ctx>, task: &Arc<Task>, msg: Msg) -> Result<(), String> {
    let links = listen_links(ctx, task, msg)?;
    if links.is_empty() {
        return Ok(());
    }

    let paths = download_all(ctx, task, msg, links)?;
    if paths.is_empty() {
        ctx.warn("ppxfy: 没有任何文件下载成功");
        return Ok(());
    }

    let Some(archive_path) = archive::pick(&paths) else {
        ctx.messenger.try_send_text(
            msg.chat_id,
            "未在已下载文件中找到压缩包（.zip/.rar/.7z/.001/.part1.rar 等）",
            None,
        );
        return Ok(());
    };

    let size = std::fs::metadata(&archive_path)
        .map(|meta| meta.len())
        .unwrap_or_default();
    let size_mb = size as f64 / 1024.0 / 1024.0;
    ctx.info(&format!(
        "ppxfy: 压缩包 {} 大小 {size_mb:.2} MB（阈值 {:.2} MB）",
        archive_path.display(),
        ctx.config.archive_size_threshold as f64 / 1024.0 / 1024.0
    ));

    let name = file_label(&archive_path);
    let tips = ctx
        .messenger
        .send_text(
            msg.chat_id,
            &format!("进入解压流程（{size_mb:.2} MB）：{name}"),
            Some(msg.id),
        )
        .map_err(|error| format!("发送解压提示失败: {error}"))?;
    task.set_tips(tips.message_id);

    let outcome = match unpack::run(ctx, task, tips, &archive_path) {
        Ok(outcome) => outcome,
        // 密码没凑齐：保留缓存，用户可能稍后带着密码重试。
        Err(unpack::ExtractError::Password) => {
            ctx.messenger.edit_text(
                tips.chat_id,
                tips.message_id,
                "❌ 解压失败: 缺少密码或密码不正确（产物已保留）",
            );
            return Ok(());
        }
        Err(unpack::ExtractError::Skipped) => {
            ctx.messenger
                .edit_text(tips.chat_id, tips.message_id, "已放弃解压。");
            return Ok(());
        }
        Err(error) => {
            crate::util::fs::clear_download_artifacts(&ctx.config.cache_dir);
            ctx.messenger.edit_text(
                tips.chat_id,
                tips.message_id,
                &format!("❌ 解压失败: {error}"),
            );
            return Ok(());
        }
    };

    ctx.info(&format!(
        "ppxfy: 解压完成 {}，共 {} 个条目",
        outcome.output_dir.display(),
        outcome.entries.len()
    ));

    // 解压完成后展示目录树：允许用户删掉不要的东西，或选一个目录打包上传。
    if let Some(target) = tree::run(ctx, task, tips, &outcome.output_dir)
        && let Err(error) = package::run(ctx, task, tips, &target)
    {
        ctx.error(&format!("ppxfy: 打包流程失败: {error}"));
        ctx.messenger.edit_text(
            tips.chat_id,
            tips.message_id,
            &format!("❌ 打包失败: {error}"),
        );
    }
    Ok(())
}

/// 并发下载全部链接，共用一条进度消息（每个链接一行进度条）。
///
/// 返回成功落盘的文件路径（顺序与链接顺序一致）。
fn download_all(
    ctx: &Arc<Ctx>,
    task: &Arc<Task>,
    msg: Msg,
    links: Vec<String>,
) -> Result<Vec<PathBuf>, String> {
    let total = links.len();
    let board_message = ctx
        .messenger
        .send_text(
            msg.chat_id,
            &format!("开始下载 {total} 个文件，请稍等..."),
            Some(msg.id),
        )
        .map_err(|error| format!("发送下载提示失败: {error}"))?;

    let board = ProgressBoard::start(
        ctx.messenger.clone(),
        board_message,
        "下载中",
        DOWNLOAD_INTERVAL,
    );
    let jobs: Vec<(String, _)> = links
        .into_iter()
        .map(|url| {
            let row = board.row(fetch::guess_name(&url));
            (url, row)
        })
        .collect();

    let results = map_concurrent(jobs, ctx.config.download_concurrency, |(url, row)| {
        fetch::download(ctx, task, &url, &row)
    });

    // 先停刷新再写汇总，否则 ticker 可能把汇总覆盖回进度条。
    board.finish();

    let paths: Vec<PathBuf> = results.into_iter().flatten().collect();
    let got = paths.len();
    let summary = if got == total {
        format!("✅ 下载完成：{got} 个文件")
    } else if got == 0 {
        format!("❌ 全部下载失败：{total} 个链接")
    } else {
        format!("⚠️ 下载完成 {got}/{total} 个文件（失败项已标记 ❌）")
    };
    ctx.messenger
        .edit_text(board_message.chat_id, board_message.message_id, &summary);
    Ok(paths)
}

/// 监听用户回复，收集下载链接。
fn listen_links(ctx: &Ctx, task: &Task, msg: Msg) -> Result<Vec<String>, String> {
    let tips = ctx
        .messenger
        .send_text(msg.chat_id, LINK_TIPS, Some(msg.id))
        .map_err(|error| format!("发送提示消息失败: {error}"))?;
    task.set_tips(tips.message_id);
    ctx.info("ppxfy: 开始监听下载链接");

    let deadline = Instant::now() + ctx.config.listen_timeout;
    let mut links: Vec<String> = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            ctx.warn("ppxfy: 监听下载链接超时");
            ctx.messenger.edit_text(
                tips.chat_id,
                tips.message_id,
                &format!("监听超时，已收集 {} 个链接", links.len()),
            );
            break;
        }

        match ctx.wait_reply(task, msg.chat_id, tips.message_id, remaining) {
            Wait::Reply(text) => {
                let text = text.trim();
                if text.eq_ignore_ascii_case("OK") {
                    ctx.info("ppxfy: 用户结束输入");
                    ctx.messenger.edit_text(
                        tips.chat_id,
                        tips.message_id,
                        &format!("输入结束，共收集 {} 个链接", links.len()),
                    );
                    break;
                }
                if !text.starts_with("http://") && !text.starts_with("https://") {
                    ctx.messenger.edit_text(
                        tips.chat_id,
                        tips.message_id,
                        "请提供有效的下载链接（以 http:// 或 https:// 开头）",
                    );
                    continue;
                }
                links.push(text.to_owned());
                ctx.info(&format!("ppxfy: 收集到链接 {}", text));
                ctx.messenger.edit_text(
                    tips.chat_id,
                    tips.message_id,
                    &format!("已收集 {} 个链接，继续回复或回复 'OK' 结束", links.len()),
                );
            }
            Wait::Timeout => {
                ctx.warn("ppxfy: 监听下载链接超时");
                ctx.messenger.edit_text(
                    tips.chat_id,
                    tips.message_id,
                    &format!("监听超时，已收集 {} 个链接", links.len()),
                );
                break;
            }
            Wait::Cancelled => {
                ctx.warn("ppxfy: 监听下载链接被 /cancel 中断");
                ctx.messenger.edit_text(
                    tips.chat_id,
                    tips.message_id,
                    &format!("🛑 已取消，已收集 {} 个链接", links.len()),
                );
                break;
            }
            Wait::Shutdown => break,
        }
    }

    if links.is_empty() && !task.is_cancelled() {
        ctx.messenger
            .edit_text(tips.chat_id, tips.message_id, "未收到任何有效链接");
    }
    Ok(links)
}

/// 取路径的文件名部分，拿不到就用完整路径。
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
