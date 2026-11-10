//! `/clearcache`：兜底清理缓存目录。
//!
//! 系统重启、任务丢失、异常中断都可能留下半截文件；这时没有活跃任务可取消，
//! 只能手动清。清理逻辑与 `/cancel` 的清理钩子共用
//! [`clear_download_artifacts`](crate::util::fs::clear_download_artifacts)：
//! 删 `Output/`、`ZipOutput/`（连同目录本身）与 `cache/` 根目录下的普通文件，
//! 下一次下载 / 打包流程会重新创建这两个子目录。

use crate::host::{Ctx, Msg};
use crate::util::fs::{clear_download_artifacts, list_entries};

/// 处理 `/clearcache`。
pub fn run(ctx: &Ctx, msg: Msg) {
    let cache = &ctx.config.cache_dir;
    if !cache.is_dir() {
        ctx.messenger
            .try_send_text(msg.chat_id, "cache 目录不存在，无需清理。", Some(msg.id));
        return;
    }

    let report = clear_download_artifacts(cache);
    // 单个失败不改变「清理完成」的结论，但要留在日志里方便排查占用。
    for (path, error) in &report.failed {
        ctx.warn(&format!("ppxfy: 清理失败 {}: {error}", path.display()));
    }

    // 清理后再数一次目录项，告诉用户还有没有残留。
    let remaining = list_entries(cache).len();
    ctx.info(&format!(
        "ppxfy: /clearcache 完成，移除 {} 项，剩余 {remaining} 项",
        report.removed.len()
    ));

    let text = if remaining > 0 {
        format!("🗑️ 缓存清理完成，cache 下仍有 {remaining} 项未删除。")
    } else {
        "🗑️ 缓存清理完成，cache 已清空。".to_owned()
    };
    ctx.messenger
        .try_send_text(msg.chat_id, &text, Some(msg.id));
}
