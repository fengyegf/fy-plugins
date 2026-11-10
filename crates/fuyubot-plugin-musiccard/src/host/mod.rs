//! 宿主适配层（消息收发 / 更新调度）。
//!
//! 线程模型（三条硬约束决定了它）：
//!
//! 1. `updateNewMessage` 回调跑在宿主的 TDLib 接收线程上，**不能阻塞**；
//! 2. 发消息 / 发照片要等 `response` 通道的响应，而响应同样由接收线程投递——
//!    在回调里等就是自己等自己；
//! 3. 插件卸载前必须 join 自己起的线程，否则动态库被卸载后仍有代码在跑。
//!
//! 所以：回调只把更新丢进 channel，一个常驻路由线程负责解析，每条命令再开一个
//! 短命业务线程跑完整流程（阻塞式取歌 / 下载封面 / 渲染 / 上传）。

pub mod messenger;
pub mod worker;

use std::sync::atomic::{AtomicBool, Ordering};

use fuyubot_plugin_api::{HostApi, HostExt};

use crate::config::Config;
use messenger::Messenger;

/// 一条命令消息的最小上下文。
#[derive(Debug, Clone, Copy)]
pub struct Msg {
    pub chat_id: i64,
    /// 触发命令的那条消息 id（回复它）。
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

    pub fn warn(&self, message: &str) {
        self.host.warn(message);
    }

    pub fn error(&self, message: &str) {
        self.host.error(message);
    }
}
