//! 宿主适配层：把 HostApi、配置、存储、发送器、限流收进一个可共享的 Ctx。

pub mod messenger;
pub mod worker;

pub use messenger::{Messenger, Sent, UserInfo};
pub use worker::{IncomingMessage, Worker};

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use fuyubot_plugin_api::{HostApi, HostExt, LogLevel};
use serde_json::Value;

use crate::core::config::Config;
use crate::core::llm::Client;
use crate::core::skills::Skills;
use crate::core::store::Store;

/// 运行期上下文：插件启动时构造一次，之后所有线程共享（Arc）。
///
/// 配置放在 Mutex<Arc<Config>> 里：读侧只要拿一份快照（很便宜），写侧整体替换，
/// 不存在"读锁还没放又去拿写锁"的死锁路径。
pub struct Ctx {
    host: HostApi,
    config: Mutex<Arc<Config>>,
    pub store: Arc<Store>,
    pub messenger: Messenger,
    pub skills: Skills,
    pub llm: Option<Arc<Client>>,
    /// 启动时间（unix 秒），/bidistatus 用。
    pub started_at: u64,
    shutdown: AtomicBool,
    /// 单用户中转限流：user_id -> 最近一分钟内的发生时刻（unix 秒）。
    limiter: Mutex<HashMap<i64, Vec<u64>>>,
}

impl Ctx {
    /// 建上下文：开本地库、装 LLM 客户端 / 翻译器 / skills。
    pub fn new(host: HostApi, config: Config) -> Result<Arc<Self>, String> {
        let store = Store::open(Path::new(&config.store_path))
            .map_err(|error| format!("打开本地库 {} 失败: {error}", config.store_path))?;
        let llm = config
            .ai_enabled()
            .then(|| Arc::new(Client::new(&config.ai)));
        let skills = Skills::new(config.skills_dir.clone());
        // 翻译器与广告判定共用同一份提示词来源（skills_dir 配到别处也生效）。
        Ok(Arc::new(Self {
            host,
            config: Mutex::new(Arc::new(config)),
            store: Arc::new(store),
            messenger: Messenger::new(host),
            skills,
            llm,
            started_at: now(),
            shutdown: AtomicBool::new(false),
            limiter: Mutex::new(HashMap::new()),
        }))
    }

    pub fn host(&self) -> HostApi {
        self.host
    }

    /// 当前配置快照。每条消息取一次即可，别在循环里反复取。
    pub fn config(&self) -> Arc<Config> {
        self.config
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 就地改配置（命令改语言 / 关键词用），同时**写回 fuyubot.toml**。
    ///
    /// 写回失败只记日志：内存里的新值照样生效，重启后可能回退到旧值。
    pub fn update_config(&self, key: &str, value: Value, apply: impl FnOnce(&mut Config)) {
        {
            let mut guard = self
                .config
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let mut next = (**guard).clone();
            apply(&mut next);
            *guard = Arc::new(next);
        }
        if let Err(error) = self.host.set_config_value(key, value.clone()) {
            self.host.warn(&format!(
                "bidibot: 写回配置 {key}={value} 失败（内存已生效）: {error}"
            ));
        } else {
            self.host.info(&format!("bidibot: 配置 {key} 已写回"));
        }
    }

    /// 重新读一遍 skills 目录（不做热更新，留给命令/重启）。
    pub fn reload_skills(&mut self) {
        let dir = self.config().skills_dir.clone();
        self.skills = Skills::new(dir);
    }

    /// 用配置里的发送超时发一条纯文本（失败只记日志）。
    ///
    /// 命令层与中转层统一走这里：超时是 [`Config::send_timeout`]，离线回放 / 弱网
    /// 排错时把它调小，整条链路就不会每条消息卡 30 秒。
    pub fn send(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<messenger::Sent> {
        let timeout = self.config().send_timeout();
        self.messenger
            .try_send_text_timeout(chat_id, text, reply_to, timeout)
    }

    /// 调试日志：HostExt 只给了 info / warn / error，debug 走 HostApi::log。
    pub fn debug(&self, message: &str) {
        self.host.log(LogLevel::Debug, message);
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// 单用户每分钟中转次数限制。limit = 0 表示不限制。
    ///
    /// 用"最近 60 秒内的时刻表"实现：超过上限直接拒绝，被拒绝的不记账。
    pub fn allow_rate(&self, user_id: i64) -> bool {
        let limit = self.config().rate_limit_per_min;
        if limit == 0 {
            return true;
        }
        let now = now();
        let mut guard = self
            .limiter
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let hits = guard.entry(user_id).or_default();
        hits.retain(|at| now.saturating_sub(*at) < 60);
        if hits.len() >= limit as usize {
            return false;
        }
        hits.push(now);
        true
    }

    /// 清掉限流表的过期项（长跑时防止只增不减）。
    pub fn prune_rate(&self) {
        let now = now();
        let mut guard = self
            .limiter
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.retain(|_, hits| {
            hits.retain(|at| now.saturating_sub(*at) < 60);
            !hits.is_empty()
        });
    }
}

/// 当前 unix 秒。
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 把秒数写成"3 分钟前 / 2 小时前"这种短描述（/bidilist 用）。
pub fn human_age(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds} 秒前"),
        60..=3_599 => format!("{} 分钟前", seconds / 60),
        3_600..=86_399 => format!("{} 小时前", seconds / 3_600),
        _ => format!("{} 天前", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_age_is_readable() {
        assert_eq!(human_age(0), "0 秒前");
        assert_eq!(human_age(59), "59 秒前");
        assert_eq!(human_age(120), "2 分钟前");
        assert_eq!(human_age(7_200), "2 小时前");
        assert_eq!(human_age(172_800), "2 天前");
    }
}
