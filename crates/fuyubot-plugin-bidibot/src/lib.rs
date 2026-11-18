//! bidibot：双向私聊中转。
//!
//! 用户私聊 bot → bot 把消息**转发**进论坛群；每个用户一个话题（话题名 = 用户昵称）；
//! 主人在这条话题里回复 → bot 用**自己的身份重新发一条纯文本**给用户（不转发、不暴露主人）。
//!
//! 分层：
//! - [core]：配置 / 本地库 / 语言 / 翻译 / 广告 / LLM（不碰宿主）；
//! - [host]：订阅、线程调度、发送与取用户信息（宿主适配）；
//! - [relay]：双向中转主链路（私聊 ⇄ 群话题）；
//! - [cmd]：命令层（前缀解析 + 主人管理命令）；
//! - [help]：命令表登记（宿主持有 /help 的数据源）。
//!
//! 安全边界：主人消息只重发不转发，回调里不阻塞，
//! 封禁只影响双向私聊、基础命令照常可用。

pub mod cmd;
pub mod core;
pub mod help;
pub mod host;
pub mod relay;
pub mod render;

use fuyubot_plugin_api::{HostApi, HostExt, Plugin, Subscription, define_plugin};

use crate::core::Config;
use crate::host::Worker;

/// 插件名：同时是 [plugin.fengyegf.bidibot] 段名。
pub const PLUGIN_NAME: &str = "fengyegf.bidibot";

struct Bidibot {
    host: HostApi,
    /// 订阅句柄必须活着：它 drop 的那一刻就退订了。
    subscription: Option<Subscription>,
    worker: Option<Worker>,
}

impl Plugin for Bidibot {
    fn init(host: HostApi) -> Result<Self, String> {
        Ok(Self {
            host,
            subscription: None,
            worker: None,
        })
    }

    fn start(&mut self, config_json: &str) -> Result<(), String> {
        let config = Config::load(&self.host, config_json);
        if config.group_id == 0 {
            self.host.warn(
                "bidibot: [plugin.fengyegf.bidibot].group_id 还没配置——私聊消息只回「暂时不可用」，不会转发进群",
            );
        }
        if config.owner_id == 0 {
            self.host.warn(
                "bidibot: [plugin.fengyegf.bidibot].owner_id 还没配置——群里任何人的回复都不会转给用户（只能看不能回）",
            );
        }
        let worker = Worker::start(self.host, config.clone())?;
        let subscription =
            self.host
                .subscribe_json("updateNewMessage", None, on_update, worker.sender_ptr());
        let Some(subscription) = subscription else {
            return Err("订阅 updateNewMessage 失败".to_owned());
        };

        self.host.info(&format!(
            "bidibot: 已启动（群 {}，主人 {}，广告 {:?}，库 {}）",
            if config.group_id == 0 {
                "未配置".to_owned()
            } else {
                config.group_id.to_string()
            },
            if config.owner_id == 0 {
                "未配置（管理命令不可用）".to_owned()
            } else {
                config.owner_id.to_string()
            },
            config.effective_ad_mode(),
            config.store_path
        ));
        self.subscription = Some(subscription);
        self.worker = Some(worker);

        // html2img 是软依赖：装载顺序不保证，启动这一刻看不到不代表真没有——
        // 真正的判断放在每次出名片时（render::render_card），这里只是报一声现状。
        if crate::render::available(&self.host) {
            self.host
                .info("bidibot: 检测到 fuyubot.html2img，名片用图片卡片");
        } else {
            self.host.info(
                "bidibot: 启动时还没看到 fuyubot.html2img，先按文字版兜底（出名片时会再探一次）",
            );
        }

        // 命令表联动是可选：登记失败只记 warn，不影响中转本身。
        if config.command_table {
            help::register(&self.host);
        }
        Ok(())
    }

    fn stop(&mut self) {
        // 顺序要紧：先退订（不再有新回调），再停线程。
        self.subscription = None;
        self.worker = None;
        self.host.info("bidibot: 已停止");
    }
}

/// 宿主回调：只把更新投进 channel，不做任何阻塞操作。
extern "C-unwind" fn on_update(
    user: *mut std::ffi::c_void,
    payload: *const std::ffi::c_char,
) -> i32 {
    crate::host::worker::on_update(user, payload)
}

define_plugin!(Bidibot, "fengyegf.bidibot");
