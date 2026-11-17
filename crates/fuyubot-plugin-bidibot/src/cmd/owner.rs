//! 管理命令的实现（`/bidiban`、`/bidiunban` …），v0.2 群 + 话题模式。
//!
//! 为什么单独一个文件：管理命令都是"读状态 / 改状态"，和分派（[`crate::cmd`]）、
//! 解析（[`crate::cmd::parser`]）没有耦合；分派只认名字，行为全在这里。
//!
//! 三条纪律：
//! 1. 一条命令**只回一条**消息，引用命令本身，并且回在同一条话题里（见 [`crate::cmd::reply`]）；
//! 2. 目标用户先看命令里的显式 id，其次用"本条消息所在话题对应的用户"（`topic_user`）；
//!    General 话题没有对应用户，需要目标的命令必须显式带 id；
//! 3. 外部输入（id / 语言码 / 关键词）先校验再落地，失败要告诉操作者"该怎么改"。
//!
//! 鉴权不在这里：调用方（[`crate::cmd::handle_group`]）已经确认过"这是配置的群 +
//! 发送者有管理权限"。但**参数**一律当不可信输入处理。

use std::sync::Arc;

use fuyubot_plugin_api::HostExt as _;
use serde_json::json;

use super::reply;
use crate::cmd::parser::Command;
use crate::core::config::{Config, DEFAULT_AD_KEYWORDS};
use crate::core::store::{BanRecord, StoreStats};
use crate::host::{Ctx, IncomingMessage, human_age, now};

/// `/bidilist` 最多显示多少条：再多就是刷屏，剩下的让操作者看本地库。
const LIST_LIMIT: usize = 20;
/// `/bidiword list` 最多显示多少个关键词。
const WORD_LIMIT: usize = 30;
/// 拉黑原因最多保留多少字符（会写进本地库，别让手滑粘一整段话进去）。
const REASON_MAX_CHARS: usize = 120;
/// 单个关键词最多多少字符（会写回 fuyubot.toml）。
const WORD_MAX_CHARS: usize = 64;

/// 是不是管理命令。分派和"非管理员发管理命令"的拒绝都靠它。
pub fn owns(name: &str) -> bool {
    matches!(
        name,
        "bidiban"
            | "bidiunban"
            | "bidilist"
            | "bidiwarn"
            | "bidiclear"
            | "bidiword"
            | "bidistatus"
            | "bidisay"
            | "bidicard"
    )
}

