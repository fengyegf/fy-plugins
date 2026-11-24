//! 订阅回调 + 工作线程。
//!
//! 宿主回调只做一件事：把 `updateNewMessage` 的 JSON 丢进 channel。
//! 解析、准入、推理、发送全部在 `agent-worker` 线程里做——那些都是**同步阻塞**的
//! TDLib / HTTP 调用，放在回调里会卡死接收循环。
//! 正常的连续消息按会话合并（见 consts::MESSAGE_MERGE_MS）：连发几句只产生一次请求、一条回复。

use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fuyubot_plugin_api::{HostApi, HostExt, LogLevel, parse_payload};
use serde_json::Value;

use crate::PluginState;
use crate::agent::GuardVerdict;
use crate::core::{
    AgentError, AgentResult, ChatKey, ChatKind, IncomingMessage, QuoteAuthor, QuotedMessage,
    RunMode, TurnRequest, chunk_text, consts, math_open, normalize_math, strip_markdown,
};
use crate::td::SentMessage;

/// 空闲轮询间隔（用来观察停机标志）。
const IDLE_TICK: Duration = Duration::from_millis(200);
/// 业务线程栈：推理 / 发送都是同步调用，给足 2 MiB。
const TASK_STACK: usize = 2 * 1024 * 1024;

/// 工作线程组：一个路由线程。
pub struct Worker {
    /// 必须活着：它就是回调拿到的那个指针的目标。
    sender: Box<mpsc::Sender<Value>>,
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

/// 裸指针 + Send：状态由插件主体持有，Worker 一定先于状态析构。
struct StatePtr(*const PluginState);
// SAFETY: PluginState 只在 `AgentPlugin` 里被 Box 持有，stop() 的顺序保证
// "先停 Worker、后放 state"；线程里对它的访问都限制在 worker 生命周期内。
unsafe impl Send for StatePtr {}

impl Worker {
    /// 起路由线程。
    pub fn start(host: HostApi, state: *const PluginState) -> Result<Self, String> {
        if state.is_null() {
            return Err("agent: 状态指针为空".to_owned());
        }
        let (sender, receiver) = mpsc::channel::<Value>();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let state = StatePtr(state);
        let thread = std::thread::Builder::new()
            .name("agent-worker".to_owned())
            .stack_size(TASK_STACK)
            .spawn(move || route_loop(host, state, receiver, thread_stop))
            .map_err(|error| format!("agent: 启动 worker 线程失败：{error}"))?;
        Ok(Self {
            sender: Box::new(sender),
            thread: Some(thread),
            stop,
        })
    }

