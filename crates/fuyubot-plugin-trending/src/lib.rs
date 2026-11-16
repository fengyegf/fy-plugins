//! trending：GitHub Trending 榜单卡片插件。
//!
//! 插件基于 FuyuBot SDK Native ABI v6：命令进来的文本消息在业务线程里解析，
//! 抓 `https://github.com/trending?since=…` 的 HTML 解析成榜单数据，
//! 再把数据交给 `assets/card.html` 里的 Vue 模板（`vue: true` + `data`），
//! 由宿主上的 `fuyubot.html2img` 服务渲染成图片。
//!
//! 分层：
//!
//! - [`host`]：宿主适配层，只说「怎么和 fuyubot 通信」（消息、订阅、线程模型）；
//! - [`cmd`]：命令层，`/trending` 一条命令一个文件；
//! - [`core`]：与宿主无关的能力（抓取、HTML 解析、数据模型、配置）；
//! - [`render`]：图像生成，Vue 卡片模板 + 数据 → `fuyubot.html2img` → 本机图片文件；
//! - [`help`]：`/help` 联动——把 `/trending` 登记进宿主的命令表
//!   （`fuyubot.host.commands`），内置 `/help` 只是这张表的读取方。

// 模块一律 `pub`：crate 额外编了 rlib，`cargo test` / `examples/` 才能直接调内部模块。
// 这不影响宿主加载 cdylib —— 宿主只找 `plugin_*` 那几个导出符号。
pub mod cmd;
pub mod core;
pub mod help;
pub mod host;
pub mod render;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, define_plugin};

use crate::core::Config;
use crate::host::worker::{Worker, on_update};

/// 插件主体。字段很少：真正的状态都在 [`Worker`] 里。
struct Trending {
    host: HostApi,
    /// 订阅句柄**必须活着**：它 drop 的那一刻就退订。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for Trending {
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
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        self.host.info("trending: 已启动");
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // 把 `/trending` 登记进宿主的命令表；登记失败只记 warn，不影响命令本身。
        help::register(&self.host);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("trending: 已停止");
    }

    /// 硬依赖渲染服务：没装 `fuyubot.html2img` 时插件停在 Pending，
    /// 而不是启动后每条命令都渲染失败。
    fn inject() -> Vec<String> {
        vec![render::html2img::SERVICE.to_owned()]
    }
}

define_plugin!(Trending, "trending");
