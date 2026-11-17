//! 收件箱与调度。
//!
//! 宿主回调只做一件事：把更新丢进 channel。剩下都在这里：解析成 [IncomingMessage]，
//! 过滤掉不该管的（群聊 / 自己发的），再为每条消息开一个短命线程跑完整中转流程。
//!
//! **回调里绝不阻塞**：LLM / 翻译 / 发消息都是同步阻塞的，只能放在业务线程。

use std::ffi::{c_char, c_void};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt, parse_payload};
use serde_json::Value;

use crate::core::Config;
use crate::host::Ctx;

/// 空闲轮询间隔（用来观察停机标志）。
const IDLE_TICK: Duration = Duration::from_millis(200);
/// 业务线程栈：HTTP + 翻译都在这里跑，给足 1 MiB。
const TASK_STACK: usize = 1024 * 1024;

/// 一条消息里我们关心的字段（私聊与"我们的论坛群"共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub chat_id: i64,
    pub id: i64,
    /// 发送者 user id；频道 / 匿名发送者没有，取不到就是 None。
    pub sender_user_id: Option<i64>,
    /// 1 对 1 私聊（TDLib 里私聊 id 恒为正）。
    pub is_private: bool,
    /// 自己发的（机器人自己的消息、或用户号自己发的）。
    pub is_outgoing: bool,
    /// 消息正文：文本消息取文本，媒体取 caption，其余为空。
    pub text: String,
    /// 内容类型标签：text / photo / video / document / voice / sticker / …
    pub kind: String,
    /// 引用的那条消息 id（主人引用某条消息回复时用）。
    pub reply_to_message_id: Option<i64>,
    /// 消息所在话题的 forum_topic_id（普通群 / 私聊为 None）。
    pub topic_thread_id: Option<i64>,
    /// 群里的发送者 chat（匿名管理员），普通用户为 None。
    pub sender_chat_id: Option<i64>,
    /// TDLib 消息时间戳（unix 秒）。
    pub date: u64,
}