/// 执行一条管理命令；不是管理命令返回 `false`（交回分派层）。
///
/// `topic_user` = 本条消息所在话题对应的用户（General 话题为 `None`）。
pub fn handle(
    ctx: &Arc<Ctx>,
    message: &IncomingMessage,
    command: &Command,
    topic_user: Option<i64>,
) -> bool {
    match command.name.as_str() {
        "bidiban" => ban(ctx, message, command, topic_user),
        "bidiunban" => unban(ctx, message, command, topic_user),
        "bidilist" => list_bans(ctx, message),
        "bidiwarn" => show_warnings(ctx, message, command, topic_user),
        "bidiclear" => clear_warnings(ctx, message, command, topic_user),
        "bidiword" => words(ctx, message, command),
        "bidistatus" => status(ctx, message),
        "bidicard" => card(ctx, message, topic_user),
        "bidisay" => say(ctx, message, command, topic_user),
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------- 封禁管理

/// `/bidiban [id] [原因]`：拉黑一个用户。
///
/// 在某个用户的话题里直接发 `/bidiban`（后面写什么都当原因）就拉黑该用户；
/// 在 General 或想拉黑别人时显式写 id：`/bidiban <id> 原因`。
fn ban(ctx: &Arc<Ctx>, message: &IncomingMessage, command: &Command, topic_user: Option<i64>) {
    let config = ctx.config();
    let (target, reason) = match ban_args(&command.args, topic_user) {
        Ok(parsed) => parsed,
        Err(reason) => {
            reply(ctx, message, &format!("❓ {reason}"));
            return;
        }
    };
    let user_id = target.user_id;
    if config.owner_id != 0 && user_id == config.owner_id {
        reply(ctx, message, "🙃 不能拉黑自己。");
        return;
    }
    if let Some(record) = ctx.store.is_banned(user_id) {
        let age = human_age(now().saturating_sub(record.at));
        let reason = reason_text(&record.reason, "（未记录原因）");
        reply(
            ctx,
            message,
            &format!(
                "ℹ️ 用户 {user_id} 已经在黑名单里（{age}，原因：{reason}）。\n解除用 /bidiunban {user_id}。"
            ),
        );
        return;
    }
    match ctx.store.ban(user_id, &reason, config.owner_id, false) {
        Ok(()) => {
            ctx.host()
                .info(&format!("bidibot: 拉黑 {user_id}（{reason}）"));
            let who = target_label(&target);
            reply(
                ctx,
                message,
                &format!(
                    "🚫 已拉黑用户 {who}。\n原因：{reason}\n该用户的消息不再转发进群；解封用 /bidiunban {user_id}（或在 ta 的话题里发 /bidiunban）。"
                ),
            );
        }
        Err(error) => {
            ctx.host()
                .error(&format!("bidibot: 拉黑 {user_id} 失败: {error}"));
            reply(
                ctx,
                message,
                &format!(
                    "❌ 写本地库失败：{error}\n检查 store_path 指向的目录是否可写，然后重试。"
                ),
            );
        }
    }
}

/// `/bidiunban [id]`：解除拉黑。
fn unban(ctx: &Arc<Ctx>, message: &IncomingMessage, command: &Command, topic_user: Option<i64>) {
    let target = match target_required(&command.args, topic_user, "/bidiunban [user_id]") {
        Ok(target) => target,
        Err(reason) => {
            reply(ctx, message, &format!("❓ {reason}"));
            return;
        }
    };
    let user_id = target.user_id;
    match ctx.store.unban(user_id) {
        Ok(true) => {
            ctx.host().info(&format!("bidibot: 解封 {user_id}"));
            let who = target_label(&target);
            reply(
                ctx,
                message,
                &format!("✅ 已解除用户 {who} 的拉黑，消息会重新转发进群。"),
            );
        }
        Ok(false) => reply(
            ctx,
            message,
            &format!("ℹ️ 用户 {user_id} 不在黑名单里，不用解封。"),
        ),
        Err(error) => {
            ctx.host()
                .error(&format!("bidibot: 解封 {user_id} 失败: {error}"));
            reply(ctx, message, &format!("❌ 写本地库失败：{error}"));
        }
    }
}

/// `/bidilist`：拉黑列表（前 [`LIST_LIMIT`] 条）。不需要目标用户，General 也能用。
fn list_bans(ctx: &Arc<Ctx>, message: &IncomingMessage) {
    let bans = ctx.store.bans();
    reply(ctx, message, &render_ban_list(&bans, now(), LIST_LIMIT));
}

/// 黑名单正文。纯函数：ban 记录 + 当前时间 → 文案，单测不用造 `Store`。
pub fn render_ban_list(bans: &[BanRecord], current: u64, limit: usize) -> String {
    if bans.is_empty() {
        return "✅ 黑名单是空的，现在没有用户被拦截。".to_owned();
    }
    let mut out = format!("🚫 黑名单：{} 人\n", bans.len());
    for (index, record) in bans.iter().take(limit).enumerate() {
        let index = index + 1;
        let user_id = record.user_id;
        let age = human_age(current.saturating_sub(record.at));
        let source = if record.auto {
            "自动熔断"
        } else {
            "手动"
        };
        let reason = reason_text(&record.reason, "（未记录原因）");
        out.push_str(&format!(
            "{index}. {user_id}  {age}  {source}\n   原因：{reason}\n"
        ));
    }
    if bans.len() > limit {
        let hidden = bans.len() - limit;
        out.push_str(&format!("\n……还有 {hidden} 人没显示。"));
    }
    out.push_str("\n解封：/bidiunban <user_id>（或在该用户的话题里发 /bidiunban）");
    out
}

// ---------------------------------------------------------------- 警告

/// `/bidiwarn [id]`：看某个用户的警告数。
fn show_warnings(
    ctx: &Arc<Ctx>,
    message: &IncomingMessage,
    command: &Command,
    topic_user: Option<i64>,
) {
    let target = match target_required(&command.args, topic_user, "/bidiwarn [user_id]") {
        Ok(target) => target,
        Err(reason) => {
            reply(ctx, message, &format!("❓ {reason}"));
            return;
        }
    };
    let user_id = target.user_id;
    let config = ctx.config();
    let count = ctx.store.warnings(user_id);
    let limit = config.auto_ban_after.max(1);
    let state = if ctx.store.is_banned(user_id).is_some() {
        format!("已拉黑（/bidiunban {user_id} 解除）")
    } else {
        "正常".to_owned()
    };
    let who = target_label(&target);
    reply(
        ctx,
        message,
        &format!(
            "📊 用户 {who} 的警告：{count}/{limit}，状态：{state}。\n清零用 /bidiclear {user_id}；自动熔断阈值看 auto_ban_after。"
        ),
    );
}

/// `/bidiclear [id]`：清零警告（不解除拉黑）。
fn clear_warnings(
    ctx: &Arc<Ctx>,
    message: &IncomingMessage,
    command: &Command,
    topic_user: Option<i64>,
) {
    let target = match target_required(&command.args, topic_user, "/bidiclear [user_id]") {
        Ok(target) => target,
        Err(reason) => {
            reply(ctx, message, &format!("❓ {reason}"));
            return;
        }
    };
    let user_id = target.user_id;
    match ctx.store.clear_warnings(user_id) {
        Ok(true) => {
            let who = target_label(&target);
            reply(
                ctx,
                message,
                &format!(
                    "🧹 已清零用户 {who} 的警告。\n（不影响黑名单：还在黑名单里就用 /bidiunban {user_id} 解除）"
                ),
            );
        }
        Ok(false) => reply(
            ctx,
            message,
            &format!("ℹ️ 用户 {user_id} 没有警告记录，不用清。"),
        ),
        Err(error) => {
            ctx.host()
                .error(&format!("bidibot: 清零 {user_id} 警告失败: {error}"));
            reply(ctx, message, &format!("❌ 写本地库失败：{error}"));
        }
    }
}

// ---------------------------------------------------------------- 关键词

/// `/bidiword add|del|list`：维护广告关键词。不需要目标用户，General 也能用。
fn words(ctx: &Arc<Ctx>, message: &IncomingMessage, command: &Command) {
    let action = match word_action(&command.args) {
        Ok(action) => action,
        Err(usage) => {
            reply(ctx, message, &format!("❓ {usage}"));
            return;
        }
    };
    match action {
        WordAction::List => {
            let config = ctx.config();
            reply(ctx, message, &render_words(&config, WORD_LIMIT));
        }
        WordAction::Add(word) => add_word(ctx, message, word),
        WordAction::Del(word) => del_word(ctx, message, word),
    }
}

/// `/bidiword` 的子动作。
#[derive(Debug)]
enum WordAction {
    /// 追加一个关键词。
    Add(String),
    /// 从追加表里删掉一个关键词。
    Del(String),
    /// 列出生效的关键词。
    List,
}

/// 解析 `/bidiword` 的参数：`args[0]` 是子命令，后面整串是关键词。
///
/// 关键词里允许有空格（`add 加 微信`），所以按"其余参数用空格拼回来"处理，
/// 而不是只取一个参数——中文关键词里带空格的情况确实存在。
fn word_action(args: &[String]) -> Result<WordAction, String> {
    const USAGE: &str = "用法：/bidiword add <词> 或 /bidiword del <词> 或 /bidiword list";
    let Some(sub) = args.first() else {
        return Err(USAGE.to_owned());
    };
    let word = clip(&args.get(1..).unwrap_or_default().join(" "), WORD_MAX_CHARS);
    match sub.trim().to_ascii_lowercase().as_str() {
        "add" | "a" | "加" => {
            if word.is_empty() {
                Err("用法：/bidiword add <词>".to_owned())
            } else {
                Ok(WordAction::Add(word))
            }
        }
        "del" | "delete" | "rm" | "删" => {
            if word.is_empty() {
                Err("用法：/bidiword del <词>".to_owned())
            } else {
                Ok(WordAction::Del(word))
            }
        }
        "list" | "ls" | "列表" => Ok(WordAction::List),
        _ => Err(USAGE.to_owned()),
    }
}

/// 追加关键词：写配置 `ad_keywords_extra`（追加表），走 [`Ctx::update_config`] 落盘。
fn add_word(ctx: &Arc<Ctx>, message: &IncomingMessage, word: String) {
    let config = ctx.config();
    if config.ad_keywords.iter().any(|item| item.as_str() == word) {
        reply(
            ctx,
            message,
            &format!("ℹ️ 「{word}」已经在配置文件的 ad_keywords 里，本来就会生效。"),
        );
        return;
    }
    if config
        .ad_keywords_extra
        .iter()
        .any(|item| item.as_str() == word)
    {
        reply(ctx, message, &format!("ℹ️ 「{word}」已经在追加表里了。"));
        return;
    }
    let mut list = config.ad_keywords_extra.clone();
    list.push(word.clone());
    let total = list.len();
    let value = json!(list.clone());
    ctx.update_config("ad_keywords_extra", value, |next| {
        next.ad_keywords_extra = list;
    });
    let effective = ctx.config().effective_ad_keywords().len();
    reply(
        ctx,
        message,
        &format!(
            "✅ 已追加广告关键词「{word}」（追加表 {total} 个，整体生效 {effective} 个）。\n撤销：/bidiword del {word}"
        ),
    );
}

/// 删除关键词：只删追加表里的。
///
/// 配置里 `ad_keywords`（整份替换内置表）和内置表都不是运行时可变的状态：
/// 前者是手写的 toml，后者要改代码；命令只动追加表，避免"命令删了、重启又回来"。
fn del_word(ctx: &Arc<Ctx>, message: &IncomingMessage, word: String) {
    let config = ctx.config();
    let mut list = config.ad_keywords_extra.clone();
    let before = list.len();
    list.retain(|item| item.as_str() != word);
    if list.len() == before {
        if config.ad_keywords.iter().any(|item| item.as_str() == word) {
            reply(
                ctx,
                message,
                &format!(
                    "ℹ️ 「{word}」来自配置文件的 ad_keywords，命令删不掉：改 fuyubot.toml 里的 [plugin.bidibot].ad_keywords 后重启。"
                ),
            );
        } else if DEFAULT_AD_KEYWORDS.contains(&word.as_str()) {
            reply(
                ctx,
                message,
                &format!(
                    "ℹ️ 「{word}」是内置关键词，删不掉。\n要换内置表：把完整关键词写进 ad_keywords（它会整份替换内置表）。"
                ),
            );
        } else {
            reply(
                ctx,
                message,
                &format!(
                    "ℹ️ 追加表里没有「{word}」，可能已经删过了。\n/bidiword list 看当前生效的关键词。"
                ),
            );
        }
        return;
    }
    let total = list.len();
    let value = json!(list.clone());
    ctx.update_config("ad_keywords_extra", value, |next| {
        next.ad_keywords_extra = list;
    });
    reply(
        ctx,
        message,
        &format!("🗑 已移除追加关键词「{word}」（追加表剩 {total} 个）。"),
    );
}

/// 生效关键词列表：前 `limit` 个 + 各自的来源 + 怎么维护。
pub fn render_words(config: &Config, limit: usize) -> String {
    let effective = config.effective_ad_keywords();
    let mode = if config.ad_keywords.is_empty() {
        "内置表 + 追加"
    } else {
        "ad_keywords 覆盖内置表 + 追加"
    };
    let mut out = format!("📚 生效的广告关键词：{} 个（{mode}）\n", effective.len());
    if effective.is_empty() {
        out.push_str("（一个都没有：关键词拦截实际上不生效）\n");
    }
    for (index, word) in effective.iter().take(limit).enumerate() {
        let index = index + 1;
        let source = word_source(config, word);
        out.push_str(&format!("{index}. {word}（{source}）\n"));
    }
    if effective.len() > limit {
        let hidden = effective.len() - limit;
        out.push_str(&format!(
            "……还有 {hidden} 个没显示，完整的在 fuyubot.toml 的 [plugin.bidibot] 里。\n"
        ));
    }
    out.push_str("\n/bidiword add <词> 追加；/bidiword del <词> 只删追加表；配置里的词请改 toml。");
    out
}

/// 这个词从哪来：配置覆盖表 / 追加表 / 内置表。
fn word_source(config: &Config, word: &str) -> &'static str {
    if config.ad_keywords.iter().any(|item| item.as_str() == word) {
        "配置 ad_keywords"
    } else if config
        .ad_keywords_extra
        .iter()
        .any(|item| item.as_str() == word)
    {
        "追加"
    } else if DEFAULT_AD_KEYWORDS.contains(&word) {
        "内置"
    } else {
        "其它"
    }
}

// ---------------------------------------------------------------- 名片 / 状态 / 主动发送

/// `/bidicard`：在当前话题里重发这个用户的名片。
///
/// 用途：话题被清空 / 名片被删 / 想再看一眼对方资料。只有在该用户的话题里才有目标，
/// General 里必须显式给 id（这种场景请直接去对应话题里发）。
fn card(ctx: &Arc<Ctx>, message: &IncomingMessage, topic_user: Option<i64>) {
    let (Some(user_id), Some(thread)) = (topic_user, message.topic_thread_id) else {
        reply(
            ctx,
            message,
            "❓ /bidicard 需要在某个用户的话题里用（General 话题里没有对应用户）。",
        );
        return;
    };
    if crate::relay::resend_card(ctx, user_id, thread) {
        ctx.host()
            .info(&format!("bidibot: 已重发用户 {user_id} 的名片"));
    } else {
        reply(
            ctx,
            message,
            &format!("❌ 名片发送失败（用户 {user_id}）：查日志看是渲染、头像还是权限问题。"),
        );
    }
}

// ---------------------------------------------------------------- 状态 / 主动发送

/// `/bidistatus`：运行状态与计数。不需要目标用户，General 也能用。
fn status(ctx: &Arc<Ctx>, message: &IncomingMessage) {
    let config = ctx.config();
    let stats = ctx.store.stats();
    reply(
        ctx,
        message,
        &render_status(&config, stats, ctx.started_at, now()),
    );
}

/// 运行状态正文。纯函数：配置 + 库计数 + 时间 → 文案，单测不必造 `Ctx`。
pub fn render_status(config: &Config, stats: StoreStats, started_at: u64, current: u64) -> String {
    let group = if config.group_id == 0 {
        "未配置（转发不会工作）".to_owned()
    } else {
        config.group_id.to_string()
    };
    let owner = if config.owner_id == 0 {
        "未配置（管理命令不可用）".to_owned()
    } else {
        config.owner_id.to_string()
    };
    let ai = if config.ai_enabled() {
        "已配置"
    } else {
        "未配置"
    };
    let ad = config.effective_ad_mode().as_str();
    let limit = config.auto_ban_after.max(1);
    let words = config.effective_ad_keywords().len();
    let rate_limit = config.rate_limit_per_min;
    let rate = if rate_limit == 0 {
        "不限".to_owned()
    } else {
        format!("{rate_limit} 条/分钟")
    };
    let uptime = human_age(current.saturating_sub(started_at));
    let mut out = String::from("📊 bidibot 运行状态\n");
    out.push_str(&format!("群：{group}\n"));
    out.push_str(&format!("管理员：{owner}\n"));
    out.push_str(&format!("大模型：{ai}\n"));
    out.push_str(&format!("广告拦截：{ad}，生效关键词 {words} 个\n"));
    out.push_str(&format!("自动熔断：警告 {limit} 次\n"));
    out.push_str(&format!(
        "本地库：封禁 {} 人，用户档案 {} 个，有警告 {} 个，话题 {} 条\n",
        stats.bans, stats.users, stats.warned, stats.topics
    ));
    out.push_str(&format!("限流：{rate}\n"));
    out.push_str(&format!("运行：{uptime}\n"));
    out.push_str("/bidiword 维护关键词；其它项在 fuyubot.toml。");
    out
}

/// `/bidisay [id] <文字>`：以 bot 身份主动给某个用户发一条消息。
///
/// 在某个用户的话题里可以直接 `/bidisay <文字>`；在 General 里要写全 `/bidisay <id> <文字>`。
/// 发的是 **bot 的普通私聊消息**（私聊 chat_id 恒等于对方 user id），不转发、不带群与
/// 管理员的任何痕迹——和 relay 的"群 → 用户"方向同一条安全底线。
fn say(ctx: &Arc<Ctx>, message: &IncomingMessage, command: &Command, topic_user: Option<i64>) {
    let (target, text) = match say_args(&command.args, topic_user) {
        Ok(parsed) => parsed,
        Err(reason) => {
            reply(ctx, message, &format!("❓ {reason}"));
            return;
        }
    };
    let user_id = target.user_id;
    let config = ctx.config();
    let body = config.truncate_relay(&text);
    let timeout = config.send_timeout();
    match ctx
        .messenger
        .send_text_timeout(user_id, &body, None, timeout)
    {
        Ok(sent) => {
            let message_id = sent.message_id;
            ctx.host()
                .info(&format!("bidibot: 主动给 {user_id} 发了一条消息"));
            let who = target_label(&target);
            reply(
                ctx,
                message,
                &format!(
                    "✅ 已发送给 {who}（消息 id {message_id}）。\n对方只会看到 bot 发来的一条普通消息。"
                ),
            );
        }
        Err(error) => {
            ctx.host()
                .warn(&format!("bidibot: /bidisay 发给 {user_id} 失败: {error}"));
            reply(
                ctx,
                message,
                &format!(
                    "❌ 发给 {user_id} 失败：{error}\n确认 id 是否正确（对方得先给 bot 发过消息），或者稍后再试。"
                ),
            );
        }
    }
}

// ---------------------------------------------------------------- 目标解析

/// 命令作用在谁身上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    /// 用户 user id。
    user_id: i64,
    /// `true` = 来自"本条消息所在话题"；`false` = 命令里显式写的 id。
    from_topic: bool,
}

