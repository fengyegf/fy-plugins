//! 命令层：一条命令一个文件。
//!
//! 这里只做三件事：把消息翻译成 [`Job`]、按命令名分发、兜住业务线程的 panic。

pub mod parser;
pub mod system;

use std::any::Any;
use std::sync::Arc;

use crate::cmd::parser::Command;
use crate::host::worker::IncomingMessage;
use crate::host::{Ctx, Msg};

/// 一条待执行的任务。
#[derive(Debug, Clone)]
pub enum Job {
    /// `/system`：采集系统信息 → 渲染卡片 → 发回去。
    System { msg: Msg },
}

impl Job {
    /// 任务所属聊天，panic 兜底时用来回消息。
    pub fn chat_id(&self) -> i64 {
        match self {
            Job::System { msg } => msg.chat_id,
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
        "system" => Some(Job::System { msg }),
        _ => None,
    }
}

/// 在业务线程上执行任务，并兜住 panic（panic 不能穿透 cdylib 边界）。
pub fn run(ctx: Arc<Ctx>, job: Job) {
    let chat_id = job.chat_id();
    let guard = ctx.clone();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || match job {
        Job::System { msg } => system::run(&ctx, msg),
    }));

    if let Err(payload) = outcome {
        let reason = panic_message(&payload);
        guard.error(&format!(
            "system: 任务线程 panic（chat={chat_id}）: {reason}"
        ));
        guard.messenger.try_send_text(
            chat_id,
            &format!("❌ 内部错误，获取系统信息失败：{reason}"),
            None,
        );
    }
}

/// 从 panic 载荷里取出可读信息。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "未知 panic".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> IncomingMessage {
        IncomingMessage {
            chat_id: 9,
            id: 3,
            text: text.to_owned(),
        }
    }

    fn parse(text: &str) -> Command {
        parser::parse(text, &["/".to_owned()]).expect("应当识别为命令")
    }

    #[test]
    fn plans_system_command() {
        let job = plan(&message("/system"), &parse("/system")).expect("system 应当有任务");
        match job {
            Job::System { msg } => {
                assert_eq!(msg.chat_id, 9);
                assert_eq!(msg.id, 3);
            }
        }
    }

    #[test]
    fn ignores_unknown_commands() {
        assert!(plan(&message("/whois"), &parse("/whois")).is_none());
    }
}
