//! 命令层。
//!
//! 这里只做三件事：把消息翻译成 [`Job`]、按命令名分发、兜住业务线程的 panic。
//! 具体流程在 [`musiccard`] 里，能力实现见 [`crate::netease`] 与 [`crate::render`]。

pub mod musiccard;
pub mod parser;

use std::any::Any;
use std::sync::Arc;

use crate::cmd::parser::Command;
use crate::host::worker::IncomingMessage;
use crate::host::{Ctx, Msg};

/// 命令名：大小写不敏感，只有它算本插件的命令。
const COMMAND_NAME: &str = "musiccard";

/// 一条待执行的任务。
pub enum Job {
    /// 取歌 → 取封面 → 选歌词 → 渲染 → 发照片。
    MusicCard {
        msg: Msg,
        /// 参数按空格拼回一整串（关键词可以带空格）。
        query: String,
    },
}

impl Job {
    /// 触发这条命令的消息；`Msg` 是 `Copy`，可以先取出来再 move `job` 进闭包。
    pub fn msg(&self) -> Msg {
        match self {
            Job::MusicCard { msg, .. } => *msg,
        }
    }
}

/// 命令名 → 任务。未识别的命令返回 `None`（静默忽略，和其它插件共存）。
pub fn plan(message: &IncomingMessage, command: &Command) -> Option<Job> {
    if !command.name.eq_ignore_ascii_case(COMMAND_NAME) {
        return None;
    }
    Some(Job::MusicCard {
        msg: Msg {
            chat_id: message.chat_id,
            id: message.id,
        },
        // 参数用单个空格重新连接，并去掉首尾空白。
        query: command.args.join(" ").trim().to_owned(),
    })
}

/// 在业务线程上执行任务，并兜住 panic。
pub fn run(ctx: Arc<Ctx>, job: Job) {
    // `Msg` 是 `Copy`：先取出来，panic 兜底时才能「回复原消息」而不是发一条裸消息。
    let msg = job.msg();
    let guard = ctx.clone();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || match job {
        Job::MusicCard { msg, query } => musiccard::run(&ctx, msg, &query),
    }));

    if let Err(payload) = outcome {
        let reason = panic_message(&payload);
        guard.error(&format!(
            "musiccard: 任务线程 panic（chat={}, msg={}）: {reason}",
            msg.chat_id, msg.id
        ));
        // 进度消息与临时文件由 `musiccard::run` 里的 Drop 守卫在 unwind 时清掉；
        // 这里只需要把失败原因回复到原消息上。
        guard.messenger.try_send_text(
            msg.chat_id,
            &format!("生成音乐卡片失败：内部错误（{reason}）"),
            Some(msg.id),
        );
    }
}

/// 从 panic 载荷里取出可读信息。
fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "未知 panic".to_owned()
    }
}
