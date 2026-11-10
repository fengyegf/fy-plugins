//! 运行期骨架：一次插件启动对应一个 [`Ctx`]，被宿主回调线程与业务线程共享。
//!
//! 线程模型沿用 ppxfy：宿主回调只把更新丢进 channel（它在 TDLib 接收线程上，不能阻塞），
//! 一个常驻 worker 线程负责路由，命令再各起一条短命线程跑完整流程。

pub mod messenger;
pub mod worker;

use std::sync::atomic::{AtomicBool, Ordering};

use fuyubot_plugin_api::{HostApi, HostExt};

use crate::core::Config;
use messenger::Messenger;

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
    shutdown: AtomicBool,
}

impl Ctx {
    pub fn new(host: HostApi, config: Config) -> Self {
        Self {
            host,
            messenger: Messenger::new(host),
            config,
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
}
