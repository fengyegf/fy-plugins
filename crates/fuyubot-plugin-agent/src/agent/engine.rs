//! 一轮推理的编排：会话历史 → 提示词 → 网关 → 落历史 / 循环迭代。
//!
//! 这里是唯一"组合"的地方：网关不管历史，会话库不管提示词。两个取舍值得记下来：
//!
//! 1. **失败不落历史**：网关报错（没装、没 key、上游 4xx）时整轮丢弃，user 消息也
//!    不留。否则下一次请求会看到一条"用户说了但没人回答"的孤立消息，模型容易把那句
//!    当成还没处理完的指令重复回答。
//! 2. **工具调用是一个内循环**：模型点了工具就同步执行（宿主 `call_tool_result`），
//!    把结果回填上下文再问一次；`max_tool_rounds` 用尽后这一轮不再下发工具，逼它收口。
//! 3. **循环模式靠文本收敛**：工具用完后仍只把上一轮 assistant 输出放回上下文，
//!    再追加一条"继续推进 / 完成就回 DONE"的指令，最多 `max_turns` 轮；assistant 文本
//!    以 DONE 结尾即停。这个约定是跨协议都成立的收敛信号。
//!
//! 网关调用是同步阻塞的，只能在 worker 线程 / 工具处理器里跑。

use std::sync::Arc;
use std::time::{Duration, Instant};

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

use crate::core::{
    AgentError, AgentResult, ChatKey, ChatMessage, Config, RunMode, TurnOutcome, TurnRequest,
};
use crate::session::SessionStore;

use super::toolcall::{assistant_entries, result_entries};
use super::tools::ToolCatalog;
use super::{Completion, GatewayClient, PromptBuilder};

/// 循环模式每轮追加的推进指令。
const LOOP_NUDGE: &str = "继续推进；如果任务已完成，只回复 DONE。";
/// 循环收敛标记。
const DONE_MARKER: &str = "DONE";
/// 会话标题取首条用户消息的前 N 个字符。
const TITLE_CHARS: usize = 40;

/// Agent 运行时：把"一轮对话"翻译成网关请求并维护历史。
pub struct AgentEngine {
    host: HostApi,
    config: Arc<Config>,
    gateway: GatewayClient,
}

/// 一次流式推理的结果。
#[derive(Debug, Clone)]
pub struct StreamOutcome {
    /// 已收到的文本（可能为空 / 部分）。
    pub reply: String,
    /// 会话 id。
    pub session: String,
    /// 是否被用户 / 网关取消。
    pub cancelled: bool,
    /// 中途错误（有文本时也返回，调用方决定照不照发）。
    pub error: Option<String>,
    pub model: Option<String>,
    pub usage: Value,
}

/// 一次 run 的中间结果。
struct TurnDraft {
    reply: String,
    turns: u32,
    /// 这一轮实际执行了几次工具调用。
    tool_calls: u32,
    usage: Value,
    model: Option<String>,
}

impl AgentEngine {
    /// 建引擎。
    pub fn new(host: HostApi, config: Arc<Config>) -> Self {
        let gateway = GatewayClient::new(host, &config);
        Self {
            host,
            config,
            gateway,
        }
    }

    /// 配置。
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 网关。
    pub fn gateway(&self) -> &GatewayClient {
        &self.gateway
    }

    /// 宿主。
    pub fn host(&self) -> HostApi {
        self.host
    }

    /// 现查宿主工具表；关掉 `model_tools` 或宿主没有工具时返回 `None`。
    ///
    /// 不缓存：宿主工具表随插件热插拔变化，缓存住名字会让模型拿着已卸载的工具
    /// 去调（宿主回 `NoHandler`）。
    fn tool_catalog(&self) -> Option<ToolCatalog> {
        if !self.config.model_tools {
            return None;
        }
        let catalog = ToolCatalog::load(
            &self.host,
            &self.config.tool_denylist,
            self.config.tool_result_max_chars,
        );
        (!catalog.is_empty()).then_some(catalog)
    }

