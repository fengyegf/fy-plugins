//! 收件箱与调度：宿主回调只投递，业务在自己的线程里跑。
//!
//! 路由线程做三件事：把 `updateNewMessage` 翻译成 [`Incoming`]、
//! 用 [`crate::cmd::parser`] 判断是不是 `/ocr`、
//! 为命中的命令开一个短命业务线程。

use std::ffi::{c_char, c_void};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fuyubot_plugin_api::parse_payload;
use serde_json::Value;

use crate::cmd;
use crate::cmd::parser;
use crate::host::Ctx;
use crate::host::photo::Incoming;

/// 空闲时的轮询间隔（用于观察停机标志）。
const IDLE_TICK: Duration = Duration::from_millis(200);
/// 业务线程栈：下载 + 识别（引擎深递归）+ 回复，给足 2 MiB。
const TASK_STACK: usize = 2 * 1024 * 1024;

/// 插件的工作线程组：一个路由线程 + 若干短命业务线程。
pub struct Worker {
    /// 停机标志挂在它上面：drop 时先置位，路由线程才会退出循环。
    ctx: Arc<Ctx>,
    thread: Option<JoinHandle<()>>,
    /// 必须活着：它就是回调拿到的那个指针的目标。
    sender: Box<mpsc::Sender<Value>>,
    /// 已起的业务线程；停机时逐个 join。
    handles: Handles,
}

/// 业务线程句柄表。
type Handles = Arc<Mutex<Vec<JoinHandle<()>>>>;

impl Worker {
    /// 起路由线程。
    pub fn start(ctx: Arc<Ctx>) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel::<Value>();
        let handles: Handles = Arc::new(Mutex::new(Vec::new()));

        let loop_ctx = ctx.clone();
        let loop_handles = handles.clone();
        let thread = std::thread::Builder::new()
            .name("rusto-worker".to_owned())
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
        // 顺序要紧：先让路由线程看到停机标志，否则 join 永远等下去。
        self.ctx.shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let handles = std::mem::take(
            &mut *self
                .handles
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
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
/// 签名与 ABI 的 `EventCallbackFn` 一致（`extern "C-unwind"`）。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C-unwind" fn on_update(user: *mut c_void, payload: *const c_char) -> i32 {
    if user.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: 宿主按 ABI 契约传的是临时 NUL 结尾字符串。
    let update = unsafe { parse_payload(payload) };
    // SAFETY: `user` 来自 `Worker::sender_ptr`，由 `Worker` 持有。
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

/// 把一条更新翻译成命令并执行；不认识的更新静默忽略（与其它插件共存）。
fn dispatch(ctx: &Arc<Ctx>, handles: &Handles, update: Value) {
    let Some(incoming) = Incoming::parse(&update) else {
        return;
    };
    let command = match parser::parse(&incoming.text) {
        Some(command) => command,
        None => {
            // 没有命令时，只有「私聊 + 直接发图 + config.auto」才自动识别。
            if !ctx.config.auto || !incoming.is_private || incoming.file_id.is_none() {
                return;
            }
            parser::Command::Ocr {
                detail: ctx.config.detail_default,
            }
        }
    };
    spawn_task(ctx, handles, incoming, command);
}

/// 起一个业务线程跑完整流程。
fn spawn_task(ctx: &Arc<Ctx>, handles: &Handles, incoming: Incoming, command: parser::Command) {
    let task_ctx = ctx.clone();
    let job = cmd::Job { incoming, command };
    let mut list = handles.lock().unwrap_or_else(|error| error.into_inner());
    list.retain(|handle| !handle.is_finished());
    match std::thread::Builder::new()
        .name("rusto-task".to_owned())
        .stack_size(TASK_STACK)
        .spawn(move || cmd::handle(task_ctx, job))
    {
        Ok(handle) => list.push(handle),
        Err(error) => ctx.error(&format!("rusto: 启动任务线程失败: {error}")),
    }
}
