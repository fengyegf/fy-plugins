//! 用户回复路由。
//!
//! 一次交互（链接 / 密码 / 编号 / 包名）都以「某条提示消息的回复」为载体，
//! 所以等待方按 `(chat_id, 提示消息 id)` 登记，worker 线程把收到的回复投递过去。
//! 只有一个进程内的 `HashMap`，不持有任何线程或句柄，等待结束时自动注销。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use serde_json::Value;

/// 一次等待的结局。
#[derive(Debug, Clone)]
pub enum Wait {
    /// 用户回复的原始文本。
    Reply(String),
    /// 超时未回复。
    Timeout,
    /// 被 `/cancel` 中断。
    Cancelled,
    /// 插件正在卸载。
    Shutdown,
}

/// 一条命中等待者的回复。
#[derive(Debug, Clone)]
pub struct ReplyMatch {
    pub chat_id: i64,
    /// 用户那条回复的消息 id（用于删除，保持界面整洁）。
    pub message_id: i64,
}

/// 回复登记表。
#[derive(Default)]
pub struct Inbox {
    waiters: Mutex<HashMap<(i64, i64), Sender<String>>>,
}

impl Inbox {
    /// 登记一个等待：`tips_message_id` 是提示消息的 id，回复它才算数。
    pub fn register(&self, chat_id: i64, tips_message_id: i64) -> Registration<'_> {
        let (sender, receiver) = channel();
        self.waiters
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert((chat_id, tips_message_id), sender);
        Registration {
            inbox: self,
            key: (chat_id, tips_message_id),
            receiver,
        }
    }

    /// 把一条更新分发给等待者；命中时返回回复内容，否则返回 `None`。
    ///
    /// 只接受「文本 + 回复了目标提示消息」的更新，其余一概忽略。
    pub fn deliver(&self, update: &Value) -> Option<ReplyMatch> {
        let message = update.get("message")?;
        if update.get("@type").and_then(Value::as_str) != Some("updateNewMessage") {
            return None;
        }
        let chat_id = message.get("chat_id").and_then(Value::as_i64)?;
        let content = message.get("content")?;
        if content.get("@type").and_then(Value::as_str) != Some("messageText") {
            return None;
        }
        let text = content.get("text")?.get("text")?.as_str()?.to_owned();

        let reply_to = message.get("reply_to")?;
        if reply_to.get("@type").and_then(Value::as_str) != Some("messageReplyToMessage") {
            return None;
        }
        let tips_message_id = reply_to.get("message_id").and_then(Value::as_i64)?;

        let sender = self
            .waiters
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&(chat_id, tips_message_id))
            .cloned()?;
        // 等待方可能刚好超时退出，发送失败不算错误。
        let _ = sender.send(text);

        Some(ReplyMatch {
            chat_id,
            message_id: message.get("id").and_then(Value::as_i64)?,
        })
    }
}

/// 一次登记的守卫：drop 时注销，避免残留的 `Sender` 把回复吃掉。
pub struct Registration<'a> {
    inbox: &'a Inbox,
    key: (i64, i64),
    receiver: Receiver<String>,
}

impl Registration<'_> {
    /// 在给定时间内取一条回复；超时返回 `None`。
    pub fn poll(&self, timeout: Duration) -> Option<String> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.inbox
            .waiters
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.key);
    }
}
