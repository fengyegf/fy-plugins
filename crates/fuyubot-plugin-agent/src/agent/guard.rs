//! 防环：去重 / bot 连击频率与深度。
//!
//! worker 收到的消息里同一条可能因为 TDLib 重发而重复出现；群里的多个 bot 互相
//! 回复可能形成死循环。两类问题都在**进入推理之前**挡掉，不浪费网关额度。
//!
//! **人类消息不做时间限速**：连发两条是真实需求，按会话限速会把用户要问的话丢掉
//! （真机上踩过：话题创建的服务消息先占掉窗口，紧接着的用户消息被静默丢弃）。
//! 只有 bot 对端才按频率 / 连续深度收口。
//!
//! 状态全在内存里，由 PluginState 的 `Mutex<Guard>` 保护（本类型自己不加锁），
//! 所以这里没有任何 I/O，纯逻辑可单测。过期条目在 `check` 里顺手裁掉，避免长时间
//! 运行后 HashMap 无限增长。

use std::collections::HashMap;

use crate::core::{IncomingMessage, consts};

/// 连续 bot 往返的静默重置窗口：超过这个时长没有新往返，就当作新一轮对话。
///
/// 配置里没有这一项：它是"连续"的定义——隔了五分钟才来的 bot 消息不算连击。
const BOT_DEPTH_RESET_SECS: u64 = 300;

/// 状态表超过这个规模才做一次全量裁剪（把裁剪成本摊到多次调用上）。
const PRUNE_THRESHOLD: usize = 256;

/// 防环判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardVerdict {
    /// 放行。
    Allow,
    /// 丢弃，附原因（只进日志）。
    Drop(&'static str),
}

/// 一个 bot 对端的往返状态。
#[derive(Debug, Clone, Copy, Default)]
struct BotPeer {
    /// 最近一次 bot 往返的时间；0 = 还没有。
    last_turn: u64,
    /// 连续往返深度（每回复一次 +1）。
    depth: u32,
}

/// 去重 / 限速 / bot 深度。
pub struct Guard {
    dedupe_window_secs: u64,
    bot_rate_limit_secs: u64,
    bot_max_depth: u32,
    /// 消息去重表：`(chat_id, message_id) -> 首次看到的时间`。
    dedupe: HashMap<(i64, i64), u64>,
    /// 每个 bot 对端的连击状态。
    bots: HashMap<i64, BotPeer>,
}

impl Guard {
    /// 按内置常量建（防环节奏不开放配置）。
    pub fn new() -> Self {
        Self {
            dedupe_window_secs: consts::DEDUPE_WINDOW_SECS,
            bot_rate_limit_secs: consts::BOT_RATE_LIMIT_SECS,
            bot_max_depth: consts::BOT_MAX_DEPTH,
            dedupe: HashMap::new(),
            bots: HashMap::new(),
        }
    }

    /// 准入判定（会更新内部状态）。
    ///
    /// 判定顺序：去重 → bot 频率 / 深度。去重表**无论放不放行**都会记下这条消息。
    pub fn check(&mut self, message: &IncomingMessage, now: u64) -> GuardVerdict {
        self.prune(now);

        // 1. 去重：同一条消息在窗口内重复出现 = TDLib 重发，或上游重复回调。
        let identity = (message.chat_id, message.message_id);
        if self.dedupe_window_secs > 0
            && let Some(seen) = self.dedupe.get(&identity)
            && now.saturating_sub(*seen) <= self.dedupe_window_secs
        {
            return GuardVerdict::Drop("重复消息");
        }
        self.dedupe.insert(identity, now);

        // 2. bot 对端：频率 + 连续深度。普通用户的消息完全不碰这套额度。
        if message.sender_is_bot
            && let Some(peer) = message.sender_user_id
        {
            let state = *self.bot_peer(peer, now);
            if self.bot_rate_limit_secs > 0
                && state.last_turn > 0
                && now.saturating_sub(state.last_turn) < self.bot_rate_limit_secs
            {
                return GuardVerdict::Drop("bot 限速");
            }
            if self.bot_max_depth > 0 && state.depth >= self.bot_max_depth {
                return GuardVerdict::Drop("bot 深度超限");
            }
        }

        GuardVerdict::Allow
    }

    /// 记一次 bot 往返（深度）。
    pub fn note_bot_turn(&mut self, peer_user_id: i64, now: u64) {
        let state = self.bot_peer(peer_user_id, now);
        state.depth = state.depth.saturating_add(1);
        state.last_turn = now;
    }

    /// bot 深度（诊断用）。
    pub fn bot_depth(&self, peer_user_id: i64) -> u32 {
        self.bots.get(&peer_user_id).map_or(0, |peer| peer.depth)
    }

    /// 取（必要时重置）某个 bot 对端的状态。
    fn bot_peer(&mut self, peer_user_id: i64, now: u64) -> &mut BotPeer {
        let state = self.bots.entry(peer_user_id).or_default();
        if state.last_turn > 0 && now.saturating_sub(state.last_turn) > BOT_DEPTH_RESET_SECS {
            state.depth = 0;
            state.last_turn = 0;
        }
        state
    }

    /// 裁剪过期条目：只在表变大时做，平时是零开销。
    fn prune(&mut self, now: u64) {
        if self.dedupe.len() > PRUNE_THRESHOLD {
            let window = self.dedupe_window_secs.max(1);
            self.dedupe
                .retain(|_, seen| now.saturating_sub(*seen) <= window);
        }
        if self.bots.len() > PRUNE_THRESHOLD {
            let window = BOT_DEPTH_RESET_SECS.max(self.bot_rate_limit_secs);
            self.bots.retain(|_, peer| {
                peer.last_turn == 0 || now.saturating_sub(peer.last_turn) <= window
            });
        }
    }
}

impl Default for Guard {
    fn default() -> Self {
        Self::new()
    }
}