/// 回复里怎么称呼目标：显式 id 就是数字，话题来的标一下"当前话题"，
/// 让人知道这条命令打到了谁身上（尤其防止在错误的话题里手滑拉黑）。
fn target_label(target: &Target) -> String {
    if target.from_topic {
        format!("{}（当前话题）", target.user_id)
    } else {
        target.user_id.to_string()
    }
}

/// 解析参数里的**显式** user id。
///
/// 只认正整数：
/// - 没有参数 / 第一段不是数字 → `Ok(None)`（可能是原因或正文，交给调用方判断）；
/// - `@username` → 明确报不支持（插件只有 id，没有用户名到 id 的映射）。
fn explicit_target(args: &[String]) -> Result<Option<i64>, String> {
    let Some(raw) = args.first() else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.starts_with('@') {
        return Err(format!(
            "不支持 @username（{raw}）：请给数字 user id（话题里的名片或 /bidilist 里都有）。"
        ));
    }
    match raw.parse::<i64>() {
        Ok(id) if id > 0 => Ok(Some(id)),
        _ => Ok(None),
    }
}

/// 需要目标用户的命令（`/bidiunban`、`/bidiwarn`、`/bidiclear`）怎么找目标。
///
/// 顺序：显式 id → 本条话题对应的用户。General 话题没有"对应的用户"，必须显式写 id。
/// 参数里给了非数字的东西时报错，而不是悄悄退回话题用户——否则 `/bidiunban 123abc`
/// 会把话题里的用户解封，比报错更难查。
fn target_required(
    args: &[String],
    topic_user: Option<i64>,
    usage: &str,
) -> Result<Target, String> {
    match explicit_target(args)? {
        Some(user_id) => Ok(Target {
            user_id,
            from_topic: false,
        }),
        None => {
            if let Some(raw) = args
                .first()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
            {
                return Err(format!(
                    "「{raw}」不是有效的 user id：要给数字 id，或者不带参数用当前话题对应的用户。"
                ));
            }
            match topic_user {
                Some(user_id) => Ok(Target {
                    user_id,
                    from_topic: true,
                }),
                None => Err(missing_target(usage)),
            }
        }
    }
}

