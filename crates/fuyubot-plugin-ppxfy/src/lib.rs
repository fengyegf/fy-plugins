//! ppxfy：下载 / 解压 / 打包一条龙的 fuyubot 插件。
//!
//! 入口极薄，分层明确：
//!
//! - [`cmd`]：命令层，一个命令一个文件（`download` / `galgame` / `cancel` / `clearcache`），带命令表与 panic 兜底；
//! - [`host`]：宿主适配层，只说「怎么和 fuyubot 通信」（消息、回复、任务、进度、调度）；
//! - [`core`]：通用能力，一个能力一个文件（`fetch` / `archive` / `tree` / `page` / `config`）；
//! - [`db`]：galgame 作品记录，经 mongo-plugin 的 `storage.mongo` 服务读写；
//! - [`galgame`]：作品信息，按站点拆文件（`dmm` / `dlsite` / `dom` / `text`）；
//! - [`telegraph`]：把作品图集发成 Telegraph 页面；
//! - [`unpack`] / [`package`]：解压与打包；
//! - [`util`]：纯工具，不碰宿主、不含流程（文件操作、7z、乱码名修复）；
//! - [`help`]：`/help` 联动——把本插件的四条命令登记进**宿主**的命令表
//!   `fuyubot.host.commands`（命令表归宿主，图片版 `/help` 只是读取方）。

mod cmd;
mod core;
mod db;
mod galgame;
mod help;
mod host;
mod package;
mod telegraph;
mod unpack;
mod util;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, define_plugin};

use crate::core::Config;
use crate::host::worker::{Worker, on_update};

struct Ppxfy {
    host: HostApi,
    /// 订阅句柄**必须活着**：它 drop 的那一刻就退订了。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for Ppxfy {
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
            // worker 在此 drop：线程已 join，sender 随之释放，不会留下悬垂指针。
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        self.host.info("ppxfy: 已启动");
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // 可选联动：把四条命令登记进宿主命令表 `fuyubot.host.commands`。
        // 服务由宿主预置，登记失败（跨插件重名 / 旧宿主）也只记 warn。
        help::register(&self.host);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("ppxfy: 已停止");
    }

    /// 软依赖：只有可选的 `storage.mongo`。
    ///
    /// 写**服务名**（Cordis 的 inject 语义）而不是插件名：业务依赖的是「谁能提供
    /// 这个能力」，至于由哪个插件提供与 ppxfy 无关。
    ///
    /// - `storage.mongo`：装了 mongo-plugin 就把作品记录入库，没装照常跑。
    ///
    /// 命令表 `fuyubot.host.commands` **不在这里**：它由宿主在启动时预置，不是某个
    /// 插件提供的服务，不需要软依赖去等；登记见 [`help`]，失败也只记 warn。
    /// 旧的 `fuyubot.help.commands` 只是兼容别名，已从软依赖里删掉。
    fn soft_inject() -> Vec<String> {
        vec!["storage.mongo".to_owned()]
    }
}

define_plugin!(Ppxfy, "ppxfy");
