//! # fuyubot-plugin-agent
//!
//! Telegram 侧的 **AI Agent 运行时**。它不是"LLM 网关"——网关是 `fengyegf.llm`，
//! 本插件只做"把会话接上脑子并落到 Telegram"：
//!
//! - [`td`]：唯一碰 TDLib 的地方（订阅更新、发消息、话题、身份）；
//! - [`agent`]：Agent 运行时（网关客户端、提示词组装、单次 / 循环推理、防环）；
//! - [`session`]：会话与 JSON 落盘（按 chat + topic 归档，重启不丢）；
//! - [`api`]：对外接口（工具 + 共享服务 `fengyegf.agent`）；
//! - [`cmd`]：`/agent` 命令表；
//! - [`host`]：订阅回调 + 工作线程；
//! - [`core`]：配置、错误、纯数据类型。
//!
//! ## 消息链路
//!
//! ```text
//! updateNewMessage
//!   -> host::worker（回调只投递，不阻塞）
//!   -> 600ms 内同一会话的连发消息合并（只回答最后一条；命令不参与）
//!   -> 解析 / 准入 / 防环
//!   -> 私聊无话题且开了 topic_auto_create：createForumTopic 建新会话
//!   -> agent::AgentEngine 取历史 + 调网关 + （可选）循环迭代
//!   -> 思考占位 -> 编辑成终稿（rich / markdown / plain）
//!   -> 历史落盘
//! ```
//!
//! ## 对外接口
//!
//! - 工具：`fengyegf.agent.chat` / `.run` / `.send` / `.session` / `.title` / `.result`
//! - 服务：`fengyegf.agent`（方法 `chat` / `run` / `send` / `session` / `describe`）
//! - 命令：`/agent new|list|use|reset|compress|status|help`
//!
//! ## 依赖
//!
//! `fengyegf.llm` 是**硬依赖**（[`Plugin::inject`]）：没装网关时本插件停在 Pending，
//! 而不是启动后每条消息都只剩失败文案。运行时网关调用仍是同步阻塞的。

pub mod agent;
pub mod api;
pub mod cmd;
pub mod core;
pub mod host;
pub mod session;
pub mod td;

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, ToolHandle, define_plugin};

use crate::agent::AgentEngine;
use crate::core::Config;
use crate::session::SessionStore;
use crate::td::TdClient;

/// 插件 id（与 `plugin.json` 一致，也是配置段名）。
pub const PLUGIN_ID: &str = "fengyegf.agent";

/// 当前 unix 秒。
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// 插件运行期状态：工具处理器与服务方法都通过裸指针拿到它。
pub struct PluginState {
    /// 宿主能力（Copy，可跨线程复制）。
    pub host: HostApi,
    /// 合并后的配置。
    pub config: Arc<Config>,
    /// TDLib 门面。
    pub td: TdClient,
    /// 会话库。
    pub store: SessionStore,
    /// Agent 运行时。
    pub engine: AgentEngine,
    /// 长工具结果的短期缓存（分页 / 字段过滤；工具目录写、fengyegf.agent.result 读）。
    pub artifacts: Arc<agent::ArtifactStore>,
    /// 能力注册表：其它插件通过服务方法 register 把自己的能力登记到这里，
    /// 工具目录每轮读它（见 agent::registry 与 agent::tools）。
    pub registry: std::sync::Arc<agent::Registry>,
    /// 防环 / 限速状态（worker 与工具调用并发访问）。
    pub guard: Mutex<agent::Guard>,
    /// 用户按下「停止生成」的草稿：`(chat_id, forum_topic_id, draft_id)`。
    /// 订阅回调写入、worker 在流式块之间取走（只碰内存，不阻塞接收循环）。
    pub stops: Mutex<HashSet<(i64, i32, i64)>>,
    /// 启动时间。
    pub started_at: u64,
    /// 上次清理会话的时间（worker 空转时每小时最多清一次）。
    pub last_prune: Mutex<std::time::Instant>,
}

impl PluginState {
    /// 建状态：配置 → 门面 → 会话库 → 引擎。
    pub fn new(host: HostApi, config: Config) -> Result<Self, String> {
        let config = Arc::new(config);
        let td = TdClient::new(host);
        let store = SessionStore::open(&config).map_err(|error| error.to_string())?;
        let registry = Arc::new(agent::Registry::new());
        let artifacts = Arc::new(agent::ArtifactStore::new());
        let engine = AgentEngine::new(
            host,
            Arc::clone(&config),
            Arc::clone(&registry),
            Arc::clone(&artifacts),
        );
        let guard = Mutex::new(agent::Guard::new());
        Ok(Self {
            host,
            config,
            td,
            store,
            engine,
            artifacts,
            registry,
            guard,
            stops: Mutex::new(HashSet::new()),
            started_at: unix_now(),
            last_prune: Mutex::new(std::time::Instant::now()),
        })
    }