impl IncomingMessage {
    /// 从 updateNewMessage 里取字段。非 updateNewMessage 一律 None。
    pub fn parse(update: &Value) -> Option<Self> {
        if update.get("@type").and_then(Value::as_str) != Some("updateNewMessage") {
            return None;
        }
        let message = update.get("message")?;
        let chat_id = message.get("chat_id").and_then(Value::as_i64)?;
        let id = message.get("id").and_then(Value::as_i64)?;
        let sender_user_id = sender_of(message);
        let content = message.get("content")?;
        let kind = content
            .get("@type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let text = message_text(content, &kind);
        Some(Self {
            chat_id,
            id,
            sender_user_id,
            is_private: chat_id > 0,
            is_outgoing: message
                .get("is_outgoing")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            text,
            kind,
            reply_to_message_id: reply_to_of(message),
            topic_thread_id: topic_of(message),
            sender_chat_id: sender_chat_of(message),
            date: message.get("date").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// 给主人看的内容：文本原样，媒体退回一个方括号占位。
    pub fn display_text(&self) -> String {
        if !self.text.trim().is_empty() {
            return self.text.clone();
        }
        format!("[{}]", kind_label(&self.kind))
    }
}

/// 发送者 user id：只认 messageSenderUser（频道发送者没有 user id）。
fn sender_of(message: &Value) -> Option<i64> {
    let sender = message.get("sender_id")?;
    if sender.get("@type").and_then(Value::as_str) != Some("messageSenderUser") {
        return None;
    }
    sender.get("user_id").and_then(Value::as_i64)
}

/// 话题：只认论坛话题（messageTopicForum）；General 话题没有 topic_id。
fn topic_of(message: &Value) -> Option<i64> {
    let topic = message.get("topic_id")?;
    if topic.get("@type").and_then(Value::as_str) != Some("messageTopicForum") {
        return None;
    }
    topic.get("forum_topic_id").and_then(Value::as_i64)
}

/// 群里的发送者 chat（匿名管理员）；普通用户消息返回 None。
fn sender_chat_of(message: &Value) -> Option<i64> {
    let sender = message.get("sender_id")?;
    if sender.get("@type").and_then(Value::as_str) != Some("messageSenderChat") {
        return None;
    }
    sender.get("chat_id").and_then(Value::as_i64)
}

/// 引用关系：只认 messageReplyToMessage（故事 / 外部回复不参与中转映射）。
fn reply_to_of(message: &Value) -> Option<i64> {
    let reply = message.get("reply_to")?;
    if reply.get("@type").and_then(Value::as_str) != Some("messageReplyToMessage") {
        return None;
    }
    reply.get("message_id").and_then(Value::as_i64)
}

/// 正文：文本消息取 formattedText.text，媒体取 caption.text。
fn message_text(content: &Value, kind: &str) -> String {
    let field = match kind {
        "messageText" => "text",
        _ => "caption",
    };
    content
        .get(field)
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 内容类型的中文标签，用于"[图片]"这种占位。
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "messageText" => "文本",
        "messagePhoto" => "图片",
        "messageVideo" => "视频",
        "messageVideoNote" => "圆形视频",
        "messageVoiceNote" => "语音",
        "messageAudio" => "音频",
        "messageDocument" => "文件",
        "messageSticker" => "贴纸",
        "messageAnimation" => "动图",
        "messageLocation" => "位置",
        "messageVenue" => "地点",
        "messageContact" => "联系人",
        "messagePoll" => "投票",
        "messageDice" => "骰子",
        "messageInvoice" => "账单",
        "messageCall" => "通话",
        _ => "消息",
    }
}

/// 插件的工作线程组：一个路由线程 + 若干短命业务线程。
pub struct Worker {
    ctx: Arc<Ctx>,
    thread: Option<JoinHandle<()>>,
    /// 必须活着：它就是回调拿到的那个指针的目标。
    sender: Box<mpsc::Sender<Value>>,
    handles: Handles,
}

/// 业务线程句柄表：路由线程往里塞，停机时统一 join。
type Handles = Arc<Mutex<Vec<JoinHandle<()>>>>;

impl Worker {
    /// 建上下文、起路由线程。
    pub fn start(host: HostApi, config: Config) -> Result<Self, String> {
        let ctx = Ctx::new(host, config)?;
        let (sender, receiver) = mpsc::channel::<Value>();
        let handles: Handles = Arc::new(Mutex::new(Vec::new()));

        let loop_ctx = ctx.clone();
        let loop_handles = handles.clone();
        let thread = std::thread::Builder::new()
            .name("bidibot-worker".to_owned())
            .stack_size(TASK_STACK)
            .spawn(move || route_loop(loop_ctx, loop_handles, receiver))
            .map_err(|error| format!("启动 worker 线程失败: {error}"))?;

        Ok(Self {
            ctx,
            thread: Some(thread),
            sender: Box::new(sender),
            handles,
        })
    }

    /// 传给 subscribe_json 的 user_data：指向 channel 发送端。
    pub fn sender_ptr(&self) -> *mut c_void {
        (&*self.sender as *const mpsc::Sender<Value>) as *mut c_void
    }

    /// 上下文（start 之后命令表登记等场景会用到）。
    pub fn ctx(&self) -> &Arc<Ctx> {
        &self.ctx
    }

    /// 停机：置位标志 → join 路由线程 → join 业务线程。
    fn shutdown(&mut self) {
        self.ctx.shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let handles = std::mem::take(&mut *self.handles.lock().unwrap_or_else(|e| e.into_inner()));
        for handle in handles {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 宿主回调：只投递，不做任何阻塞操作。
///
/// 必须保持"安全 fn"签名：ABI 里的 EventCallbackFn 就是
/// extern "C-unwind" fn(*mut c_void, *const c_char) -> i32。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C-unwind" fn on_update(user: *mut c_void, payload: *const c_char) -> i32 {
    // SAFETY: 宿主按 ABI 契约传的是临时 NUL 结尾字符串。
    let update = unsafe { parse_payload(payload) };
    if user.is_null() {
        return -1;
    }
    // SAFETY: user 是 Worker::sender_ptr 给出的、由 Worker 持有的发送端。
    let sender = unsafe { &*(user as *const mpsc::Sender<Value>) };
    if sender.send(update).is_err() {
        return -1;
    }
    0
}

/// 路由线程主循环。
fn route_loop(ctx: Arc<Ctx>, handles: Handles, receiver: Receiver<Value>) {
    while !ctx.is_shutdown() {
        match receiver.recv_timeout(IDLE_TICK) {
            Ok(update) => dispatch(&ctx, &handles, update),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// 把一条更新翻译成"该不该管"，该管的丢给业务线程。
fn dispatch(ctx: &Arc<Ctx>, handles: &Handles, update: Value) {
    let Some(message) = IncomingMessage::parse(&update) else {
        return;
    };
    // 自己发的不回环。
    if message.is_outgoing {
        return;
    }
    // 只处理「私聊」和「我们自己的论坛群」：别的群 / 频道在这里就丢掉，
    // 省得每条无关消息都开一个业务线程。
    let config = ctx.config();
    let relevant =
        message.is_private || (config.group_id != 0 && message.chat_id == config.group_id);
    if !relevant {
        return;
    }
    // 匿名管理员（没有 user id）无法归属，直接忽略。
    if message.sender_user_id.is_none() {
        return;
    }

    let task_ctx = ctx.clone();
    let mut list = handles.lock().unwrap_or_else(|error| error.into_inner());
    // 顺手回收已结束的句柄，长跑下不至于越攒越多。
    list.retain(|handle| !handle.is_finished());
    match std::thread::Builder::new()
        .name("bidibot-task".to_owned())
        .stack_size(TASK_STACK)
        .spawn(move || {
            crate::relay::handle(&task_ctx, &message);
            task_ctx.prune_rate();
        }) {
        Ok(handle) => list.push(handle),
        Err(error) => ctx
            .host()
            .error(&format!("bidibot: 启动任务线程失败: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(content: Value, outgoing: bool, chat_id: i64) -> Value {
        json!({
            "@type": "updateNewMessage",
            "message": {
                "id": 11,
                "chat_id": chat_id,
                "is_outgoing": outgoing,
                "date": 1700000000,
                "sender_id": { "@type": "messageSenderUser", "user_id": 777 },
                "content": content,
            }
        })
    }

    #[test]
    fn parses_private_text_message() {
        let content = json!({
            "@type": "messageText",
            "text": { "@type": "formattedText", "text": "你好", "entities": [] },
        });
        let msg = IncomingMessage::parse(&update(content, false, 777)).unwrap();
        assert_eq!(msg.chat_id, 777);
        assert_eq!(msg.id, 11);
        assert_eq!(msg.sender_user_id, Some(777));
        assert!(msg.is_private);
        assert!(!msg.is_outgoing);
        assert_eq!(msg.text, "你好");
        assert_eq!(msg.kind, "messageText");
        assert_eq!(msg.reply_to_message_id, None);
    }

    #[test]
    fn parses_caption_and_reply() {
        let content = json!({
            "@type": "messagePhoto",
            "caption": { "@type": "formattedText", "text": "看图", "entities": [] },
        });
        let mut value = update(content, false, 777);
        value["message"]["reply_to"] = json!({
            "@type": "messageReplyToMessage",
            "message_id": 90,
            "chat_id": 777,
        });
        let msg = IncomingMessage::parse(&value).unwrap();
        assert_eq!(msg.kind, "messagePhoto");
        assert_eq!(msg.text, "看图");
        assert_eq!(msg.reply_to_message_id, Some(90));
    }

    #[test]
    fn captures_become_none_and_placeholder_shows_kind() {
        let content = json!({ "@type": "messagePhoto", "caption": { "@type": "formattedText", "text": "", "entities": [] } });
        let msg = IncomingMessage::parse(&update(content, false, 777)).unwrap();
        assert_eq!(msg.chat_id, 777);
        assert_eq!(msg.display_text(), "[图片]");
    }

    #[test]
    fn non_message_sender_is_ignored() {
        let content = json!({ "@type": "messageText", "text": { "@type": "formattedText", "text": "x", "entities": [] } });
        let mut value = update(content, false, -100);
        value["message"]["sender_id"] = json!({ "@type": "messageSenderChat", "chat_id": -100 });
        let msg = IncomingMessage::parse(&value).unwrap();
        assert_eq!(msg.sender_user_id, None);
        assert!(!msg.is_private);
    }
}
