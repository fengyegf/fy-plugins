//! 命令层：一个命令一个文件，和「有哪些功能」一一对应。
//!
//! 这里只做三件事：把消息翻译成 [`Job`]、按命令名分发、兜住业务线程的 panic。
//! 具体流程在各自文件里，能力实现见 `crate::core`、`crate::galgame`、`crate::telegraph`
//! 与 `crate::{unpack, package}`。

pub mod cancel;
pub mod clearcache;
pub mod download;
pub mod galgame;
pub mod parser;

use std::any::Any;
use std::sync::Arc;

use crate::cmd::parser::Command;
use crate::host::worker::IncomingMessage;
use crate::host::{Ctx, Msg};

/// 一条待执行的任务。
pub enum Job {
    /// 下载 → 解压 → 目录树 → 打包。
    Download(Msg),
    /// 取消当前聊天的活跃任务。
    Cancel(Msg),
    /// 兜底清理缓存目录（没有活跃任务时也能用）。
    ClearCache(Msg),
    /// DMM / DLSite 作品页 → 核对 → 分享消息。带原文是因为链接要从消息里取。
    Galgame { msg: Msg, text: String },
}

impl Job {
    /// 任务所属聊天，panic 兜底时用来回消息。
    pub fn chat_id(&self) -> i64 {
        match self {
            Job::Download(msg) | Job::Cancel(msg) | Job::ClearCache(msg) => msg.chat_id,
            Job::Galgame { msg, .. } => msg.chat_id,
        }
    }
}

/// 命令名 → 任务。未识别的命令返回 `None`（静默忽略，和其它插件共存）。
pub fn plan(message: &IncomingMessage, command: &Command) -> Option<Job> {
    let msg = Msg {
        chat_id: message.chat_id,
        id: message.id,
    };
    match command.name.to_ascii_lowercase().as_str() {
        // 同时保留短名、常规名和历史长命令名，避免已有调用方式失效。
        "d" | "download" | "downloadcommand" => Some(Job::Download(msg)),
        "cancel" => Some(Job::Cancel(msg)),
        "clearcache" => Some(Job::ClearCache(msg)),
        "p" | "galgame" | "galgamecommand" => Some(Job::Galgame {
            msg,
            text: message.text.clone(),
        }),
        _ => None,
    }
}

/// 在业务线程上执行任务，并兜住 panic。
pub fn run(ctx: Arc<Ctx>, job: Job) {
    let chat_id = job.chat_id();
    let guard = ctx.clone();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || match job {
        Job::Download(msg) => download::run(&ctx, msg),
        Job::Cancel(msg) => cancel::run(&ctx, msg),
        Job::ClearCache(msg) => clearcache::run(&ctx, msg),
        Job::Galgame { msg, text } => galgame::run(&ctx, msg, &text),
    }));

    if let Err(payload) = outcome {
        let reason = panic_message(&payload);
        guard.error(&format!(
            "ppxfy: 任务线程 panic（chat={chat_id}）: {reason}"
        ));
        // 登记必须撤掉，否则这个聊天看起来一直「忙」；
        // 但产物不删——已下载的压缩包往往有几个 GB，留着让用户自己决定怎么处理。
        guard.tasks.force_end(chat_id);
        guard.messenger.try_send_text(
            chat_id,
            &format!("❌ 内部错误，流程已中断：{reason}（临时文件保留在 cache/，可手动清理）"),
            None,
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
