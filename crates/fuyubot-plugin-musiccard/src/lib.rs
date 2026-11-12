//! musiccard：生成网易云音乐卡片的 fuyubot 插件（Rust）。
//!
//! 处理链路：命令接收 → 网易云取数 → 卡片数据 → Vue 模板 → `fuyubot.html2img` 服务出图 → 上传照片。
//!
//! 渲染不落在插件里：界面在 [`assets/card.html`](../assets/card.html)（`<style>` + Vue 模板），
//! 封面用网易云 CDN 的公网 URL、二维码用 `data:image/png;base64,…`，数据交给宿主上的
//! html2img 服务（服务名 `fuyubot.html2img`，方法 `render`，`vue: true`）。
//! 插件不拼 HTML、不链接 hyper-render，也不做任何位图合成。
//!
//! 模块划分：
//! - [`cmd`]：命令解析与业务流程；
//! - [`config`]：插件配置；
//! - [`help`]：`/help` 联动——把 `/musiccard` 登记进内置帮助的命令表；
//! - [`host`]：宿主适配（消息收发、更新调度）；
//! - [`model`]：跨模块共享的数据模型；
//! - [`netease`]：网易云接口；
//! - [`render`]：卡片数据映射 + 二维码 data URI + html2img 服务客户端；
//! - [`util`]：纯工具函数。

// 模块一律 `pub`：crate 额外编了 rlib，`cargo test` / `examples/` 才能直接调内部模块。
// 这不影响宿主加载 cdylib —— 宿主只找 `plugin_*` 那几个导出符号。
pub mod cmd;
pub mod config;
pub mod help;
pub mod host;
pub mod model;
pub mod netease;
pub mod render;
pub mod util;

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

    /// 硬依赖渲染服务：没装 `fuyubot.html2img` 时插件停在 Pending，
    /// 而不是启动后每条命令都渲染失败。
    fn inject() -> Vec<String> {
        vec![render::html2img::SERVICE.to_owned()]
    }

    /// 软依赖内置 `/help` 的命令表：有它就排在它后面，好把 `/musiccard` 登记进去；
    /// 没有也照常启动（登记失败只记日志）。
    fn soft_inject() -> Vec<String> {
        vec![help::SERVICE.to_owned()]
    }
}

define_plugin!(MusicCard, "musiccard");
