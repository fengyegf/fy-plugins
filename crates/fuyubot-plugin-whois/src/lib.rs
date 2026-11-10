//! whois：域名 Whois 信息查询卡片插件。
//!
//! 插件基于 FuyuBot SDK Native ABI v6，完整查询、渲染与消息发送链路均在 Rust
//! 进程内完成。图像由 `hyper-render` 的 Blitz + Vello 管线生成。
//!
//! 分层：
//!
//! - [`host`]：宿主适配层，只说「怎么和 fuyubot 通信」（消息、订阅、线程模型）；
//! - [`cmd`]：命令层，`/whois` 一条命令一个文件；
//! - [`core`]：与宿主无关的能力（whois / IP 接口、数据模型、域名清洗、配置）；
//! - [`render`]：图像生成，HTML 模板 → PNG，一行信息一张卡片；
//! - [`help`]：`/help` 联动——把 `/whois` 登记进内置帮助的命令表。
//!
//! 线程模型与 ABI 约束见 [`host`] 模块文档：宿主回调只投递，业务线程里才做阻塞操作。

mod cmd;
mod core;
mod help;
mod host;
mod render;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, define_plugin};

use crate::core::Config;
use crate::host::worker::{Worker, on_update};

/// 插件主体。字段很少：真正的状态都在 [`Worker`] 里。
struct Whois {
    host: HostApi,
    /// 订阅句柄**必须活着**：它 drop 的那一刻就退订。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for Whois {
    fn init(host: HostApi) -> Result<Self, String> {
        Ok(Self {
            host,
            subscription: None,
            worker: None,
        })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let config = Config::load(&self.host, config_json);
        let worker = Worker::start(self.host, config)?;

        // `user_data` 指向 worker 持有的 channel 发送端；worker 必须比订阅活得久，
        // 所以先建 worker、再订阅，stop 时反过来先退订再 drop worker。
        let subscription =
            self.host
                .subscribe_json("updateNewMessage", None, on_update, worker.sender_ptr());
        let Some(subscription) = subscription else {
            // worker 在此 drop：线程已 join，sender 随之释放，不会留下悬垂指针。
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        self.host.info("whois: 已启动");
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // 可选联动：把 `/whois` 登记进内置 `/help`；help 不在也只记一条 warn。
        help::register(&self.host);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("whois: 已停止");
    }

    /// 软依赖内置 `/help` 的命令表：有它就排在它后面，好把 `/whois` 登记进去；
    /// 没有也照常启动（登记失败只记日志）。
    fn soft_inject() -> Vec<String> {
        vec![help::SERVICE.to_owned()]
    }
}

define_plugin!(Whois, "whois");
