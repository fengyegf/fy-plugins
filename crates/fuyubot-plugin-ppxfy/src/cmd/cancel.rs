//! `/cancel`：取消当前聊天的活跃任务。
//!
//! 取消 = 置位标志 + 执行任务登记时挂的清理钩子。所有等待点（链接、密码、编号、包名）
//! 都会在时间片里观察这个标志，因此取消是即时的。

use crate::host::{Ctx, Msg};

/// 处理 `/cancel`。
pub fn run(ctx: &Ctx, msg: Msg) {
    match ctx.tasks.cancel(msg.chat_id) {
        Some(task) => {
            ctx.info(&format!(
                "ppxfy: 取消任务 chat={} tips={}",
                task.chat_id(),
                task.tips()
            ));
            ctx.messenger.try_send_text(
                msg.chat_id,
                "🛑 已发送取消信号，当前任务的等待步骤将结束，相关临时文件已清理。",
                Some(msg.id),
            );
        }
        None => {
            ctx.messenger
                .try_send_text(msg.chat_id, "当前没有活跃任务可取消。", Some(msg.id));
        }
    }
}
