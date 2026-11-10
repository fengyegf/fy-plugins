//! musiccard：生成网易云音乐卡片的 fuyubot 插件（Rust）。
//!
//! 处理链路：命令接收 → 网易云取数 → HTML 卡片渲染 → 上传照片。
//!
//! 模块划分：
//! - [`cmd`]：命令解析与业务流程；
//! - [`config`]：插件配置；
//! - [`help`]：`/help` 联动——把 `/musiccard` 登记进内置帮助的命令表；
//! - [`host`]：宿主适配（消息收发、更新调度）；
//! - [`model`]：跨模块共享的数据模型；
//! - [`netease`]：网易云接口；
//! - [`render`]：卡片渲染与图片合成；
//! - [`util`]：纯工具函数。

mod cmd;
mod config;
mod help;
mod host;
mod model;
mod netease;
mod render;
mod util;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, define_plugin};

use crate::config::Config;
use crate::host::worker::{Worker, on_update};

/// 插件主体：宿主 ABI 生命周期 + 自己的 worker 线程组。
struct MusicCard {
    host: HostApi,
    /// 订阅句柄**必须活着**：它 drop 的那一刻就退订了。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for MusicCard {
    fn init(host: HostApi) -> Result<Self, String> {
        host.info("musiccard: init");
        Ok(Self {
            host,
            subscription: None,
            worker: None,
        })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let config = Config::load(&self.host);
        let worker = Worker::start(self.host, config)?;

        // 只订阅消息更新：卡片命令是文本消息。
        let subscription =
            self.host
                .subscribe_json("updateNewMessage", None, on_update, worker.sender_ptr());
        let Some(subscription) = subscription else {
            // worker 在此 drop：线程已 join，sender 随之释放，不会留下悬垂指针。
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        self.host.info(&format!(
            "musiccard: 已启动（宿主配置 {}）",
            if config_json.is_empty() {
                "{}"
            } else {
                config_json
            }
        ));
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // 可选联动：把 `/musiccard` 登记进内置 `/help`；help 不在也只记一条 warn。
        help::register(&self.host);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("musiccard: 已停止");
    }

    /// 软依赖内置 `/help` 的命令表：有它就排在它后面，好把 `/musiccard` 登记进去；
    /// 没有也照常启动（登记失败只记日志）。
    fn soft_inject() -> Vec<String> {
        vec![help::SERVICE.to_owned()]
    }
}

define_plugin!(MusicCard, "musiccard");
