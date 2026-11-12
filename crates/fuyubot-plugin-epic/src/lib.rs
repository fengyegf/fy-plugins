//! epic：Epic 免费游戏卡片（Rust 版）。
//!
//! 处理链路：抓 Epic 商店促销接口 → 筛出「当前免费 / 即将免费」→ 整理成 Vue 模板的
//! `data` 交给宿主上的 `fuyubot.html2img` 服务渲染成卡片图 → 以照片回复命令消息。
//!
//! 界面不在 Rust 里拼：卡片版式、循环与条件都在 `assets/card.html`（Vue 模板 +
//! 内联 CSS）里，插件只给数据。渲染器（hyper-render 的本地 fork）只存在于 html2img
//! 插件里，本插件不链接渲染引擎，也不下载 / 合成封面。
//!
//! 分层：
//! - [`core`]：配置、HTTP、Epic 接口与数据模型（不碰宿主）；
//! - [`render`]：Vue 模板数据映射 + `fuyubot.html2img` 服务客户端（不碰宿主）；
//! - [`host`]：宿主适配（订阅、消息发送、线程调度）；
//! - [`cmd`]：命令层（`/epic` 一个命令一个文件）；
//! - [`help`]：`/help` 联动——把 `/epic` 登记进内置帮助的命令表。

// 模块一律 `pub`：crate 额外编了 rlib，`cargo test` / `examples/` 才能直接调内部模块。
// 这不影响宿主加载 cdylib —— 宿主只找 `plugin_*` 那几个导出符号。
pub mod cmd;
pub mod core;
pub mod help;
pub mod host;
pub mod render;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, define_plugin};

use crate::core::Config;
use crate::host::worker::Worker;

struct Epic {
    host: HostApi,
    /// 订阅句柄**必须活着**：它 drop 的那一刻就退订了。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for Epic {
    fn init(host: HostApi) -> Result<Self, String> {
        Ok(Self {
            host,
            subscription: None,
            worker: None,
        })
    }

    fn start(&mut self, _config_json: &str) -> Result<(), String> {
        let config = Config::load(&self.host);
        let worker = Worker::start(self.host, config)?;

        let subscription =
            self.host
                .subscribe_json("updateNewMessage", None, on_update, worker.sender_ptr());
        let Some(subscription) = subscription else {
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        self.host.info("epic: 已启动");
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // 可选联动：把 `/epic` 登记进内置 `/help`；help 不在也只记一条 warn。
        help::register(&self.host);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("epic: 已停止");
    }

    /// 硬依赖渲染服务：没装 `fuyubot.html2img` 时插件停在 Pending，
    /// 而不是启动后每条命令都渲染失败。
    fn inject() -> Vec<String> {
        vec![render::html2img::SERVICE.to_owned()]
    }

    /// 软依赖内置 `/help` 的命令表：有它就排在它后面，好把 `/epic` 登记进去；
    /// 没有也照常启动（登记失败只记日志）。
    fn soft_inject() -> Vec<String> {
        vec![help::SERVICE.to_owned()]
    }
}

/// 宿主回调：只把更新投进 channel，不做任何阻塞操作。
extern "C-unwind" fn on_update(
    user: *mut std::ffi::c_void,
    payload: *const std::ffi::c_char,
) -> i32 {
    crate::host::worker::on_update(user, payload)
}

define_plugin!(Epic, "epic");
