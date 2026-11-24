//! 内置常量：Agent 的确定性行为。
//!
//! 配置只保留「部署环境真的会不同」的项（模型 / 网关方法 / 准入 / 目录 / TTL / 工具屏蔽）。
//! 其余旋钮收成常量：会话历史默认**全量携带**，上下文太长用 `/agent compress` 让模型把历史
//! 压成摘要——而不是让用户去调「每次带几条历史」「工具结果截多少字」这类实现细节。
//!
//! 改这里等于改 Agent 的默认人格与节奏，改之前先想清楚是不是所有人都该跟着变。

/// 网关共享服务名（硬依赖 `fengyegf.llm`，由 `Plugin::inject` 声明）。
pub const GATEWAY_SERVICE: &str = "fengyegf.llm";
/// 网关方法默认值（`chat` / `messages` / `responses`）。
pub const DEFAULT_GATEWAY_METHOD: &str = "chat";

/// 模型连续调用工具的最大轮数；用尽后再问一次且不再下发工具，逼它给结论。
pub const MAX_TOOL_ROUNDS: u32 = 8;
/// 单个工具结果回填给模型的最大字符数（防止一个工具把上下文撑爆）。
pub const TOOL_RESULT_MAX_CHARS: usize = 4_000;
/// 长结果分页时每页的字符数（artifact 第一页也用它）。
pub const TOOL_RESULT_PAGE_CHARS: usize = 3_500;
/// 长结果缓存时长（秒）：过期后模型再读会得到「不存在或已过期」。
pub const ARTIFACT_TTL_SECS: u64 = 600;
/// 内存里最多缓存多少个长结果。
pub const ARTIFACT_MAX_ENTRIES: usize = 64;
/// 长结果缓存总字节上限（UTF-8）。
pub const ARTIFACT_MAX_BYTES: usize = 16 * 1024 * 1024;
/// 单个长结果的字节上限；超过就不缓存，退回普通截断。
pub const ARTIFACT_MAX_ITEM_BYTES: usize = 2 * 1024 * 1024;
/// 读取工具一次最多返回多少逻辑字符（JSON 转义后实际长度可能略多；读工具不再触发分页）。
pub const ARTIFACT_READ_MAX_CHARS: usize = 3_500;
/// 字段过滤默认返回多少个值。
pub const ARTIFACT_VALUE_DEFAULT_LIMIT: usize = 200;
/// 字段过滤一次最多返回多少个值。
pub const ARTIFACT_VALUE_MAX_LIMIT: usize = 1_000;
/// 单个过滤值的字符上限（超长字符串会截断）。
pub const ARTIFACT_VALUE_MAX_CHARS: usize = 500;

/// 分块回放时每块的基础字符数。
pub const STREAM_CHUNK_CHARS: usize = 30;
/// 同一会话连续消息的合并窗口（毫秒）：窗口内到达的消息合成一次请求，只回答最后一条。
///
/// 用户连发几句时，不要每句都请求一次模型；等一小段静默再一起送进去，
/// 既省额度也避免「一句话一条回复」的刷屏。命令不参与合并。
pub const MESSAGE_MERGE_MS: u64 = 600;
/// 一轮最多合并的消息条数（防止刷屏把窗口无限延长）。
pub const MESSAGE_MERGE_MAX: usize = 20;
/// 草稿更新间隔（毫秒）。
pub const STREAM_INTERVAL_MS: u64 = 200;
/// 分块回放的总时长上限（秒）：超过就自动放大块、快速补完。
pub const STREAM_MAX_SECONDS: u64 = 6;
/// 网关真流式的总超时（毫秒）。
pub const STREAM_TIMEOUT_MS: u64 = 120_000;
/// 两次增量之间最多等多久（毫秒）；超时视为模型卡住。
pub const STREAM_IDLE_TIMEOUT_MS: u64 = 30_000;
/// 网关流轮询间隔（毫秒）。
pub const STREAM_POLL_MS: u64 = 50;
/// 是否给草稿挂「停止生成」按钮。
pub const STREAM_CAN_STOP: bool = true;
/// 富草稿里思考块（`inputPageBlockThinking`）的文案。
pub const THINKING_TEXT: &str = "Thinking…";

/// TDLib 发送 / 编辑等应答超时（毫秒）。
pub const SEND_TIMEOUT_MS: u64 = 30_000;
/// TDLib 话题 / 身份等辅助调用超时（毫秒）。
pub const ACTION_TIMEOUT_MS: u64 = 15_000;

/// 单条回复字符上限（Telegram 单条 4096，留余量）。
pub const MAX_REPLY_CHARS: usize = 3_500;
/// 引用正文 / 用户选中片段的字符上限。
pub const QUOTE_MAX_CHARS: usize = 800;

/// 话题标题长度上限。
pub const TOPIC_TITLE_MAX_CHARS: usize = 24;
/// 话题图标颜色（Telegram 只认这六个值之一）。
pub const TOPIC_ICON_COLOR: i64 = 7_321_584;

/// 同一条消息的去重窗口（秒）。
pub const DEDUPE_WINDOW_SECS: u64 = 30;
/// 单个会话里 bot 之间最多连续往返轮数。
pub const BOT_MAX_DEPTH: u32 = 2;
/// 同一对端 bot 的最小回复间隔（秒）。
pub const BOT_RATE_LIMIT_SECS: u64 = 5;

/// 首次失败后的重试次数（0 = 不重试；总共尝试 `RETRY_TIMES + 1` 次）。
pub const RETRY_TIMES: u32 = 2;
/// 两次重试之间的等待（毫秒）。
pub const RETRY_DELAY_MS: u64 = 800;

/// 循环模式默认最多迭代轮数（API `run` 不传 max_turns 时）。
pub const LOOP_MAX_TURNS: u32 = 3;

/// 单会话保留的消息条数上限：只是落盘体积的最后一道闸，超出从最旧的丢。
/// 想长期记住更早的内容用 `/agent compress`。
pub const SESSION_MAX_MESSAGES: usize = 2_000;
/// 会话文件数上限（修剪用）。
pub const SESSION_MAX_SESSIONS: usize = 500;

/// 压缩摘要的最大字符数（注入提示词时不再无限放大）。
pub const SUMMARY_MAX_CHARS: usize = 4_000;
/// 压缩时单条消息进入转录的最大字符数。
pub const COMPACT_MESSAGE_MAX_CHARS: usize = 4_000;
/// 压缩转录的总字符上限；超出只保留最近的部分。
pub const COMPACT_TRANSCRIPT_MAX_CHARS: usize = 120_000;
/// 压缩请求的 max_tokens。
pub const COMPACT_MAX_TOKENS: u32 = 2_048;
/// 压缩请求的 temperature（总结要稳，不要发散）。
pub const COMPACT_TEMPERATURE: f64 = 0.3;
