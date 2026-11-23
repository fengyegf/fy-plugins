//! 插件配置：`[plugin.fengyegf.agent]` 段。
//!
//! 宿主已经把 `config.default.toml`、运行时落盘与用户配置合并好，
//! 这里只负责把 JSON 解析成 Rust 结构并给出默认值（缺键不致命）。

use std::path::PathBuf;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde::{Deserialize, Serialize};

use crate::PLUGIN_ID;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub gateway_service: String,
    pub gateway_method: String,
    pub model: String,
    pub system_prompt: String,
    /// 是否在系统提示后附加「Telegram 富消息支持的 Markdown 语法」说明。
    pub format_guide: bool,
    pub max_history: usize,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    /// 是否把宿主工具表按协议下发给模型（true = 全部工具，按 `tool_denylist` 屏蔽个别）。
    pub model_tools: bool,
    /// 模型连续调用工具的最大轮数；用尽后再问一次且不再下发工具，逼它给结论。
    pub max_tool_rounds: u32,
    /// 不下发给模型的工具名：精确名，或以 `*` 结尾的前缀；空 = 全部下发。
    pub tool_denylist: Vec<String>,
    /// 单个工具结果回填给模型的最大字符数（防止一个工具把上下文撑爆）。
    pub tool_result_max_chars: usize,
    pub reply_mode: ReplyMode,
    /// 私聊是否用 TDLib 草稿消息（sendTextMessageDraft）做「Thinking… → 流式」。
    pub stream_replies: bool,
    /// 流式草稿每次追加的字符数。
    pub stream_chunk_chars: usize,
    /// 流式草稿之间的间隔（毫秒）。
    pub stream_interval_ms: u64,
    /// 流式总时长上限（秒）；超过就整段补完，避免长文本拖太久。
    pub stream_max_seconds: u64,
    /// 是否给用户显示「停止生成」按钮（私有聊天草稿才有）。
    pub stream_can_stop: bool,
    /// 流式草稿用富消息（`sendRichMessageDraft`）；false 退回空文本草稿。
    pub rich_drafts: bool,
    /// 富消息前把 LaTeX（\(...\) / \[...\] / 单行 $$...$$）归一成宿主认识的写法。
    pub normalize_math: bool,
    /// 富草稿里 `inputPageBlockThinking` 块的文案。
    pub thinking_text: String,
    /// 网关提供流式方法（stream）时，私聊用真 token 流更新草稿。
    pub gateway_stream: bool,
    /// 一次流式会话的总超时（毫秒）。
    pub stream_timeout_ms: u64,
    /// 两次增量之间最多等多久（毫秒）；超时视为模型卡住。
    pub stream_idle_timeout_ms: u64,
    /// 流式轮询间隔（毫秒）。
    pub stream_poll_ms: u64,
    pub send_timeout_ms: u64,
    pub action_timeout_ms: u64,
    pub allow_private: bool,
    pub allow_groups: bool,
    pub group_require_mention: bool,
    pub allow_bots: bool,
    pub bot_allowlist: Vec<String>,
    pub bot_max_depth: u32,
    pub bot_rate_limit_secs: u64,
    pub dedupe_window_secs: u64,
    pub topic_auto_create: bool,
    pub topic_title_max_chars: usize,
    /// 新对话首次聊天时用首条消息内容设置话题标题（不额外调用模型）。
    pub topic_title_auto: bool,
    pub topic_icon_color: i64,
    pub session_dir: String,
    pub session_max_sessions: usize,
    pub session_max_messages: usize,
    /// 会话自动清理天数：超过这个天数没活动的会话会被删除；0 = 永久保留（不自动删）。
    pub session_ttl_days: u64,
    pub commands: bool,
    pub tools: bool,
    pub service: bool,
    /// 首次失败后的重试次数（0 = 不重试，总共尝试 retry_times + 1 次）。
    pub retry_times: u32,
    /// 两次重试之间的等待（毫秒）。
    pub retry_delay_ms: u64,
    /// 重试耗尽后发给用户的文案；{times} = 重试次数，{error} = 失败原因。
    pub failure_text: String,
    pub max_reply_chars: usize,
    pub loop_max_turns: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            gateway_service: "fengyegf.llm".to_owned(),
            gateway_method: "chat".to_owned(),
            model: String::new(),
            system_prompt:
                "你是运行在 Telegram 里的 AI Agent。回答简洁、可执行；需要排版时用 Markdown。"
                    .to_owned(),
            format_guide: true,
            max_history: 20,
            max_tokens: Some(1024),
            temperature: Some(0.6),
            model_tools: true,
            max_tool_rounds: 4,
            tool_denylist: Vec::new(),
            tool_result_max_chars: 4000,
            reply_mode: ReplyMode::Rich,
            stream_replies: true,
            stream_chunk_chars: 30,
            stream_interval_ms: 200,
            stream_max_seconds: 6,
            stream_can_stop: true,
            rich_drafts: true,
            normalize_math: true,
            thinking_text: "".to_owned(),
            gateway_stream: true,
            stream_timeout_ms: 120_000,
            stream_idle_timeout_ms: 30_000,
            stream_poll_ms: 50,
            send_timeout_ms: 30_000,
            action_timeout_ms: 15_000,
            allow_private: true,
            allow_groups: true,
            group_require_mention: true,
            allow_bots: false,
            bot_allowlist: Vec::new(),
            bot_max_depth: 2,
            bot_rate_limit_secs: 5,
            dedupe_window_secs: 30,
            topic_auto_create: true,
            topic_title_max_chars: 24,
            topic_title_auto: true,
            topic_icon_color: 7_321_584,
            session_dir: String::new(),
            session_max_sessions: 500,
            session_max_messages: 200,
            session_ttl_days: 7,
            commands: true,
            tools: true,
            service: true,
            retry_times: 5,
            retry_delay_ms: 800,
            failure_text: "请求失败：{error}".to_owned(),
            max_reply_chars: 3500,
            loop_max_turns: 3,
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

        if config.gateway_service.trim().is_empty() {
            config.gateway_service = "fengyegf.llm".to_owned();
        }
        if config.gateway_method.trim().is_empty() {
            config.gateway_method = "chat".to_owned();
        }
        if config.max_history == 0 {
            config.max_history = 20;
        }
        // 契约下限：单条消息 Telegram 上限 4096，留点余量。
        config.max_reply_chars = config.max_reply_chars.clamp(200, 4000);
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
            "网关 {}:{}，模型 {}，回复 {:?}，上下文 {}，会话目录 {}",
            self.gateway_service,
            self.gateway_method,
            if self.model.trim().is_empty() {
                "网关默认"
            } else {
                &self.model
            },
            self.reply_mode,
            self.max_history,
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