/// `/bidiban` 的参数解析：目标 + 原因。
///
/// 三种写法：
/// 1. `/bidiban <id> 原因`：显式 id，后面是原因；
/// 2. `/bidiban 原因`（在该用户的话题里）：整段都是原因，目标是话题用户；
/// 3. `/bidiban`（在该用户的话题里）：没有原因，用默认原因。
fn ban_args(args: &[String], topic_user: Option<i64>) -> Result<(Target, String), String> {
    const DEFAULT_REASON: &str = "管理员手动拉黑";
    match explicit_target(args)? {
        Some(user_id) => {
            let reason = clip(
                &args.get(1..).unwrap_or_default().join(" "),
                REASON_MAX_CHARS,
            );
            let reason = if reason.is_empty() {
                DEFAULT_REASON.to_owned()
            } else {
                reason
            };
            Ok((
                Target {
                    user_id,
                    from_topic: false,
                },
                reason,
            ))
        }
        None => {
            let reason = clip(&args.join(" "), REASON_MAX_CHARS);
            let reason = if reason.is_empty() {
                DEFAULT_REASON.to_owned()
            } else {
                reason
            };
            match topic_user {
                Some(user_id) => Ok((
                    Target {
                        user_id,
                        from_topic: true,
                    },
                    reason,
                )),
                None => Err(missing_target("/bidiban <user_id> [原因]")),
            }
        }
    }
}

