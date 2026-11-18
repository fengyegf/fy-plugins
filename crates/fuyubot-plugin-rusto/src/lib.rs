//! rusto：RustO!（`rusto-rs`）纯 Rust OCR 的 FuyuBot 封装。
//!
//! 三层，互不越界：
//!
//! - [`engine`]：与宿主无关的识别能力。`OcrBackend` 是唯一引擎接缝，
//!   默认实现是 RustO!；[`engine::EnginePool`] 负责懒加载、并发闸门与超时。
//! - [`host`]：宿主适配层——收图、下载、回复、命令表、共享服务/工具。
//! - [`cmd`]：命令层，`/ocr` 一条命令拆一个文件。
//!
//! 线程模型：`updateNewMessage` 回调只把更新投进 channel（跑在 TDLib 接收线程上，
//! 阻塞就会卡死整个宿主）；路由线程解析后为每条命令开一个短命业务线程；
//! 引擎只在业务线程里碰，服务方法只做「排队」，不在调用方线程上做重活。

pub mod cmd;
pub mod core;
pub mod engine;
pub mod host;

use std::sync::Arc;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, ToolHandle, define_plugin};

use crate::core::Config;
use crate::engine::{EngineConfig, EnginePool};
use crate::host::Ctx;
use crate::host::service::Service;
use crate::host::worker::{Worker, on_update};

/// 插件主体：只持有生命周期对象，业务状态都在 [`Ctx`] 里。
struct Rusto {
    host: HostApi,
    /// 订阅句柄 drop 即退订；必须比 worker 活得久。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
    /// 服务与工具回调的 `user_data` 指向它，必须活到 `stop`。
    ctx: Option<Arc<Ctx>>,
    /// 服务 VTable 与它引用的缓冲必须与注册同生共死。
    service: Option<Box<Service>>,
    tool: Option<ToolHandle>,
}

impl Plugin for Rusto {
    fn init(host: HostApi) -> Result<Self, String> {
        Ok(Self {
            host,
            subscription: None,
            worker: None,
            ctx: None,
            service: None,
            tool: None,
        })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let config = Config::load(&self.host, config_json);
        let engine = EnginePool::new(EngineConfig::from(&config));
        let ctx = Arc::new(Ctx::new(self.host, config, engine));

        let worker = Worker::start(ctx.clone())?;
        // 先建 worker、再订阅：回调的 user_data 指向 worker 持有的发送端。
        let Some(subscription) =
            self.host
                .subscribe_json("updateNewMessage", None, on_update, worker.sender_ptr())
        else {
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        // 命令表登记失败只影响 /help 一行，不阻塞插件。
        host::command::register(&self.host);

        // 服务与工具：给其它插件 / 宿主 AI 的稳定入口。
        let service = Service::register(&self.host, ctx.clone());
        let tool = host::service::register_tool(&self.host, ctx.clone());

        self.host.info("rusto: 已启动（/ocr 可用；服务 rusto.ocr）");
        self.subscription = Some(subscription);
        self.worker = Some(worker);
        self.ctx = Some(ctx);
        self.service = service;
        self.tool = tool;
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调）→ 停线程 → 放下服务 VTable。
        self.subscription = None;
        self.worker = None;
        self.service = None;
        self.tool = None;
        self.ctx = None;
        self.host.info("rusto: 已停止");
    }
}

define_plugin!(Rusto, "rusto");
