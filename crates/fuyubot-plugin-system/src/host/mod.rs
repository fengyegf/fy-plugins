//! 宿主适配层：说明「怎么和 fuyubot 通信」，不含任何业务。
//!
//! 线程模型（三条硬约束决定了它，与 whois / musiccard 迁移版一致）：
//!
//! 1. `updateNewMessage` 回调跑在宿主的 TDLib 接收线程上，**不能阻塞**；
//! 2. 发消息 / 上传图片 / 等 TDLib 响应都要等 `response` 通道，而响应同样由接收线程
//!    投递——在回调里等就是自己等自己；
//! 3. 插件卸载前必须 join 自己起的线程，否则动态库被卸载后仍有代码在跑。
//!
//! 所以：回调只把更新丢进 channel，一个常驻 worker 线程负责解析与分发，
//! 每条命令再开一个短命线程跑完整流程（系统信息采集 + TDLib 查询 + 渲染服务调用 + 上传）。

pub mod messenger;
pub mod worker;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use fuyubot_plugin_api::{HostApi, HostExt};

use crate::core::Config;
use crate::core::td::Td;
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
    /// TDLib 查询门面（`getMe` / `getOption` / `getUser` / 头像下载）。
    pub td: Td,
    pub messenger: Messenger,
    /// 宿主的插件目录（`plugin_start` 配置里给），留给需要读插件的场景。
    pub plugins_dir: Option<PathBuf>,
    shutdown: AtomicBool,
}

impl Ctx {
    pub fn new(host: HostApi, config: Config, plugins_dir: Option<PathBuf>) -> Self {
        Self {
            host,
            td: Td::new(host),
            messenger: Messenger::new(host),
            config,
            plugins_dir,
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