/// `/bidisay` 的参数解析：目标 + 要发的文字。
///
/// `/bidisay <id> <文字>` 或（在话题里）`/bidisay <文字>`。
fn say_args(args: &[String], topic_user: Option<i64>) -> Result<(Target, String), String> {
    match explicit_target(args)? {
        Some(user_id) => {
            let text = args
                .get(1..)
                .unwrap_or_default()
                .join(" ")
                .trim()
                .to_owned();
            if text.is_empty() {
                return Err("缺少要发的内容。\n用法：/bidisay <user_id> <文字>".to_owned());
            }
            Ok((
                Target {
                    user_id,
                    from_topic: false,
                },
                text,
            ))
        }
        None => {
            let text = args.join(" ").trim().to_owned();
            if text.is_empty() {
                return Err("缺少要发的内容。\n用法：/bidisay <user_id> <文字>".to_owned());
            }
            match topic_user {
                Some(user_id) => Ok((
                    Target {
                        user_id,
                        from_topic: true,
                    },
                    text,
                )),
                None => Err(missing_target("/bidisay <user_id> <文字>")),
            }
        }
    }
}

/// 没给目标、也不在话题里时的提示：说清楚"该怎么补"。
fn missing_target(usage: &str) -> String {
    format!(
        "缺少 user id。\n用法：{usage}\n或者在某个用户的话题里执行（General 话题没有对应用户）。"
    )
}

