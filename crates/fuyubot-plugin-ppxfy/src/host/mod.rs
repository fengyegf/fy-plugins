//! 运行期骨架：一次插件启动对应一个 [`Ctx`]，被宿主回调线程与各业务流程线程共享。
//!
//! 线程模型（三条硬约束决定了它）：
//!
//! 1. `updateNewMessage` 回调跑在宿主的 TDLib 接收线程上，**不能阻塞**；
//! 2. 发消息 / 编辑消息要等响应，而响应同样由接收线程投递——在回调里等就是自己等自己；
//! 3. 插件卸载前必须 join 自己起的线程，否则动态库被卸载后仍有代码在跑。
//!
//! 所以：回调只把更新丢进 channel，一个常驻 worker 线程负责路由，
//! 每个命令再开一个短命线程跑完整流程（阻塞式下载 / 解压 / 打包）。

pub mod messenger;
pub mod progress;
pub mod reply;
pub mod task;
pub mod worker;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fuyubot_plugin_api::{HostApi, HostExt};

use crate::core::Config;
use messenger::Messenger;
use reply::{Inbox, Wait};
use task::{Task, Tasks};

/// 一次等待检查的时间片：所有阻塞等待都按它分段，以便及时响应取消 / 停机。
const WAIT_SLICE: Duration = Duration::from_millis(200);

/// 一条命令消息的最小上下文。
#[derive(Debug, Clone, Copy)]
pub struct Msg {
    pub chat_id: i64,
    pub id: i64,
}

/// 共享上下文。所有字段都是 `Sync`，可以安全地放进 `Arc` 交给业务线程。
pub struct Ctx {
    pub host: HostApi,
    pub config: Config,
    pub messenger: Messenger,
    pub inbox: Inbox,
    pub tasks: Tasks,
    shutdown: AtomicBool,
}

impl Ctx {
    pub fn new(host: HostApi, config: Config) -> Self {
        Self {
            host,
            messenger: Messenger::new(host),
            config,
            inbox: Inbox::default(),
            tasks: Tasks::default(),
            shutdown: AtomicBool::new(false),
        }
    }

    /// 通知所有线程退出（`stop` 里调用）。
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub fn info(&self, message: &str) {
        self.host.info(message);
    }

    pub fn warn(&self, message: &str) {
        self.host.warn(message);
    }

    pub fn error(&self, message: &str) {
        self.host.error(message);
    }

    /// 等一条回复。超时返回 [`Wait::Timeout`]，`/cancel` 返回 [`Wait::Cancelled`]。
    ///
    /// 等待按 [`WAIT_SLICE`] 分片，因此取消与停机能被及时观察到。
    pub fn wait_reply(
        &self,
        task: &Task,
        chat_id: i64,
        tips_message_id: i64,
        timeout: Duration,
    ) -> Wait {
        let registration = self.inbox.register(chat_id, tips_message_id);
        let deadline = Instant::now() + timeout;
        loop {
            if self.is_shutdown() {
                return Wait::Shutdown;
            }
            if task.is_cancelled() {
                return Wait::Cancelled;
            }
            let now = Instant::now();
            if now >= deadline {
                return Wait::Timeout;
            }
            if let Some(text) = registration.poll((deadline - now).min(WAIT_SLICE)) {
                return Wait::Reply(text);
            }
        }
    }
}

/// 以最多 `limit` 个线程并发处理 `items`，结果按输入顺序返回。
///
/// 用 [`std::thread::scope`] 而非 `'static` 线程：业务闭包可以直接借用 `Ctx`。
pub fn map_concurrent<T, R, F>(items: Vec<T>, limit: usize, function: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync,
{
    let total = items.len();
    if total == 0 {
        return Vec::new();
    }
    let queue = Mutex::new(items.into_iter().enumerate().collect::<Vec<_>>());
    let slots: Mutex<Vec<Option<R>>> = Mutex::new((0..total).map(|_| None).collect());
    let workers = limit.clamp(1, total);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let Some((index, item)) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop()
                    else {
                        break;
                    };
                    let result = function(item);
                    slots.lock().unwrap_or_else(|e| e.into_inner())[index] = Some(result);
                }
            });
        }
    });

    slots
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .flatten()
        .collect()
}

/// 截断过长文本，避免超过 Telegram 的 4096 字符上限导致整条消息发不出去。
pub fn clip(text: impl Into<String>, max: usize) -> String {
    let text = text.into();
    if text.chars().count() <= max {
        return text;
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(16)).collect();
    clipped.push_str("\n…（内容过长已截断）");
    clipped
}