    /// 这一轮会不会给模型下发工具（worker 用它决定能不能走网关真流式）。
    pub fn tools_active(&self) -> bool {
        self.tool_catalog().is_some()
    }

    /// 本次下发的工具数（describe / 诊断用）。
    pub fn tool_count(&self) -> usize {
        self.tool_catalog()
            .map(|catalog| catalog.len())
            .unwrap_or(0)
    }

    /// 本次下发的工具名（describe / 诊断用）。
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_catalog()
            .map(|catalog| catalog.names().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// 系统提示：自定义优先；始终按 `format_guide` 附加富消息语法说明。
    fn prompt_builder(&self, custom: Option<&str>) -> PromptBuilder {
        match custom.map(str::trim).filter(|prompt| !prompt.is_empty()) {
            Some(prompt) => PromptBuilder {
                system: super::prompt::with_guide(prompt, self.config.format_guide),
                max_history: self.config.max_history,
            },
            None => PromptBuilder::from_config(&self.config),
        }
    }

    /// 跑一轮（single 一次调用；loop 多轮迭代）。
    ///
    /// 成功后才落历史（见模块注释第 1 条）；会话标题取首条用户消息前 40 个字符。
    pub fn run(
        &self,
        store: &SessionStore,
        request: &TurnRequest,
        mode: RunMode,
        max_turns: u32,
    ) -> AgentResult<TurnOutcome> {
        let user_text = request.user_text.trim();
        if user_text.is_empty() {
            return Err(AgentError::BadRequest("用户消息为空".to_owned()));
        }

        // 指定了会话 id 就用它（不存在算参数错误），否则按 chat + topic 归。
        let key = resolve_key(store, request)?;
        let session = match store.load(&key)? {
            Some(session) => session,
            None => store.ensure(&key, &session_title(user_text))?,
        };

        let builder = self.prompt_builder(request.system_prompt.as_deref());
        let messages = builder.messages(&session.messages, user_text);
        let model = request.model.as_deref();
        let max_tokens = request.max_tokens.or(self.config.max_tokens);
        let temperature = self.config.temperature;
        let catalog = self.tool_catalog();

        let draft = match mode {
            RunMode::Single => {
                let (completion, tool_calls) = self.complete_with_tools(
                    catalog.as_ref(),
                    messages,
                    model,
                    max_tokens,
                    temperature,
                )?;
                let reply = completion.content.trim().to_owned();
                if reply.is_empty() {
                    return Err(empty_completion(&completion));
                }
                TurnDraft {
                    reply,
                    turns: 1,
                    tool_calls,
                    usage: completion.usage,
                    model: completion.model,
                }
            }
            RunMode::Loop => self.run_loop(
                messages,
                model,
                max_tokens,
                temperature,
                max_turns,
                catalog.as_ref(),
            )?,
        };

        // 只有成功才落历史：失败的轮次整轮丢弃（见模块注释第 1 条）。
        let now = crate::unix_now();
        store.append(
            &key,
            &[
                ChatMessage::user(user_text, now),
                ChatMessage::assistant(draft.reply.as_str(), now),
            ],
        )?;

        Ok(TurnOutcome {
            reply: draft.reply,
            session: session.id,
            model: draft
                .model
                .or_else(|| configured_model(model, &self.config)),
            turns: draft.turns,
            tool_calls: draft.tool_calls,
            usage: draft.usage,
        })
    }

    /// 无会话的直通完成（API `chat` 用）：不读也不写会话历史，但**照常带工具**。
    pub fn complete_messages(
        &self,
        messages: Vec<Value>,
        model: Option<&str>,
        max_tokens: Option<u32>,
    ) -> AgentResult<Completion> {
        let catalog = self.tool_catalog();
        let (completion, _) = self.complete_with_tools(
            catalog.as_ref(),
            messages,
            model,
            max_tokens.or(self.config.max_tokens),
            self.config.temperature,
        )?;
        Ok(completion)
    }

    /// 带重试的同步调用：首轮 + retry_times 次重试（配置类错误不重试，重试也没用）。
    fn chat_with_retry(
        &self,
        messages: Vec<Value>,
        model: Option<&str>,
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        tools: Option<&Value>,
    ) -> AgentResult<Completion> {
        let mut attempt = 0u32;
        loop {
            match self
                .gateway
                .chat(messages.clone(), model, max_tokens, temperature, tools)
            {
                Ok(completion) => return Ok(completion),
                Err(error) => {
                    if attempt >= self.config.retry_times || !retryable(&error) {
                        return Err(error);
                    }
                    attempt += 1;
                    self.host.info(&format!(
                        "agent: 推理失败，第 {attempt}/{} 次重试：{error}",
                        self.config.retry_times
                    ));
                    std::thread::sleep(Duration::from_millis(self.config.retry_delay_ms));
                }
            }
        }
    }

    /// 带工具的一轮完成：模型要调工具就执行、回填，再问一次，直到它给出文本。
    ///
    /// 返回（最终完成结果, 实际执行的工具调用次数）。`max_tool_rounds` 用尽后
    /// **不再下发工具**再问一次：模型没有了工具，只能把结论写成文本。
    fn complete_with_tools(
        &self,
        catalog: Option<&ToolCatalog>,
        messages: Vec<Value>,
        model: Option<&str>,
        max_tokens: Option<u32>,
        temperature: Option<f64>,
    ) -> AgentResult<(Completion, u32)> {
        let protocol = self.gateway.protocol();
        let definitions = catalog.and_then(|catalog| catalog.definitions(protocol));
        if definitions.is_none() {
            let completion =
                self.chat_with_retry(messages, model, max_tokens, temperature, None)?;
            return Ok((completion, 0));
        }
        let limit = self.config.max_tool_rounds.max(1);
        let mut messages = messages;
        let mut used = 0u32;
        loop {
            let tools = if used < limit {
                definitions.as_ref()
            } else {
                None
            };
            let completion =
                self.chat_with_retry(messages.clone(), model, max_tokens, temperature, tools)?;
            if completion.tool_calls.is_empty() {
                return Ok((completion, used));
            }
            let Some(catalog) = catalog else {
                // 没下发工具却收到工具调用：协议错配或上游缓存，原样交回调用方。
                return Ok((completion, used));
            };
            if used >= limit {
                self.host.warn(&format!(
                    "agent: 工具轮次已达上限 {limit}，仍有 {} 个调用未执行",
                    completion.tool_calls.len()
                ));
                return Ok((completion, used));
            }
            used += 1;
            messages.extend(assistant_entries(
                protocol,
                &completion.content,
                &completion.tool_calls,
            ));
            let mut outputs = Vec::with_capacity(completion.tool_calls.len());
            for call in &completion.tool_calls {
                let output = catalog.call(&call.name, &call.arguments);
                self.host.info(&format!(
                    "agent: 调用工具 {}（回填 {} 字节）",
                    call.name,
                    output.len()
                ));
                outputs.push(output);
            }
            messages.extend(result_entries(protocol, &completion.tool_calls, &outputs));
        }
    }

    /// 流式跑一轮：网关支持 stream 时走真 token 流，每块文本回调一次 sink。
    ///
    /// - on_connected：网关流**连通后**回调（占位符放这里，而不是收到消息就发）；
    /// - sink 返回 false 表示调用方要求停止：取消网关流并带 cancelled = true 返回；
    /// - 失败按 retry_times 重试：开流失败、或还没收到任何文本就出错时整轮重试；
    ///   已经有部分文本后出错不再重试（避免重复回答），把已收到的部分交给调用方。
    pub fn run_stream(
        &self,
        store: &SessionStore,
        request: &TurnRequest,
        on_connected: &mut dyn FnMut(),
        sink: &mut dyn FnMut(&str) -> bool,
    ) -> AgentResult<StreamOutcome> {
        let user_text = request.user_text.trim();
        if user_text.is_empty() {
            return Err(AgentError::BadRequest("用户消息为空".to_owned()));
        }
        // 带工具的回合不能走网关真流式：流式通道只透传文本增量，模型要调工具时
        // 那一轮没有任何文本（调用块被丢弃），等下去只会超时。退回「同步跑完 +
        // 分块回放」，注意这里没有连接可等，占位符一开始就发。
        if self.tools_active() {
            return self.replay_stream(store, request, on_connected, sink);
        }

        let key = resolve_key(store, request)?;
        let session = match store.load(&key)? {
            Some(session) => session,
            None => store.ensure(&key, &session_title(user_text))?,
        };
        let builder = self.prompt_builder(request.system_prompt.as_deref());
        let messages = builder.messages(&session.messages, user_text);
        let model = request.model.as_deref();
        let max_tokens = request.max_tokens.or(self.config.max_tokens);
        let body = self
            .gateway
            // 走到这里说明这一轮不带工具（带工具的分支在函数开头就回放了）。
            .chat_body(messages, model, max_tokens, self.config.temperature, None);
        let total = Duration::from_millis(self.config.stream_timeout_ms.max(1_000));
        let idle = Duration::from_millis(self.config.stream_idle_timeout_ms.max(1_000));
        let poll = Duration::from_millis(self.config.stream_poll_ms.max(10));
        let mut attempt = 0u32;

        loop {
            let stream_id = match self.gateway.stream_chat(body.clone()) {
                Ok(id) => id,
                Err(error) => {
                    if attempt < self.config.retry_times && retryable(&error) {
                        attempt += 1;
                        self.host.info(&format!(
                            "agent: 开流失败，第 {attempt}/{} 次重试：{error}",
                            self.config.retry_times
                        ));
                        std::thread::sleep(Duration::from_millis(self.config.retry_delay_ms));
                        continue;
                    }
                    return Err(error);
                }
            };
            // 连通之后才回调：占位符在这里发，而不是收到消息就发。
            on_connected();

            let deadline = Instant::now() + total;
            let mut last_progress = Instant::now();
            let mut reply = String::new();
            let mut model_used = None;
            let mut usage = Value::Null;
            let mut cursor = 0u64;
            let mut cancelled = false;
            let mut error: Option<String> = None;

            loop {
                if Instant::now() >= deadline {
                    let _ = self.gateway.cancel_stream(stream_id);
                    error = Some("流式总超时".to_owned());
                    break;
                }
                if last_progress.elapsed() >= idle {
                    let _ = self.gateway.cancel_stream(stream_id);
                    error = Some("流式空闲超时".to_owned());
                    break;
                }
                match self.gateway.poll_stream(stream_id, cursor) {
                    Ok(polled) => {
                        cursor = polled.cursor;
                        let mut stop = false;
                        for chunk in &polled.chunks {
                            reply.push_str(chunk);
                            last_progress = Instant::now();
                            if !sink(chunk) {
                                stop = true;
                                break;
                            }
                        }
                        if stop {
                            let _ = self.gateway.cancel_stream(stream_id);
                            cancelled = true;
                            break;
                        }
                        if let Some(reason) = polled.error {
                            error = Some(reason);
                            break;
                        }
                        if polled.cancelled {
                            cancelled = true;
                            break;
                        }
                        if polled.done {
                            model_used = polled.model;
                            usage = polled.usage;
                            break;
                        }
                    }
                    Err(failure) => {
                        error = Some(failure.to_string());
                        break;
                    }
                }
                std::thread::sleep(poll);
            }
            if cancelled {
                let _ = self.gateway.cancel_stream(stream_id);
                return Ok(StreamOutcome {
                    reply,
                    session: session.id,
                    cancelled: true,
                    error: None,
                    model: model_used.or_else(|| configured_model(model, &self.config)),
                    usage,
                });
            }
            if let Some(reason) = error {
                // 还没吐字就失败：整轮重试；已经有内容就不再重试（避免重复回答）。
                if reply.trim().is_empty() && attempt < self.config.retry_times {
                    attempt += 1;
                    self.host.info(&format!(
                        "agent: 流式中断且没有内容，第 {attempt}/{} 次重试：{reason}",
                        self.config.retry_times
                    ));
                    std::thread::sleep(Duration::from_millis(self.config.retry_delay_ms));
                    continue;
                }
                return Ok(StreamOutcome {
                    reply,
                    session: session.id,
                    cancelled: false,
                    error: Some(reason),
                    model: model_used.or_else(|| configured_model(model, &self.config)),
                    usage,
                });
            }

            // 只有正常结束且非空才落历史。
            let now = crate::unix_now();
            store.append(
                &key,
                &[
                    ChatMessage::user(user_text, now),
                    ChatMessage::assistant(reply.as_str(), now),
                ],
            )?;
            return Ok(StreamOutcome {
                reply,
                session: session.id,
                cancelled: false,
                error: None,
                model: model_used.or_else(|| configured_model(model, &self.config)),
                usage,
            });
        }
    }

    /// 循环模式：最多 `max_turns` 轮，靠最后一段文本的 DONE 标记收敛。
    fn run_loop(
        &self,
        mut messages: Vec<Value>,
        model: Option<&str>,
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        max_turns: u32,
        catalog: Option<&ToolCatalog>,
    ) -> AgentResult<TurnDraft> {
        let limit = max_turns.max(1);
        let mut turns = 0u32;
        let mut tool_calls = 0u32;
        let mut reply = String::new();
        let mut raw_tail = String::new();
        let mut usage = Value::Null;
        let mut model_used = None;

        while turns < limit {
            let (completion, used) = self.complete_with_tools(
                catalog,
                messages.clone(),
                model,
                max_tokens,
                temperature,
            )?;
            turns += 1;
            tool_calls += used;
            let text = completion.content.trim().to_owned();
            // 多轮只保留最后一轮的用量 / 模型：对调用方来说累计量意义不大，
            // 而每轮的上游模型一般相同。
            usage = completion.usage;
            model_used = completion.model;
            if !text.is_empty() {
                raw_tail = text.clone();
                let cleaned = strip_done(&text);
                if !cleaned.is_empty() {
                    reply = cleaned;
                }
            }
            // 收敛：模型自己说完成了，或者轮次用光。
            if is_done(&text) || turns >= limit {
                break;
            }
            // 没完成：把这一轮输出放回上下文，再推一句"继续"。
            messages.push(json!({ "role": "assistant", "content": text }));
            messages.push(json!({ "role": "user", "content": LOOP_NUDGE }));
        }

        if reply.is_empty() {
            // 模型全程只回 DONE（或空串）：把原文给用户，总比发空消息强。
            reply = raw_tail;
        }
        if reply.is_empty() {
            return Err(AgentError::Gateway(format!(
                "网关 {turns} 轮都没有返回文本"
            )));
        }
        Ok(TurnDraft {
            reply,
            turns,
            tool_calls,
            usage,
            model: model_used,
        })
    }

    /// 工具回合的流式兜底：同步跑完一轮（含工具循环），再把终稿按块喂给 `sink`。
    ///
    /// 效果与 worker 里「网关不支持 stream」时的回放一致；区别只是这里没有
    /// 「连接」可等，占位符在开始前就发。
    fn replay_stream(
        &self,
        store: &SessionStore,
        request: &TurnRequest,
        on_connected: &mut dyn FnMut(),
        sink: &mut dyn FnMut(&str) -> bool,
    ) -> AgentResult<StreamOutcome> {
        on_connected();
        let outcome = self.run(store, request, RunMode::Single, 1)?;
        let mut reply = String::new();
        for chunk in crate::core::chunk_text(&outcome.reply, self.config.stream_chunk_chars.max(1))
        {
            reply.push_str(&chunk);
            if !sink(&chunk) {
                return Ok(StreamOutcome {
                    reply,
                    session: outcome.session,
                    cancelled: true,
                    error: None,
                    model: outcome.model,
                    usage: outcome.usage,
                });
            }
        }
        Ok(StreamOutcome {
            reply: outcome.reply,
            session: outcome.session,
            cancelled: false,
            error: None,
            model: outcome.model,
            usage: outcome.usage,
        })
    }
}

/// 会话定位：`session_id` 优先，其次 `ChatKey`。
fn resolve_key(store: &SessionStore, request: &TurnRequest) -> AgentResult<ChatKey> {
    match request
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => match store.find(id)? {
            Some(session) => Ok(session.key()),
            None => Err(AgentError::BadRequest(format!("会话 {id} 不存在"))),
        },
        None => Ok(request.key),
    }
}