// ---------------------------------------------------------------- 通用小工具

/// 库里的原因可能为空，也可能很长；展示和落盘都走这里。
fn reason_text(reason: &str, empty: &str) -> String {
    let reason = reason.trim();
    if reason.is_empty() {
        empty.to_owned()
    } else {
        clip(reason, REASON_MAX_CHARS)
    }
}

/// 按字符数截断（不是字节数：中文按字节截会截出半个字）。
fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn owns_recognises_every_management_command_and_nothing_else() {
        for name in [
            "bidiban",
            "bidiunban",
            "bidilist",
            "bidiwarn",
            "bidiclear",
            "bidiword",
            "bidistatus",
            "bidisay",
            "bidicard",
        ] {
            assert!(owns(name), "{name} 应当是管理命令");
        }
        for name in ["bidihelp", "bidistart", "bidiclaim", "start", "kfc", ""] {
            assert!(!owns(name), "{name} 不该被当成管理命令");
        }
    }

    #[test]
    fn explicit_target_only_accepts_positive_ids() {
        assert_eq!(
            explicit_target(&args(&["123456789"])),
            Ok(Some(123_456_789))
        );
        assert_eq!(explicit_target(&args(&[])), Ok(None));
        assert_eq!(explicit_target(&args(&["  "])), Ok(None));
        // 不是数字：交给调用方决定当原因还是报缺 id。
        assert_eq!(explicit_target(&args(&["刷广告"])), Ok(None));
        assert_eq!(explicit_target(&args(&["0"])), Ok(None));
        assert_eq!(explicit_target(&args(&["-5"])), Ok(None));
    }

    #[test]
    fn explicit_target_rejects_usernames() {
        let error = explicit_target(&args(&["@zhangsan"])).expect_err("@username 应当被拒绝");
        assert!(error.contains("@username"), "{error}");
        assert!(error.contains("数字"), "{error}");
    }

    #[test]
    fn explicit_id_wins_over_the_current_topic() {
        let target = target_required(&args(&["42"]), Some(7), "/x").expect("显式 id 应当生效");
        assert_eq!(target.user_id, 42);
        assert!(!target.from_topic);
    }

    #[test]
    fn topic_user_is_the_default_target_inside_a_topic() {
        let target = target_required(&args(&[]), Some(7), "/bidiunban [user_id]")
            .expect("话题里应当默认打到话题用户");
        assert_eq!(target.user_id, 7);
        assert!(target.from_topic);
        // 回复里要标出"当前话题"，防止打错人还不知道。
        assert_eq!(target_label(&target), "7（当前话题）");
    }

    #[test]
    fn general_topic_requires_an_explicit_id() {
        let error = target_required(&args(&[]), None, "/bidiunban [user_id]")
            .expect_err("General 里没有默认目标");
        assert!(error.contains("General"), "{error}");
        assert!(error.contains("/bidiunban [user_id]"), "{error}");
    }

    #[test]
    fn junk_argument_is_an_error_not_a_silent_topic_fallback() {
        let error = target_required(&args(&["123abc"]), Some(7), "/bidiunban [user_id]")
            .expect_err("非数字参数应当报错");
        assert!(error.contains("123abc"), "{error}");
    }

    #[test]
    fn ban_arguments_cover_all_three_forms() {
        let (target, reason) =
            ban_args(&args(&["42", "刷", "广告"]), None).expect("显式 id + 原因应当成功");
        assert_eq!(target.user_id, 42);
        assert!(!target.from_topic);
        assert_eq!(reason, "刷 广告");

        let (target, reason) =
            ban_args(&args(&["刷广告"]), Some(7)).expect("话题里只写原因应当成功");
        assert_eq!(target.user_id, 7);
        assert!(target.from_topic);
        assert_eq!(reason, "刷广告");

        let (target, reason) = ban_args(&args(&[]), Some(7)).expect("话题里不写原因应当成功");
        assert_eq!(target.user_id, 7);
        assert_eq!(reason, "管理员手动拉黑");

        assert!(ban_args(&args(&[]), None).is_err(), "General 里必须给 id");
        assert!(
            ban_args(&args(&["刷广告"]), None).is_err(),
            "General 里必须给 id"
        );
        assert!(
            ban_args(&args(&["@name"]), Some(7)).is_err(),
            "@username 不支持"
        );
    }

    #[test]
    fn say_arguments_follow_the_same_target_rule() {
        let (target, text) =
            say_args(&args(&["42", "你好", "啊"]), None).expect("显式 id + 正文应当成功");
        assert_eq!(target.user_id, 42);
        assert_eq!(text, "你好 啊");

        let (target, text) = say_args(&args(&["你好"]), Some(7)).expect("话题里只写正文应当成功");
        assert_eq!(target.user_id, 7);
        assert!(target.from_topic);
        assert_eq!(text, "你好");

        assert!(say_args(&args(&[]), Some(7)).is_err(), "没有正文要报错");
        assert!(
            say_args(&args(&["42"]), Some(7)).is_err(),
            "有 id 没正文要报错"
        );
        assert!(
            say_args(&args(&["你好"]), None).is_err(),
            "General 里必须给 id"
        );
    }

    #[test]
    fn word_action_parses_subcommands() {
        let added = word_action(&args(&["add", "加", "微信"]));
        assert!(
            matches!(&added, Ok(WordAction::Add(word)) if word.as_str() == "加 微信"),
            "add 应当拼回带空格的关键词，实际 {added:?}"
        );
        let removed = word_action(&args(&["DEL", "加微信"]));
        assert!(
            matches!(&removed, Ok(WordAction::Del(word)) if word.as_str() == "加微信"),
            "del 应当识别（大小写不敏感），实际 {removed:?}"
        );
        assert!(matches!(
            word_action(&args(&["list"])),
            Ok(WordAction::List)
        ));
        for usage in [
            &args(&[])[..],
            &args(&["add"])[..],
            &args(&["unknown", "x"])[..],
        ] {
            assert!(word_action(usage).is_err(), "{usage:?} 应当给出用法提示");
        }
    }

    #[test]
    fn clip_counts_chars_not_bytes() {
        assert_eq!(clip("  短  ", 10), "短");
        assert_eq!(clip("你好世界", 2), "你好…");
        assert_eq!(reason_text("  ", "（无）"), "（无）");
        assert_eq!(reason_text("刷广告", "（无）"), "刷广告");
    }

    fn ban_record(user_id: i64, reason: &str, auto: bool) -> BanRecord {
        BanRecord {
            user_id,
            at: 1_000,
            reason: reason.to_owned(),
            by: 1,
            auto,
        }
    }

    #[test]
    fn empty_ban_list_says_so() {
        let text = render_ban_list(&[], 2_000, LIST_LIMIT);
        assert!(text.contains("黑名单是空的"), "{text}");
    }

    #[test]
    fn ban_list_shows_age_source_and_reason() {
        let bans = [ban_record(42, "刷广告", false), ban_record(7, "", true)];
        let text = render_ban_list(&bans, 1_120, LIST_LIMIT);
        assert!(text.contains("黑名单：2 人"), "{text}");
        assert!(text.contains("42"), "{text}");
        assert!(text.contains("2 分钟前"), "{text}");
        assert!(text.contains("手动"), "{text}");
        assert!(text.contains("刷广告"), "{text}");
        assert!(text.contains("自动熔断"), "{text}");
        assert!(text.contains("（未记录原因）"), "{text}");
        assert!(text.contains("/bidiunban"), "{text}");
    }

    #[test]
    fn ban_list_truncates_at_the_limit() {
        let bans = [
            ban_record(1, "a", false),
            ban_record(2, "b", false),
            ban_record(3, "c", false),
        ];
        let text = render_ban_list(&bans, 1_000, 2);
        assert!(text.contains("还有 1 人没显示"), "{text}");
        assert!(!text.contains("3."), "{text}");
    }

    #[test]
    fn word_list_explains_each_source() {
        let config = Config {
            ad_keywords: vec!["广告甲".to_owned(), "广告乙".to_owned()],
            ad_keywords_extra: vec!["追加丙".to_owned()],
            ..Config::default()
        };
        let text = render_words(&config, WORD_LIMIT);
        assert!(text.contains("生效的广告关键词：3 个"), "{text}");
        assert!(text.contains("广告甲（配置 ad_keywords）"), "{text}");
        assert!(text.contains("追加丙（追加）"), "{text}");
        assert!(!text.contains("还有"), "{text}");
    }

    #[test]
    fn word_list_truncates_and_mentions_how_to_maintain() {
        let config = Config {
            ad_keywords: vec!["一".to_owned(), "二".to_owned(), "三".to_owned()],
            ..Config::default()
        };
        let text = render_words(&config, 1);
        assert!(text.contains("……还有 2 个没显示"), "{text}");
        assert!(text.contains("/bidiword add"), "{text}");
    }

    #[test]
    fn builtin_keywords_are_labelled_as_builtin() {
        let config = Config::default();
        let text = render_words(&config, 3);
        assert!(text.contains("内置表 + 追加"), "{text}");
        assert!(text.contains("（内置）"), "{text}");
    }

    #[test]
    fn status_shows_group_admin_and_effective_modes() {
        let text = render_status(&Config::default(), StoreStats::default(), 1_000, 4_600);
        assert!(text.contains("群：未配置"), "{text}");
        assert!(text.contains("管理员：未配置（管理命令不可用）"), "{text}");
        assert!(!text.contains("翻译："), "翻译功能已删除: {text}");
        assert!(text.contains("广告拦截：keyword"), "{text}");
        assert!(text.contains("自动熔断：警告 3 次"), "{text}");
        assert!(text.contains("运行：1 小时前"), "{text}");
        assert!(text.contains("限流：12 条/分钟"), "{text}");
        assert!(!text.contains("/bidiclaim"), "认领命令已删除: {text}");
    }

    #[test]
    fn status_counts_bans_users_warnings_and_topics() {
        let stats = StoreStats {
            bans: 2,
            users: 5,
            warned: 1,
            topics: 4,
        };
        let config = Config {
            owner_id: 777,
            group_id: -1_001_234_567_890,
            rate_limit_per_min: 0,
            ..Config::default()
        };
        let text = render_status(&config, stats, 0, 30);
        assert!(text.contains("群：-1001234567890"), "{text}");
        assert!(text.contains("管理员：777"), "{text}");
        assert!(text.contains("封禁 2 人"), "{text}");
        assert!(text.contains("用户档案 5 个"), "{text}");
        assert!(text.contains("有警告 1 个"), "{text}");
        assert!(text.contains("话题 4 条"), "{text}");
        assert!(text.contains("限流：不限"), "{text}");
    }
}
