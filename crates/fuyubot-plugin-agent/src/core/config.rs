//! 插件配置：`[plugin.fengyegf.agent]` 段。
//!
//! 设计取舍：Agent 的行为尽量确定，配置只留「部署环境真的会不同」的项。
//! 会话历史默认**全量携带**（没有「每次带几条」这种旋钮），上下文太长用
//! `/agent compress` 让模型把历史压成摘要；其余节奏 / 超时 / 截断长度都在
//! [`crate::core::consts`] 里，不暴露成配置。

use std::path::PathBuf;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde::{Deserialize, Serialize};

use crate::PLUGIN_ID;
use crate::core::consts;

/// 回复形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyMode {
    /// 纯文本。
    Plain,
    /// Markdown（宿主 parseTextEntities 转 formattedText）。
    Markdown,
    /// 富消息（失败自动降级 markdown）。
    Rich,
}

/// 插件配置。
///
/// 只保留部署侧真的要改的键；删掉的键（`max_history` / `model_tools` /
/// `stream_*` / `retry_*` 等）不再读取，用户配置里留着也不会报错。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// 走网关的哪个方法：`chat` / `messages` / `responses`。
    pub gateway_method: String,
    /// 空 = 用网关配置里的 `default_model`。
    pub model: String,
    /// 系统提示词；留空则不注入 system 消息（富消息语法说明仍按需附加）。
    pub system_prompt: String,
    pub reply_mode: ReplyMode,
    /// 私聊是否用 TDLib 草稿消息做流式（群聊没有草稿能力，直接发终稿）。
    pub stream_replies: bool,
    pub allow_private: bool,
    pub allow_groups: bool,
    /// 群里必须 @ 我或回复我的消息才接。
    pub group_require_mention: bool,
    /// Bot-to-Bot 总开关（默认关）。
    pub allow_bots: bool,
    /// 允许对话的对端 bot 用户名（不带 @）；空 = 只按 `allow_bots` 放行。
    pub bot_allowlist: Vec<String>,
    /// 额外不下发给模型的工具名：精确名，或以 `*` 结尾的前缀。
    /// 只能追加：`fuyubot.td.functions` 是内置屏蔽，删不掉。
    pub tool_denylist: Vec<String>,
    /// 私聊首条消息自动开一个话题（新会话）。
    pub topic_auto_create: bool,
    /// 会话落盘目录；空 = `data/plugins/fengyegf.agent/sessions`。
    pub session_dir: String,
    /// 会话自动清理天数；0 = 永久保留（不自动删、也不按数量修剪）。
    pub session_ttl_days: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            gateway_method: consts::DEFAULT_GATEWAY_METHOD.to_owned(),
            model: String::new(),
            system_prompt:
                "你是运行在 Telegram 里的 AI Agent。回答简洁、可执行；需要排版时用 Markdown。"
                    .to_owned(),
            reply_mode: ReplyMode::Rich,
            stream_replies: true,
            allow_private: true,
            allow_groups: true,
            group_require_mention: true,
            allow_bots: false,
            bot_allowlist: Vec::new(),
            tool_denylist: Vec::new(),
            topic_auto_create: true,
            session_dir: String::new(),
            session_ttl_days: 7,
        }
    }
}

impl Config {
    /// 从宿主配置段读（段名 = 插件 id）。
    pub fn load(host: &HostApi) -> Self {
        let mut config = match host.config_value(PLUGIN_ID, "") {
            Some(value) => match serde_json::from_value::<Self>(value) {
                Ok(config) => config,
                Err(error) => {
                    host.warn(&format!("agent: 配置段解析失败，使用默认值：{error}"));
                    Self::default()
                }
            },
            None => {
                host.info("agent: 没有配置段，使用内置默认值");
                Self::default()
            }
        };

        if config.gateway_method.trim().is_empty() {
            config.gateway_method = consts::DEFAULT_GATEWAY_METHOD.to_owned();
        }
        config
    }

    /// 会话落盘目录（空配置 → 实例工作目录下的 data/plugins/<id>/sessions）。
    pub fn session_path(&self) -> PathBuf {
        let configured = self.session_dir.trim();
        if configured.is_empty() {
            PathBuf::from("data")
                .join("plugins")
                .join(PLUGIN_ID)
                .join("sessions")
        } else {
            PathBuf::from(configured)
        }
    }

    /// 一行摘要（启动日志与 describe 用）。
    pub fn summary(&self) -> String {
        format!(
            "网关 {}:{}，模型 {}，回复 {:?}，会话目录 {}",
            consts::GATEWAY_SERVICE,
            self.gateway_method,
            if self.model.trim().is_empty() {
                "网关默认"
            } else {
                &self.model
            },
            self.reply_mode,
            self.session_path().display()
        )
    }

    /// 是否按 bot 白名单放行这个用户名。
    pub fn bot_allowed(&self, username: Option<&str>) -> bool {
        if !self.allow_bots {
            return false;
        }
        if self.bot_allowlist.is_empty() {
            return true;
        }
        match username {
            Some(name) => self
                .bot_allowlist
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(name.trim_start_matches('@'))),
            None => false,
        }
    }
}
