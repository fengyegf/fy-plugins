//! 收件箱与调度。
//!
//! 宿主回调只做一件事：把更新丢进 channel。剩下都在这里：
//! 解析成 [`IncomingMessage`]、按前缀解析命令、为每条命令开一个短命线程跑完整流程。

use std::ffi::{c_char, c_void};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, parse_payload};
use serde_json::Value;

use crate::cmd;
use crate::core::Config;
use crate::host::Ctx;

/// 空闲时的轮询间隔（用于观察停机标志）。
const IDLE_TICK: Duration = Duration::from_millis(200);
/// 业务线程栈：HTTP + 渲染都在这里跑，给足 1 MiB。
const TASK_STACK: usize = 1024 * 1024;

/// 一条命令消息。
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub chat_id: i64,
    pub id: i64,
    pub text: String,
}

impl IncomingMessage {
    /// 从 `updateNewMessage` 里取出需要的字段；不是文本消息就返回 `None`。
    ///
    /// 不按 `is_outgoing` 过滤：fuyubot 的 general 插件常挂在用户号上，
    /// 用户自己发的 `/kfc` 也是 `is_outgoing = true`（TS 版同样不过滤）。
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
        })
    }
}

/// 插件的工作线程组：一个路由线程 + 若干短命业务线程。
pub struct Worker {
    ctx: Arc<Ctx>,
    thread: Option<JoinHandle<()>>,
    /// 必须活着：它就是回调拿到的那个指针的目标。
    sender: Box<mpsc::Sender<Value>>,
    /// 已起的业务线程。插件停机时要逐个 join，否则动态库卸载后还有代码在跑。
    handles: Handles,
}

/// 业务线程句柄表：路由线程往里面塞，停机时统一 join。
type Handles = Arc<Mutex<Vec<JoinHandle<()>>>>;

impl Worker {
    /// 建上下文、起路由线程。
    pub fn start(host: HostApi, config: Config) -> Result<Self, String> {
        let ctx = Arc::new(Ctx::new(host, config));
        let (sender, receiver) = mpsc::channel::<Value>();
        let handles: Handles = Arc::new(Mutex::new(Vec::new()));

        let loop_ctx = ctx.clone();
        let loop_handles = handles.clone();
        let thread = std::thread::Builder::new()
            .name("kfc-worker".to_owned())
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

    /// 传给 `subscribe_json` 的 `user_data`：指向 channel 发送端。
    pub fn sender_ptr(&self) -> *mut c_void {
        (&*self.sender as *const mpsc::Sender<Value>) as *mut c_void
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
/// 必须保持「安全 fn」签名：ABI 里的 `EventCallbackFn` 就是
/// `extern "C-unwind" fn(*mut c_void, *const c_char) -> i32`，改成 `unsafe fn`
/// 会与宿主侧的函数指针类型不兼容。指针合法性由 ABI 契约保证，见函数体注释。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C-unwind" fn on_update(user: *mut c_void, payload: *const c_char) -> i32 {
    // SAFETY: 宿主按 ABI 契约传的是临时 NUL 结尾字符串。
    let update = unsafe { parse_payload(payload) };
    if user.is_null() {
        return -1;
    }
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

/// 把一条更新翻译成命令并执行；不认识的更新静默忽略（和其它插件共存）。
fn dispatch(ctx: &Arc<Ctx>, handles: &Handles, update: Value) {
    let Some(message) = IncomingMessage::parse(&update) else {
        return;
    };
    let Some(command) = cmd::parser::parse(&message.text, &ctx.config.prefixes) else {
        return;
    };
    let Some(job) = cmd::plan(&message, &command) else {
        return;
    };

    let task_ctx = ctx.clone();
    // 每个命令一个短命线程：HTTP 与渲染都是阻塞的，不能占住路由线程。
    let mut list = handles.lock().unwrap_or_else(|error| error.into_inner());
    // 顺手回收已结束的句柄，长跑下不至于越攒越多。
    list.retain(|handle| !handle.is_finished());
    match std::thread::Builder::new()
        .name("kfc-task".to_owned())
        .stack_size(TASK_STACK)
        .spawn(move || cmd::run(task_ctx, job))
    {
        Ok(handle) => list.push(handle),
        Err(error) => ctx.error(&format!("kfc: 启动任务线程失败: {error}")),
    }
}
