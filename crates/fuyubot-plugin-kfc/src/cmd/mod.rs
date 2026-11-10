//! 命令层：一条命令一个文件。
//!
//! 这里只做三件事：把消息翻译成 [`Job`]、按命令名分发、兜住业务线程的 panic。

pub mod kfc;
pub mod parser;

use std::any::Any;
use std::sync::Arc;

use crate::cmd::parser::Command;
use crate::host::worker::IncomingMessage;
use crate::host::{Ctx, Msg};

/// 一条待执行的任务。
#[derive(Debug, Clone, Copy)]
pub enum Job {
    /// `/kfc`：抓文案 → 渲染卡片 → 发回去。
    Kfc(Msg),
}

impl Job {
    /// 任务所属聊天，panic 兜底时用来回消息。
    pub fn chat_id(&self) -> i64 {
        match self {
            Job::Kfc(msg) => msg.chat_id,
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
        "kfc" => Some(Job::Kfc(msg)),
        _ => None,
    }
}

/// 在业务线程上执行任务，并兜住 panic（panic 不能穿透 cdylib 边界）。
pub fn run(ctx: Arc<Ctx>, job: Job) {
    let chat_id = job.chat_id();
    let guard = ctx.clone();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || match job {
        Job::Kfc(msg) => kfc::run(&ctx, msg),
    }));

    if let Err(payload) = outcome {
        let reason = panic_message(&payload);
        guard.error(&format!("kfc: 任务线程 panic（chat={chat_id}）: {reason}"));
        guard
            .messenger
            .try_send_text(chat_id, &format!("❌ 内部错误，生成失败：{reason}"), None);
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