    /// 传给 `subscribe_json` 的 user_data：指向 channel 发送端。
    pub fn sender_ptr(&self) -> *mut c_void {
        (&*self.sender as *const mpsc::Sender<Value>)
            .cast_mut()
            .cast::<c_void>()
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 宿主回调：只投递，不做任何阻塞操作。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C-unwind" fn on_update(user: *mut c_void, payload: *const c_char) -> i32 {
    if user.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: 宿主按 ABI 契约传临时 NUL 结尾字符串；user 是 sender_ptr 给出的发送端。
    let update = unsafe { parse_payload(payload) };
    let sender = unsafe { &*(user as *const mpsc::Sender<Value>) };
    if sender.send(update).is_err() {
        return -1;
    }
    0
}

/// `updateStopMessageDraft` 回调：用户按了草稿上的「停止生成」。
///
/// 只写一个内存标记（拿锁 + 插一条），绝不阻塞：在接收循环上做慢事会拖垮整个宿主。
/// `user` 是 `PluginState` 指针，生命周期覆盖整个订阅期。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C-unwind" fn on_stop(user: *mut c_void, payload: *const c_char) -> i32 {
    if user.is_null() || payload.is_null() {
        return -1;
    }
    // SAFETY: 宿主按 ABI 契约传临时 NUL 结尾字符串；user 是 PluginState 指针。
    let update = unsafe { parse_payload(payload) };
    let Some(chat_id) = update.get("chat_id").and_then(Value::as_i64) else {
        return -1;
    };
    let forum_topic_id = update
        .get("forum_topic_id")
        .and_then(Value::as_i64)
        .unwrap_or(0) as i32;
    let Some(draft_id) = update.get("draft_id").and_then(Value::as_i64) else {
        return -1;
    };
    // SAFETY: 见函数文档；PluginState 由插件主体持有，订阅先于状态析构。
    let state = unsafe { &*(user as *const PluginState) };
    state.record_stop(chat_id, forum_topic_id, draft_id);
    0
}

/// 路由线程主循环。
fn route_loop(
    host: HostApi,
    state: StatePtr,
    receiver: mpsc::Receiver<Value>,
    stop: Arc<AtomicBool>,
) {
    // 批处理期间遇到命令 / 非聊天更新：留到下一轮立即处理，顺序不乱。
    let mut backlog: VecDeque<Value> = VecDeque::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let first = match backlog.pop_front() {
            Some(update) => update,
            None => match receiver.recv_timeout(IDLE_TICK) {
                Ok(update) => update,
                Err(RecvTimeoutError::Timeout) => {
                    // SAFETY: Worker 保证状态在 join 之前一直有效。
                    let state = unsafe { &*state.0 };
                    state.maybe_prune_sessions(&host);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
        };
        // SAFETY: Worker 保证状态在 join 之前一直有效。
        let state = unsafe { &*state.0 };
        if !should_merge(&first) {
            handle_update(&host, state, first);
            continue;
        }
        let batch = collect_batch(&receiver, first, &mut backlog);
        handle_batch(&host, state, batch);
    }
}

/// 这条更新要不要参与「连续消息合并」。
///
/// 只合并正常的聊天消息；命令要立刻执行，outgoing / 服务消息走单条链路
///（会在 handle_update 里被丢掉）。
fn should_merge(update: &Value) -> bool {
    let Some(message) = crate::td::TdClient::parse_incoming(update) else {
        return false;
    };
    !message.is_outgoing
        && crate::core::is_chat_content(&message.content_type)
        && message.command().is_none()
}

/// 在合并窗口里边收边等：每来一条就顺延窗口，直到静默 / 达到上限 / 遇到不该合并的更新。
///
/// 不该合并的更新（命令 / 服务消息 / outgoing）放进 backlog，当前批处理完立刻处理，
/// 不会被合并窗口拖慢，也不会乱序。
fn collect_batch(
    receiver: &mpsc::Receiver<Value>,
    first: Value,
    backlog: &mut VecDeque<Value>,
) -> Vec<Value> {
    let window = Duration::from_millis(consts::MESSAGE_MERGE_MS);
    let mut batch = vec![first];
    let mut deadline = Instant::now() + window;
    while batch.len() < consts::MESSAGE_MERGE_MAX {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match receiver.recv_timeout(deadline - now) {
            Ok(update) => {
                if !should_merge(&update) {
                    backlog.push_back(update);
                    break;
                }
                batch.push(update);
                // 用户还在连续打字就继续等：窗口从最后一条重新计时。
                deadline = Instant::now() + window;
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    batch
}

/// 一条更新的完整处理。
fn handle_update(host: &HostApi, state: &PluginState, update: Value) {
    let Some(message) = crate::td::TdClient::parse_incoming(&update) else {
        return;
    };
    if message.is_outgoing {
        return;
    }

    // 服务消息（话题创建 / 入群 / 置顶…）不是聊天内容：不推理、不建会话。
    // 真机上踩过：话题创建事件被当成用户消息，模型一本正经地分析
    // `[messageForumTopicCreated]`，还占掉了紧接着的用户消息。
    if !crate::core::is_chat_content(&message.content_type) {
        host.log(
            LogLevel::Debug,
            &format!(
                "agent: 跳过非聊天内容 {}（消息 {}，话题 {:?}）",
                message.content_type, message.message_id, message.topic_id
            ),
        );
        return;
    }

    // 1. 命令：/agent ...（只管自己的命令，其它命令交给别的插件）。
    if let Some(command) = message.command() {
        if command.name == "agent" && !command.addressed_elsewhere {
            match crate::cmd::dispatch(state, &message) {
                Ok(Some(text)) => {
                    if let Err(error) = reply_text(state, &message, &text) {
                        host.warn(&format!("agent: 命令回复失败：{error}"));
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    host.warn(&format!("agent: 命令处理失败：{error}"));
                    let text = format!("命令执行失败：{error}");
                    let _ = reply_text(state, &message, &text);
                }
            }
            return;
        }
        return;
    }

    handle_message(host, state, message);
}

/// 单条消息的准入 + 推理（批处理里的「最后一条」也走这里）。
fn handle_message(host: &HostApi, state: &PluginState, mut message: IncomingMessage) {
    // 1. 会话种类准入：先看会话类型，再去取引用内容（免得白花一次 TDLib 调用）。
    match message.kind {
        ChatKind::Private => {
            if !state.config.allow_private {
                return;
            }
        }
        ChatKind::Group => {
            if !state.config.allow_groups {
                return;
            }
        }
        ChatKind::Channel => return,
    }

    // 2. 引用上下文：群里「回复我的消息」也算点名，本轮提示词也复用这次结果。
    let quoted = quote_context(host, state, &message);
    if message.kind == ChatKind::Group
        && state.config.group_require_mention
        && !mentions_me(&state.host, &message)
        && !quoted
            .as_ref()
            .is_some_and(|quoted| quoted.author == QuoteAuthor::Me)
    {
        host.log(
            LogLevel::Debug,
            &format!(
                "agent: 群里没有 @ 我、也不是回复我的消息，忽略（消息 {}）",
                message.message_id
            ),
        );
        return;
    }

    // 3. 发送者是不是 bot（Bot-to-Bot 准入）。
    if message.sender_is_bot {
        // 已由上游标记。
    } else if let Some(user_id) = message.sender_user_id {
        match state.td.is_bot(user_id) {
            Ok(is_bot) => message.sender_is_bot = is_bot,
            Err(error) => {
                // 回放没有 TDLib：不当 bot 处理，链路继续。
                host.info(&format!(
                    "agent: 查不到发送者身份（按普通用户处理）：{error}"
                ));
            }
        }
    }
    if message.sender_is_bot {
        if !state.config.allow_bots {
            host.info("agent: 发送者是 bot 且 allow_bots = false，忽略");
            return;
        }
        if !state.config.bot_allowlist.is_empty() {
            let allowed = message
                .sender_user_id
                .and_then(|user_id| state.td.user(user_id).ok().flatten())
                .is_some_and(|user| state.config.bot_allowed(user.username.as_deref()));
            if !allowed {
                host.info("agent: bot 不在白名单，忽略");
                return;
            }
        }
    }

    // 4. 防环（去重 / 限速 / bot 深度）。
    {
        let mut guard = state
            .guard
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let GuardVerdict::Drop(reason) = guard.check(&message, crate::unix_now()) {
            host.info(&format!("agent: 防环丢弃（{reason}）"));
            return;
        }
    }

    // 5. 主链路。
    if let Err(error) = respond(host, state, &message, quoted.as_ref()) {
        host.warn(&format!(
            "agent: 处理消息 {} 失败：{error}",
            message.message_id
        ));
    }
}

/// 一批连续消息：按会话分组，每个会话只回答最后一条，前面的正文并进本轮输入。
///
/// 这是「一次对话」而不是「每个句子发一次请求」的落点：同一窗口里用户连发的几句话
/// 会合成一条 user 消息；模型看到的上下文不丢，但只产生一次调用、一条回复。
fn handle_batch(host: &HostApi, state: &PluginState, updates: Vec<Value>) {
    let mut groups: Vec<(ChatKey, Vec<IncomingMessage>)> = Vec::new();
    for update in updates {
        let Some(message) = crate::td::TdClient::parse_incoming(&update) else {
            continue;
        };
        // should_merge 已筛过；这里再兜一层，防止以后改筛选逻辑时混进非聊天内容。
        if message.is_outgoing
            || !crate::core::is_chat_content(&message.content_type)
            || message.command().is_some()
        {
            continue;
        }
        let key = message.key();
        match groups.iter_mut().find(|(candidate, _)| *candidate == key) {
            Some((_, messages)) => messages.push(message),
            None => groups.push((key, vec![message])),
        }
    }

    for (_, mut messages) in groups {
        let Some(mut last) = messages.pop() else {
            continue;
        };
        // 前面的句子按到达顺序拼在前面，最后一条是「要回答的那句」。
        let merged = merge_texts(
            messages
                .iter()
                .map(IncomingMessage::display_text)
                .chain(std::iter::once(last.display_text())),
        );
        if !messages.is_empty() {
            host.info(&format!(
                "agent: 合并连续 {} 条消息，只回复最后一条（消息 {}）",
                messages.len() + 1,
                last.message_id
            ));
        }
        // 并入正文后按「一次输入」走完整链路：会话落盘、提示词都只产生一条 user 消息。
        last.text = merged;
        handle_message(host, state, last);
    }
}

/// 合并一批正文：跳过空串，按到达顺序用换行连接。
fn merge_texts(texts: impl IntoIterator<Item = String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for text in texts {
        let text = text.trim();
        if !text.is_empty() {
            parts.push(text.to_owned());
        }
    }
    parts.join("\n")
}

/// 推理并回复。
fn respond(
    host: &HostApi,
    state: &PluginState,
    message: &IncomingMessage,
    quoted: Option<&QuotedMessage>,
) -> AgentResult<()> {
    let config = &state.config;

    // 私聊首条消息：需要的话开一个话题，这个会话以后就挂在话题里。
    let mut topic_id = message.topic_id;
    let mut created_topic = false;
    if topic_id.is_none() && message.is_private() && config.topic_auto_create {
        let title = topic_title(&message.display_text());
        match state.td.create_topic(
            message.chat_id,
            &title,
            consts::TOPIC_ICON_COLOR,
            consts::ACTION_TIMEOUT_MS,
        ) {
            Ok(topic) => {
                host.info(&format!(
                    "agent: 已开新会话话题 #{} {}",
                    topic.topic_id, title
                ));
                topic_id = Some(topic.topic_id);
                created_topic = true;
            }
            Err(error) => {
                host.warn(&format!("agent: 开话题失败，退回普通私聊：{error}"));
            }
        }
    }
    let key = ChatKey::new(message.chat_id, topic_id);

    // 这个话题里有没有历史会话：没有 = 新对话（不管话题是我们建的还是用户自己建的）。
    let session_existed = matches!(state.store.load(&key), Ok(Some(_)));

    // 首次聊天必须设置标题：用首条消息内容，不额外调用模型（自动建的话题已经是这个标题）。
    if !session_existed {
        set_topic_from_first_message(host, state, message, topic_id, created_topic);
    }

    let request = TurnRequest {
        key,
        user_text: prompt_text(message, quoted),
        session_id: None,
        system_prompt: None,
        model: None,
        max_tokens: None,
    };

    // 官方式流式：私聊 + 网关提供 stream → 真 token 流（首字延迟真正下降）；
    // 否则退回「同步推理 + 拿到终稿后分块回放」。
    let gateway_stream =
        config.stream_replies && message.is_private() && state.engine.gateway().supports_stream();

    let finished = if gateway_stream {
        stream_from_gateway(host, state, message, topic_id, &request)?
    } else {
        let (text, session_id) = match state.engine.run(
            &state.store,
            &request,
            RunMode::Single,
            consts::LOOP_MAX_TURNS,
        ) {
            Ok(outcome) => (outcome.reply, outcome.session),
            Err(error) => {
                host.warn(&format!("agent: 推理失败：{error}"));
                (failure_text(&error.to_string()), key.file_stem())
            }
        };
        // 私有聊天走草稿消息（Thinking 占位 → 分块 → 正式消息），群聊直接发终稿。
        // 两种路径都不引用用户消息——官方演示里没有回复引用。
        stream_reply(host, state, message, topic_id, &text)?;
        Some((session_id, text.chars().count()))
    };
    let Some((session_id, reply_len)) = finished else {
        return Ok(());
    };

    // 记录一次 bot 往返，供频率 / 深度收口（人类消息不记账）。
    if message.sender_is_bot
        && let Some(peer) = message.sender_user_id
    {
        state
            .guard
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .note_bot_turn(peer, crate::unix_now());
    }

    host.info(&format!(
        "agent: 会话 {} 已回复（{} 字）",
        session_id, reply_len
    ));
    Ok(())
}

/// 取引用上下文：用户点了「回复」时，把被引用那条消息从 TDLib 取回来。
///
/// 取到的结果有两处用途：群里「回复我的消息」也算点名（`group_require_mention`），
/// 以及本轮提示词里的引用块。取不到（已删除 / 回放 / 无权限）返回 `None`。
fn quote_context(
    host: &HostApi,
    state: &PluginState,
    message: &IncomingMessage,
) -> Option<QuotedMessage> {
    let reply = message.reply.as_ref()?;
    // 引用上下文始终取：群里的「回复我 = 点名」判断也依赖它。
    match state.td.quoted_message(message) {
        Ok(quoted) => {
            match &quoted {
                Some(quoted) => host.log(
                    LogLevel::Debug,
                    &format!(
                        "agent: 引用 #{}（{}，{} 字）",
                        quoted.message_id,
                        quoted.content_type,
                        quoted.text.chars().count()
                    ),
                ),
                None => host.log(
                    LogLevel::Debug,
                    &format!("agent: 引用 #{} 取不到正文，只留标记", reply.message_id),
                ),
            }
            quoted
        }
        Err(error) => {
            host.log(
                LogLevel::Debug,
                &format!("agent: 取被引用消息失败（只留引用标记）：{error}"),
            );
            None
        }
    }
}

/// 本轮交给模型的用户输入：用户点了「回复」时，在最前面拼一段被引用消息的上下文块。
///
/// 为什么必须拼：更新里只有被引用消息的 id（外加手动选中片段），模型看不到那条消息，
/// 面对「这个呢？」只能瞎猜。取不到正文时也保留引用标记——让模型知道「他在回复某条
/// 消息」，比装作没看见好。
fn prompt_text(message: &IncomingMessage, quoted: Option<&QuotedMessage>) -> String {
    let text = message.display_text();
    let Some(reply) = message.reply.as_ref() else {
        return text;
    };
    let block = crate::core::render_quote_block(reply, quoted, consts::QUOTE_MAX_CHARS);
    format!("{block}{text}")
}

/// 网关真流式：空草稿占位 → 每块增量更新草稿 → 正式消息落地。
///
/// 返回 `Some((会话 id, 回复字数))`；用户中途停止返回 `None`（不发终稿）。
fn stream_from_gateway(
    host: &HostApi,
    state: &PluginState,
    message: &IncomingMessage,
    topic_id: Option<i32>,
    request: &TurnRequest,
) -> AgentResult<Option<(String, usize)>> {
    let draft = crate::td::DraftSend {
        chat_id: message.chat_id,
        topic_id,
        draft_id: message.message_id,
        can_stop: consts::STREAM_CAN_STOP,
        keep_on_stop: false,
        timeout_ms: consts::ACTION_TIMEOUT_MS,
    };
    let topic_key = topic_id.unwrap_or(0);
    let interval = Duration::from_millis(consts::STREAM_INTERVAL_MS);
    let mut buffer = String::new();
    let mut last_update = Instant::now() - interval;
    let mut rich_ok = true;
    let mut drafts_blocked = false;
    let mut sink = |delta: &str| -> bool {
        buffer.push_str(delta);
        if state.take_stop(message.chat_id, topic_key, message.message_id) {
            return false;
        }
        if last_update.elapsed() >= interval && !drafts_blocked {
            last_update = Instant::now();
            // 停在公式中间就不更新这一帧：半个 $ 会被宿主渲染成裸美元符号。
            let formula_closed = !math_open(&buffer);
            if rich_ok
                && formula_closed
                && let Err(error) = state.td.send_rich_draft(&draft, &rich_text(&buffer))
            {
                if is_rate_limited(&error.to_string()) {
                    host.warn("agent: 草稿更新触发限流，后续不再更新草稿（终稿仍会发送）");
                    drafts_blocked = true;
                } else {
                    host.warn(&format!("agent: 富草稿更新失败，退回文本草稿：{error}"));
                    rich_ok = false;
                }
            }
            if !rich_ok && !drafts_blocked && formula_closed {
                let preview = strip_markdown(&buffer);
                if !preview.trim().is_empty()
                    && let Err(error) = state.td.send_text_draft(&draft, &preview)
                {
                    if is_rate_limited(&error.to_string()) {
                        host.warn("agent: 草稿更新触发限流，后续不再更新草稿（终稿仍会发送）");
                        drafts_blocked = true;
                    } else {
                        // 草稿发不出去就停流：终稿仍会发（用已经收到的部分）。
                        return false;
                    }
                }
            }
        }
        true
    };

    // 占位符不在收到消息时发，而是在网关流**连通之后**发（见 run_stream 的 on_connected）。
    let mut placeholder_sent = false;
    let mut on_connected = || {
        if !placeholder_sent {
            placeholder_sent = true;
            send_thinking(host, state, &draft);
        }
    };

    match state
        .engine
        .run_stream(&state.store, request, &mut on_connected, &mut sink)
    {
        Ok(outcome) if outcome.cancelled => {
            host.info("agent: 用户停止生成，丢弃终稿");
            Ok(None)
        }
        Ok(outcome) => {
            if let Some(error) = &outcome.error {
                host.warn(&format!("agent: 流式中断，发送已收到的部分：{error}"));
            }
            let mut text = outcome.reply;
            if text.trim().is_empty() {
                text = failure_text(outcome.error.as_deref().unwrap_or("模型没有返回内容"));
            }
            finish_reply(host, state, message, topic_id, &text, true)?;
            Ok(Some((outcome.session, text.chars().count())))
        }
        Err(error) => {
            host.warn(&format!("agent: 流式推理失败：{error}"));
            let text = failure_text(&error.to_string());
            finish_reply(host, state, message, topic_id, &text, false)?;
            Ok(Some((request.key.file_stem(), text.chars().count())))
        }
    }
}

/// 首次聊天：话题标题 = 首条消息内容（自动建的话题已经是它，无需重复设置）。
///
/// 不额外调用模型；AI / 其它插件想改名用工具 `fengyegf.agent.title`。
fn set_topic_from_first_message(
    host: &HostApi,
    state: &PluginState,
    message: &IncomingMessage,
    topic_id: Option<i32>,
    created_topic: bool,
) {
    let Some(topic_id) = topic_id else {
        return;
    };
    if created_topic {
        return;
    }
    let title = topic_title(&message.display_text());
    match state
        .td
        .edit_topic(message.chat_id, topic_id, &title, consts::ACTION_TIMEOUT_MS)
    {
        Ok(()) => host.info(&format!(
            "agent: 首次对话已设置话题 #{topic_id} 标题为「{title}」"
        )),
        Err(error) => host.warn(&format!("agent: 设置话题 #{topic_id} 标题失败：{error}")),
    }
    // 会话还没建时 rename 返回 false，紧接着的推理会用同一段首条消息建会话，无需补救。
    let _ = state
        .store
        .rename(&ChatKey::new(message.chat_id, Some(topic_id)), &title);
}

/// 发送正式消息（不带 reply_to）+ 投递日志。
fn finish_reply(
    host: &HostApi,
    state: &PluginState,
    message: &IncomingMessage,
    topic_id: Option<i32>,
    text: &str,
    streamed: bool,
) -> AgentResult<()> {
    let normalized = rich_text(text);
    let sent = send_with_mode(state, message.chat_id, topic_id, &normalized, None)?;
    host.info(&format!(
        "agent: 回复已投递（{}；新消息 {}，chat {}，topic {:?}{}）",
        mode_label(state.config.reply_mode),
        sent.message_id,
        message.chat_id,
        topic_id,
        if streamed { "，流式" } else { "" }
    ));
    Ok(())
}

/// 官方流式回复：Thinking 草稿 → 分块出现 → 正式消息落地。
///
/// - 私有聊天用 TDLib 草稿消息（`sendTextMessageDraft`）：空文本 = 客户端自带的
///   Thinking… 占位，同一 `draft_id` 反复更新会做增量动画，最后 `sendMessage` 落地，
///   草稿自动消失；
/// - 群聊没有草稿能力，直接发终稿；
/// - 两种路径都不引用用户消息（官方演示里没有回复引用）。
fn stream_reply(
    host: &HostApi,
    state: &PluginState,
    message: &IncomingMessage,
    topic_id: Option<i32>,
    text: &str,
) -> AgentResult<()> {
    let config = &state.config;
    let text = rich_text(&truncate(text, consts::MAX_REPLY_CHARS));
    let chat_id = message.chat_id;
    let topic_key = topic_id.unwrap_or(0);
    let draft_id = message.message_id;
    let streaming = config.stream_replies && message.is_private();
    let draft = crate::td::DraftSend {
        chat_id,
        topic_id,
        draft_id,
        can_stop: consts::STREAM_CAN_STOP,
        keep_on_stop: false,
        timeout_ms: consts::ACTION_TIMEOUT_MS,
    };

    if streaming {
        send_thinking(host, state, &draft);
    }

    // 用户在生成过程中按了「停止」：keep_on_stop = false，草稿已随停止消失，终稿也不发。
    if state.take_stop(chat_id, topic_key, draft_id) {
        host.info("agent: 用户停止生成，丢弃这条回复");
        return Ok(());
    }

    if streaming {
        reveal(host, state, &draft, &text)?;
        if state.take_stop(chat_id, topic_key, draft_id) {
            host.info("agent: 用户停止生成，丢弃终稿");
            return Ok(());
        }
    }

    let sent = send_with_mode(state, chat_id, topic_id, &text, None)?;
    host.info(&format!(
        "agent: 回复已投递（{}；新消息 {}，chat {}，topic {:?}{}）",
        mode_label(config.reply_mode),
        sent.message_id,
        chat_id,
        topic_id,
        if streaming { "，流式" } else { "" }
    ));
    Ok(())
}

/// 发一条思考占位：富草稿走 `inputPageBlockThinking` 块，文本模式走空草稿。
fn send_thinking(host: &HostApi, state: &PluginState, draft: &crate::td::DraftSend) {
    let result = state.td.send_thinking_draft(draft, consts::THINKING_TEXT);
    match result {
        Ok(()) => host.log(
            LogLevel::Debug,
            &format!("agent: 草稿占位 #{}", draft.draft_id),
        ),
        Err(error) => host.warn(&format!(
            "agent: 发思考占位失败（继续流式，只是没有 Thinking 占位）：{error}"
        )),
    }
}

/// 把终稿按块喂给同一个草稿 id，做出官方那种「逐段出现」的效果。
///
/// 这是**网关不支持 stream 时的兜底**（同步拿终稿再回放）；网关支持时走
/// [`stream_from_gateway`] 的真 token 流。两条路共用同一套富草稿 / 文本草稿。
fn reveal(
    host: &HostApi,
    state: &PluginState,
    draft: &crate::td::DraftSend,
    text: &str,
) -> AgentResult<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let chunks = chunk_text(text, reveal_chunk_chars(text.chars().count()));
    let topic_key = draft.topic_id.unwrap_or(0);
    let mut raw = String::with_capacity(text.len());
    let mut rich_ok = true;
    let mut drafts_blocked = false;
    for (index, chunk) in chunks.iter().enumerate() {
        if state.take_stop(draft.chat_id, topic_key, draft.draft_id) {
            return Ok(());
        }
        raw.push_str(chunk);
        let formula_closed = !math_open(&raw);
        if !drafts_blocked
            && rich_ok
            && formula_closed
            && let Err(error) = state.td.send_rich_draft(draft, &raw)
        {
            if is_rate_limited(&error.to_string()) {
                host.warn("agent: 草稿更新触发限流，后续不再更新草稿（终稿仍会发送）");
                drafts_blocked = true;
            } else {
                host.warn(&format!("agent: 富草稿更新失败，退回文本草稿：{error}"));
                rich_ok = false;
            }
        }
        if !drafts_blocked && !rich_ok && formula_closed {
            let preview = strip_markdown(&raw);
            if let Err(error) = state.td.send_text_draft(draft, &preview) {
                if is_rate_limited(&error.to_string()) {
                    host.warn("agent: 草稿更新触发限流，后续不再更新草稿（终稿仍会发送）");
                    drafts_blocked = true;
                } else {
                    // 流式失败不重试：直接把终稿发出去，别让用户干等。
                    host.warn(&format!("agent: 草稿流式更新失败，直接发终稿：{error}"));
                    return Ok(());
                }
            }
        }
        if index + 1 < chunks.len() {
            std::thread::sleep(Duration::from_millis(consts::STREAM_INTERVAL_MS));
        }
    }
    host.log(
        LogLevel::Debug,
        &format!("agent: 草稿流式完成（{} 块）", chunks.len()),
    );
    Ok(())
}

/// 每块字符数：按内置常量，并按「总时长上限」自动放大。
fn reveal_chunk_chars(total_chars: usize) -> usize {
    let base = consts::STREAM_CHUNK_CHARS;
    let interval = consts::STREAM_INTERVAL_MS;
    let max_steps = (consts::STREAM_MAX_SECONDS * 1000 / interval).max(1) as usize;
    base.max(total_chars.div_ceil(max_steps))
}

/// 富消息前的公式归一（固定开启）。
fn rich_text(markdown: &str) -> String {
    normalize_math(markdown)
}

/// TDLib / Telegram 的限流错误（429 / flood wait）。
fn is_rate_limited(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("429") || lower.contains("too many requests") || lower.contains("flood")
}

/// 回复形态的短标签（日志用）。
fn mode_label(mode: crate::core::ReplyMode) -> &'static str {
    match mode {
        crate::core::ReplyMode::Plain => "plain",
        crate::core::ReplyMode::Markdown => "markdown",
        crate::core::ReplyMode::Rich => "rich",
    }
}

/// 普通命令回复（不经过 Agent）。
fn reply_text(state: &PluginState, message: &IncomingMessage, text: &str) -> AgentResult<()> {
    let text = truncate(text, consts::MAX_REPLY_CHARS);
    send_with_mode(
        state,
        message.chat_id,
        message.topic_id,
        &text,
        Some(message.message_id),
    )?;
    Ok(())
}

/// 按配置的 `reply_mode` 发送，逐级降级：rich → markdown → plain。
fn send_with_mode(
    state: &PluginState,
    chat_id: i64,
    topic_id: Option<i32>,
    text: &str,
    reply_to: Option<i64>,
) -> AgentResult<SentMessage> {
    let config = &state.config;
    let timeout = consts::SEND_TIMEOUT_MS;
    match config.reply_mode {
        crate::core::ReplyMode::Plain => state
            .td
            .send_text(chat_id, topic_id, text, None, reply_to, timeout),
        crate::core::ReplyMode::Markdown => {
            match state
                .td
                .send_text(chat_id, topic_id, text, Some("markdown"), reply_to, timeout)
            {
                Ok(sent) => Ok(sent),
                Err(error) => {
                    state
                        .host
                        .warn(&format!("agent: markdown 发送失败，退回纯文本：{error}"));
                    state
                        .td
                        .send_text(chat_id, topic_id, text, None, reply_to, timeout)
                }
            }
        }
        crate::core::ReplyMode::Rich => {
            match state
                .td
                .send_rich(chat_id, topic_id, text, reply_to, timeout)
            {
                Ok(sent) => Ok(sent),
                Err(rich_error) => {
                    state.host.warn(&format!(
                        "agent: 富消息发送失败，退回 markdown：{rich_error}"
                    ));
                    match state.td.send_text(
                        chat_id,
                        topic_id,
                        text,
                        Some("markdown"),
                        reply_to,
                        timeout,
                    ) {
                        Ok(sent) => Ok(sent),
                        Err(markdown_error) => {
                            state.host.warn(&format!(
                                "agent: markdown 发送失败，退回纯文本：{markdown_error}"
                            ));
                            state
                                .td
                                .send_text(chat_id, topic_id, text, None, reply_to, timeout)
                        }
                    }
                }
            }
        }
    }
}

/// 失败文案：原因截断到 300 字符。硬依赖下网关不在场只可能是配置事故，
/// 文案仍给用户看到原因，便于排查。
pub fn failure_text(error: &str) -> String {
    let mut reason = error.trim().replace('\n', " ");
    if reason.chars().count() > 300 {
        reason = reason.chars().take(300).collect::<String>() + "…";
    }
    format!("请求失败：{reason}")
}

/// 话题标题：首条消息的可见前缀。
fn topic_title(text: &str) -> String {
    let plain: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = plain.chars().take(consts::TOPIC_TITLE_MAX_CHARS).collect();
    if title.trim().is_empty() {
        "AI 会话".to_owned()
    } else {
        title.trim().to_owned()
    }
}

/// 截断到字符上限。
fn truncate(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// 群内是否点了我（@username）。
fn mentions_me(host: &HostApi, message: &IncomingMessage) -> bool {
    let text = message.text.to_lowercase();
    if text.trim().is_empty() {
        return false;
    }
    for username in my_usernames(host) {
        if text.contains(&format!("@{}", username.to_lowercase())) {
            return true;
        }
    }
    false
}

/// 自己的全部用户名（宿主身份服务，纯内存）。
fn my_usernames(host: &HostApi) -> Vec<String> {
    let Ok(text) = host.call_service("fuyubot.host.identity", "usernames", "{}") else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text).unwrap_or_default()
}

/// 显式引用，避免 `AgentError` 在未使用时报未用导入。
#[allow(dead_code)]
fn _assert_error_used(error: AgentError) -> String {
    error.to_string()
}
