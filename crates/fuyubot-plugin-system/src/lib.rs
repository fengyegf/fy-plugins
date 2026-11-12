//! system：系统信息卡片插件（Rust）。
//!
//! 从 TS 版 `E:\\Fuyu_TDBot\\plugins\\system`（`index.ts` + `system.vue`）迁移而来。
//! 插件只把业务数据映射成 Vue 模板的 `data`（模板在 `assets/card.html`），渲染交给
//! 宿主上的 `fuyubot.html2img` 服务（Vue 模板 + data → JPEG，纯 Rust，无 Chromium / Node），
//! 不再依赖 Node / Vue / satori / resvg / sharp，也不再链接 hyper-render。
//!
//! 分层（任一层都能单独 `cargo test`）：
//!
//! - [`host`]：宿主适配层，只说「怎么和 fuyubot 通信」（消息、订阅、线程模型）；
//! - [`cmd`]：命令层，`/system` 一条命令一个文件；
//! - [`core`]：与宿主无关的能力（配置、系统信息采集、TDLib 查询、数据模型）；
//! - [`render`]：图像生成，Vue 模板 + `data` → `fuyubot.html2img` → 本机图片文件；
//! - [`help`]：`/help` 联动——把 `/system` 登记进内置帮助的命令表。
//!
//! 线程模型见 [`host`] 模块文档：宿主回调只投递，业务线程里才做阻塞操作。

pub mod cmd;
pub mod core;
pub mod help;
pub mod host;
pub mod render;

use std::path::PathBuf;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, define_plugin};

use crate::core::Config;
use crate::host::worker::{Worker, on_update};

/// 插件主体。字段很少：真正的状态都在 [`Worker`] 里。
struct SystemPlugin {
    host: HostApi,
    /// 订阅句柄**必须活着**：drop 的那一刻就退订。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for SystemPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        host.info("system: init");
        Ok(Self {
            host,
            subscription: None,
            worker: None,
        })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let config = Config::load(&self.host, config_json);
        let worker = Worker::start(self.host, config, plugins_dir_from(config_json))?;

        // `user_data` 指向 worker 持有的 channel 发送端；worker 必须比订阅活得久，
        // 所以先建 worker、再订阅，stop 时反过来先退订再 drop worker。
        let subscription =
            self.host
                .subscribe_json("updateNewMessage", None, on_update, worker.sender_ptr());
        let Some(subscription) = subscription else {
            // worker 在此 drop：线程已 join，sender 随之释放，不会留下悬垂指针。
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        self.host.info("system: 已启动");
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // 可选联动：把 `/system` 登记进内置 `/help`；help 不在也只记一条 warn。
        help::register(&self.host);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("system: 已停止");
    }

    /// 硬依赖渲染服务：没装 `fuyubot.html2img` 时插件停在 Pending，
    /// 而不是启动后每条命令都渲染失败。
    fn inject() -> Vec<String> {
        vec![render::html2img::SERVICE.to_owned()]
    }

    /// 软依赖内置 `/help` 的命令表：有它就排在它后面，好把 `/system` 登记进去；
    /// 没有也照常启动（登记失败只记日志）。
    fn soft_inject() -> Vec<String> {
        vec![help::SERVICE.to_owned()]
    }
}

/// 宿主在 `plugin_start` 的配置里给了插件目录（绝对路径），留给需要读插件文件的场景。
fn plugins_dir_from(config_json: &str) -> Option<PathBuf> {
    serde_json::from_str::<serde_json::Value>(config_json)
        .ok()?
        .get("plugins_dir")?
        .as_str()
        .map(PathBuf::from)
}

define_plugin!(SystemPlugin, "system");
