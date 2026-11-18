//! 宿主适配层：说明「怎么和 fuyubot 通信」，不含识别业务。
//!
//! 线程模型（三条硬约束决定了它）：
//!
//! 1. `updateNewMessage` 回调跑在宿主的 TDLib 接收线程上，**不能阻塞**；
//! 2. 下载文件 / 回复消息都要等响应，而响应同样由接收线程投递——在回调里等就是自己等自己；
//! 3. 插件卸载前必须 join 自己起的线程，否则动态库卸载后仍有代码在跑。
//!
//! 所以：回调只把更新丢进 channel，一个常驻路由线程负责解析与分发，
//! 每条命令再开一个短命业务线程跑「下载 → 识别 → 回复」。

pub mod command;
pub mod download;

pub mod messenger;
pub mod photo;
pub mod service;
/// 测试替身宿主（只在 cfg(test) 下编译）。
#[cfg(test)]
pub mod test_host;
pub mod worker;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fuyubot_plugin_api::{HostApi, HostExt};

use crate::core::Config;
use crate::engine::EnginePool;

/// 共享上下文。所有字段都 `Sync`，可以放进 `Arc` 交给业务线程与服务回调。
pub struct Ctx {
    /// 宿主能力。
    pub host: HostApi,
    /// 运行期配置（构造一次，之后只读）。
    pub config: Config,
    /// 引擎池（懒加载 + 并发闸门）。
    pub engine: Arc<EnginePool>,
    shutdown: AtomicBool,
}

impl Ctx {
    /// 组装上下文。
    pub fn new(host: HostApi, config: Config, engine: Arc<EnginePool>) -> Self {
        Self {
            host,
            config,
            engine,
            shutdown: AtomicBool::new(false),
        }
    }

    /// 通知所有线程退出（`stop` 里调用）。
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// 是否已停机。
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// 日志简写。
    pub fn info(&self, message: &str) {
        self.host.info(message);
    }

    /// 日志简写。
    pub fn warn(&self, message: &str) {
        self.host.warn(message);
    }

    /// 日志简写。
    pub fn error(&self, message: &str) {
        self.host.error(message);
    }
}