/// 会话标题：首条用户消息压平空白后取前 `TITLE_CHARS` 个字符。
fn session_title(user_text: &str) -> String {
    let plain = user_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = plain.chars().take(TITLE_CHARS).collect();
    let title = title.trim();
    if title.is_empty() {
        "AI 会话".to_owned()
    } else {
        title.to_owned()
    }
}

/// 下发的模型：入参 → 配置 → 都不给就交给网关默认（返回 `None`）。
fn configured_model(requested: Option<&str>, config: &Config) -> Option<String> {
    requested
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let model = config.model.trim();
            if model.is_empty() {
                None
            } else {
                Some(model.to_owned())
            }
        })
}

/// 空文本完成：错误里带上结束原因，便于区分"被长度截断"和"模型什么都没说"。
fn empty_completion(completion: &Completion) -> AgentError {
    AgentError::Gateway(format!(
        "网关返回空文本（finish_reason={}）",
        completion.finish_reason.as_deref().unwrap_or("未知")
    ))
}

/// 是否值得重试：参数错误、配置错误重试也没用，直接失败。
fn retryable(error: &AgentError) -> bool {
    !matches!(error, AgentError::BadRequest(_)) && !error.to_string().contains("配置错误")
}

/// 文本末尾是否有 DONE 收敛标记（大小写不敏感，且要求词边界）。
fn is_done(text: &str) -> bool {
    let trimmed = text.trim();
    let upper = trimmed.to_ascii_uppercase();
    if upper == DONE_MARKER {
        return true;
    }
    if !upper.ends_with(DONE_MARKER) {
        return false;
    }
    // `UNDONE` 这种不算：DONE 前面必须是空白 / 标点。
    let head = &upper[..upper.len() - DONE_MARKER.len()];
    head.chars()
        .next_back()
        .is_some_and(|last| !last.is_ascii_alphanumeric())
}

/// 去掉末尾的 DONE 标记以及紧跟的句末标点。
fn strip_done(text: &str) -> String {
    let trimmed = text.trim();
    // 先去掉尾部标点，再判断 DONE：模型经常写成「结论 DONE。」
    let without_punct = trimmed.trim_end_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '。' | '.' | '！' | '!' | '？' | '?' | '，' | ',' | '；' | ';'
            )
    });
    if !is_done(without_punct) {
        return trimmed.to_owned();
    }
    let cut = without_punct.len() - DONE_MARKER.len();
    if cut == 0 {
        return String::new();
    }
    without_punct[..cut]
        .trim_end_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '。' | '.' | '！' | '!' | '？' | '?' | '，' | ',' | '；' | ';'
                )
        })
        .trim()
        .to_owned()
}
