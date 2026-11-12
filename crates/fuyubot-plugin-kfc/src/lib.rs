//! kfc：点个肯德基吧——抓一句「肯德基疯话」，渲染成推特卡片发回去。
//!
//! 从 TS 版 `plugins/kfc`（`index.ts` + `kfc.vue` + `genImg.ts` 的 satori/resvg 管线）
//! 迁移而来。卡片界面是 `assets/card.html` 里的 Vue 模板，插件只把文案凑成 data，交给宿主上的 `fuyubot.html2img` 服务
//! （HTML/CSS → PNG/JPEG，纯 Rust，无 Chromium），不再依赖 Node / Vue / sharp。
//!
//! 分层：
//!
//! - [`host`]：宿主适配层，只说「怎么和 fuyubot 通信」（消息、订阅、线程模型）；
//! - [`cmd`]：命令层，`/kfc` 一条命令一个文件；
//! - [`core`]：与宿主无关的能力（取文案、清洗文本、配置）；
//! - [`render`]：图像生成，`assets/card.html` 的 Vue 模板 + 数据 → `fuyubot.html2img` → 本机图片文件；
//! - [`help`]：`/help` 联动——把 `/kfc` 登记进内置帮助的命令表。
//!
//! 线程模型与 ABI 约束见 [`host`] 模块文档：宿主回调只投递，业务线程里才做阻塞操作。

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
struct Kfc {
    host: HostApi,
    /// 订阅句柄**必须活着**：drop 的那一刻就退订。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for Kfc {
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

        self.host.info("kfc: 已启动");
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // 可选联动：把 `/kfc` 登记进内置 `/help`；help 不在也只记一条 warn。
        help::register(&self.host);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("kfc: 已停止");
    }

    /// 硬依赖渲染服务：没装 `fuyubot.html2img` 时插件停在 Pending，
    /// 而不是启动后每条命令都渲染失败。
    fn inject() -> Vec<String> {
        vec![render::html2img::SERVICE.to_owned()]
    }

    /// 软依赖内置 `/help` 的命令表：有它就排在它后面，好把 `/kfc` 登记进去；
    /// 没有也照常启动（登记失败只记日志）。
    fn soft_inject() -> Vec<String> {
        vec![help::SERVICE.to_owned()]
    }
}

define_plugin!(Kfc, "kfc");