    /// 立刻清理过期会话（`session_ttl_days = 0` 表示永久，直接跳过）。
    pub fn prune_sessions_now(&self, host: &HostApi) {
        if self.config.session_ttl_days == 0 {
            return;
        }
        match self.store.prune() {
            Ok(removed) if removed > 0 => host.info(&format!(
                "agent: 已清理 {removed} 个超过 {} 天的会话",
                self.config.session_ttl_days
            )),
            Ok(_) => host.info("agent: 会话清理完成（没有过期会话）"),
            Err(error) => host.warn(&format!("agent: 清理会话失败：{error}")),
        }
    }

    /// 每小时最多清理一次（worker 空转时调用，避免每来一条消息就扫盘）。
    pub fn maybe_prune_sessions(&self, host: &HostApi) {
        if self.config.session_ttl_days == 0 {
            return;
        }
        let mut last = self
            .last_prune
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if last.elapsed() < std::time::Duration::from_secs(3_600) {
            return;
        }
        *last = std::time::Instant::now();
        drop(last);
        self.prune_sessions_now(host);
    }

    /// 记一次「用户停止生成」（订阅回调里调用）。
    pub fn record_stop(&self, chat_id: i64, forum_topic_id: i32, draft_id: i64) {
        self.stops
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert((chat_id, forum_topic_id, draft_id));
    }

    /// 取走停止标记（取到一次就消费掉）。
    pub fn take_stop(&self, chat_id: i64, forum_topic_id: i32, draft_id: i64) -> bool {
        self.stops
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&(chat_id, forum_topic_id, draft_id))
    }
}

/// 插件主体。
///
/// 字段顺序即析构顺序：**state 必须最后 drop**（工具 / 服务 / 线程都拿着它的裸指针）。
struct AgentPlugin {
    host: HostApi,
    worker: Option<host::Worker>,
    subscription: Option<Subscription>,
    stop_subscription: Option<Subscription>,
    service: Option<api::AgentService>,
    tools: Vec<ToolHandle>,
    state: Option<Box<PluginState>>,
}

impl Plugin for AgentPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        Ok(Self {
            host,
            worker: None,
            subscription: None,
            stop_subscription: None,
            service: None,
            tools: Vec::new(),
            state: None,
        })
    }

    fn start(&mut self, _config_json: &str) -> Result<(), String> {
        let config = Config::load(&self.host);
        if !config.enabled {
            self.host.info("agent: enabled = false，跳过订阅与注册");
            return Ok(());
        }

        let state = Box::new(
            PluginState::new(self.host, config).map_err(|error| format!("初始化失败：{error}"))?,
        );
        let state_ptr = (&*state as *const PluginState).cast_mut();

        // 启动时先清一次过期会话（session_ttl_days = 0 表示永久，内部直接跳过）。
        state.prune_sessions_now(&self.host);

        // 命令 / 工具 / 服务常开：关掉某一层不是「配置」，是部署事故；要停就停整个插件。
        cmd::register(&self.host);
        self.tools = api::register_tools(&self.host, state_ptr);
        let service = api::AgentService::new(state_ptr);
        let handle = self
            .host
            .provide_service_shared(api::SERVICE_NAME, service.vtable_ptr());
        if handle.is_valid() {
            self.service = Some(service);
        } else {
            self.host
                .warn("agent: 注册共享服务失败（同名服务已存在？）");
        }

        let worker = host::Worker::start(self.host, state_ptr)?;
        let subscription = self.host.subscribe_json(
            "updateNewMessage",
            None,
            host::worker::on_update,
            worker.sender_ptr(),
        );
        let Some(subscription) = subscription else {
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        // 「停止生成」：停止按钮按下时 TDLib 发 updateStopMessageDraft；回调只写内存标记。
        let stop_subscription = self.host.subscribe_json(
            "updateStopMessageDraft",
            None,
            host::worker::on_stop,
            state_ptr.cast::<c_void>(),
        );
        if stop_subscription.is_none() {
            self.host
                .warn("agent: 订阅 updateStopMessageDraft 失败，停止按钮不会生效");
        }

        self.host.info(&format!(
            "agent: 已启动（{}；工具 {} 个，命令与服务常开）",
            state.config.summary(),
            self.tools.len()
        ));
        self.worker = Some(worker);
        self.subscription = Some(subscription);
        self.stop_subscription = stop_subscription;
        self.state = Some(state);
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程，最后放状态。
        self.subscription = None;
        self.stop_subscription = None;
        self.worker = None;
        self.service = None;
        self.tools.clear();
        self.state = None;
        self.host.info("agent: 已停止");
    }

    /// 硬依赖 LLM 网关：没装 `fengyegf.llm` 时插件停在 Pending，
    /// 而不是启动后每条消息都只剩失败文案。
    fn inject() -> Vec<String> {
        vec![crate::core::consts::GATEWAY_SERVICE.to_owned()]
    }
}

define_plugin!(AgentPlugin, "fengyegf.agent");
