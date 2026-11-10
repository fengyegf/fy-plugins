//! 收件箱与调度。
//!
//! 宿主回调只做一件事：把更新丢进 channel。剩下都在这里：
//! 解析出文本命令，为每条命令开一个线程跑完整流程。

use std::ffi::{c_char, c_void};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, parse_payload};
use serde_json::Value;

use crate::cmd;
use crate::config::Config;
use crate::host::Ctx;

/// 空闲时的轮询间隔（用于观察停机标志）。
const IDLE_TICK: Duration = Duration::from_millis(200);
/// 线程栈大小：流程本身不深，512 KiB 足够（渲染在自己层里再开）。
const TASK_STACK: usize = 512 * 1024;

type Handles = Arc<Mutex<Vec<JoinHandle<()>>>>;

/// 一条命令消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub chat_id: i64,
    pub id: i64,
    pub text: String,
    pub is_outgoing: bool,
}

impl IncomingMessage {
    /// 从 `updateNewMessage` 里取出需要的字段；不是文本消息就返回 `None`。
    fn parse(update: &Value) -> Option<Self> {
        if update.get("@type").and_then(Value::as_str) != Some("updateNewMessage") {
            return None;
        }
        let message = update.get("message")?;
        let content = message.get("content")?;
        if content.get("@type").and_then(Value::as_str) != Some("messageText") {
            return None;
        }
        Some(Self {
            chat_id: message.get("chat_id").and_then(Value::as_i64)?,
            id: message.get("id").and_then(Value::as_i64)?,
            text: content.get("text")?.get("text")?.as_str()?.to_owned(),
            is_outgoing: message
                .get("is_outgoing")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// 插件的工作线程组：一个路由线程 + 若干业务线程。
pub struct Worker {
    ctx: Arc<Ctx>,
    handles: Handles,
    thread: Option<JoinHandle<()>>,
    /// 必须活着：它就是回调拿到的那个指针的目标。
    sender: Box<mpsc::Sender<Value>>,
}

impl Worker {
    /// 建上下文、起路由线程。
    pub fn start(host: HostApi, config: Config) -> Result<Self, String> {
        let ctx = Arc::new(Ctx::new(host, config));
        let (sender, receiver) = mpsc::channel::<Value>();
        let handles: Handles = Arc::new(Mutex::new(Vec::new()));

        let loop_ctx = ctx.clone();
        let loop_handles = handles.clone();
        let thread = std::thread::Builder::new()
            .name("musiccard-worker".to_owned())
            .stack_size(TASK_STACK)
            .spawn(move || route_loop(loop_ctx, loop_handles, receiver))
            .map_err(|error| format!("启动 worker 线程失败: {error}"))?;

        Ok(Self {
            ctx,
            handles,
            thread: Some(thread),
            sender: Box::new(sender),
        })
    }

    /// 传给 `subscribe_json` 的 `user_data`：指向 channel 发送端。
    pub fn sender_ptr(&self) -> *mut c_void {
        (&*self.sender as *const mpsc::Sender<Value>) as *mut c_void
    }

    /// 停机：置位标志 → join 路由线程 → join 业务线程。
    ///
    /// 必须在 `Subscription` 被 drop **之后**调用，否则回调还可能摸到已经释放的 sender。
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
pub extern "C-unwind" fn on_update(user: *mut c_void, payload: *const c_char) -> i32 {
    if user.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: 宿主按 ABI 契约传的是临时 NUL 结尾字符串；上面已排除空指针。
    let update = unsafe { parse_payload(payload) };
    // SAFETY: `user` 是 `Worker::sender_ptr` 给出的、由 `Worker` 持有的发送端。
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

/// 一条更新的去向：只认文本命令，其余静默忽略（和其它插件共存）。
fn dispatch(ctx: &Arc<Ctx>, handles: &Handles, update: Value) {
    let Some(message) = IncomingMessage::parse(&update) else {
        return;
    };
    // 自己发的消息也会回流，别把机器人自己的话当命令。
    if message.is_outgoing {
        return;
    }
    let Some(command) = cmd::parser::parse(&message.text, &ctx.config.prefixes) else {
        return;
    };
    let Some(job) = cmd::plan(&message, &command) else {
        return;
    };

    let job_ctx = ctx.clone();
    spawn_task(ctx, handles, move || cmd::run(job_ctx, job));
}

/// 起一个业务线程，顺手回收已结束的句柄。
fn spawn_task(ctx: &Ctx, handles: &Handles, task: impl FnOnce() + Send + 'static) {
    let mut list = handles.lock().unwrap_or_else(|error| error.into_inner());
    list.retain(|handle| !handle.is_finished());
    match std::thread::Builder::new()
        .name("musiccard-task".to_owned())
        .stack_size(TASK_STACK)
        .spawn(task)
    {
        Ok(handle) => list.push(handle),
        Err(error) => ctx.error(&format!("musiccard: 启动业务线程失败: {error}")),
    }
}
