//! 双向中转主链路（v0.2：论坛群 + 话题）。
//!
//! 两个方向：
//! - **用户 → 群**：确保该用户有话题（没有就用昵称建一个），把原消息**真转发**进话题；
//!   语言不符时先在话题里补一条译文。
//! - **群 → 用户**：主人在话题里回复，文本重新发一条（媒体用 send_copy 复制，不带转发头），
//!   用户永远看不到主人身份。
//!
//! 所有函数都在业务线程上跑（host::worker 起的那条），可以安全阻塞。

use std::sync::Arc;

use fuyubot_plugin_api::HostExt as _;

use crate::cmd;
use crate::core::ad::{AdJudge, Verdict};
use crate::core::config::Config;
use crate::core::store::{Profile, TopicRecord};
use crate::host::messenger::Error as MessengerError;
use crate::host::{Ctx, IncomingMessage, UserInfo, now, roles};

/// 一条消息的入口：按"私聊 / 我们的论坛群"分流。
pub fn handle(ctx: &Arc<Ctx>, message: &IncomingMessage) {
    let config = ctx.config();
    if message.is_private {
        handle_private(ctx, message, &config);
    } else if config.group_id != 0 && message.chat_id == config.group_id {
        handle_group(ctx, message, &config);
    }
}

// ---------------------------------------------------------------- 用户侧

/// 陌生人私聊：基础命令 → 封禁 → 广告 → 限流 → 转发进群。
fn handle_private(ctx: &Arc<Ctx>, message: &IncomingMessage, config: &Config) {
    let Some(user_id) = message.sender_user_id else {
        return;
    };
    // 基础命令对所有人开放，包括被封禁的用户。
    if cmd::handle_user(ctx, message) {
        return;
    }
    // 命令行一律不中转：/start、/help、别的插件的命令、打错的命令都一样。
    // 以前只有插件认得的命令被吃掉，其余会被当成普通消息转发进群，群里会看到一堆 /xxx。
    if crate::cmd::parser::parse(&message.text, &config.prefixes).is_some() {
        ctx.debug(&format!(
            "bidibot: 用户 {user_id} 发的是命令，不中转：{}",
            message.text
        ));
        return;
    }
    if config.group_id == 0 {
        // 没配群：不向陌生人确认"这是个中转 bot"，只说暂时不可用。
        ctx.send(message.chat_id, "🚧 暂时无法转发，请稍后再试。", None);
        return;
    }
    if let Some(ban) = ctx.store.is_banned(user_id) {
        notice_blocked(ctx, message, user_id, config, &ban.reason);
        return;
    }

    let judge = AdJudge::new(
        config.effective_ad_mode(),
        config.effective_ad_keywords(),
        ctx.llm.clone(),
        ctx.skills.clone(),
    )
    .with_timeout(config.ai.timeout);
    if let Verdict::Ad { reason, score } = judge.judge(&message.display_text()) {
        record_ad(ctx, message, user_id, config, &reason, score);
        return;
    }

    if !ctx.allow_rate(user_id) {
        ctx.debug(&format!(
            "bidibot: 用户 {user_id} 触发限流（{} 条/分钟），本条丢弃",
            config.rate_limit_per_min
        ));
        return;
    }

    relay_to_group(ctx, message, user_id, config);
}

/// 用户 → 群：确保话题 → 转发原消息。
fn relay_to_group(ctx: &Arc<Ctx>, message: &IncomingMessage, user_id: i64, config: &Config) {
    let group = config.group_id;
    let info = user_or_profile(ctx, user_id);
    let Some(thread) = ensure_topic(ctx, &info, config) else {
        ctx.send(message.chat_id, "⚠️ 消息暂时送不出去，请稍后再试。", None);
        return;
    };

    let forward = ctx.messenger.forward_to(
        group,
        message.chat_id,
        &[message.id],
        Some(thread),
        false,
        config.send_timeout(),
    );
    if let Err(error) = &forward {
        // 只有"话题真的没了"才重建：超时 / 断网时重建会造出重复话题，
        // 还会把已有映射弄丢（离线回放里踩过这个坑）。
        if !topic_gone(error) {
            ctx.host().error(&format!(
                "bidibot: 转发到话题 {thread} 失败（不重建话题）: {error}"
            ));
            ctx.send(message.chat_id, "⚠️ 消息暂时送不出去，请稍后再试。", None);
            return;
        }
        // 话题被删 / 没权限：清掉旧记录，重建一次再试（只重试一次）。
        ctx.host()
            .warn(&format!("bidibot: 话题 {thread} 已不可用，重建后重试"));
        if let Err(error) = ctx.store.remove_topic(user_id) {
            ctx.host()
                .warn(&format!("bidibot: 清理旧话题记录失败: {error}"));
        }
        let Some(thread) = ensure_topic(ctx, &info, config) else {
            ctx.send(message.chat_id, "⚠️ 消息暂时送不出去，请稍后再试。", None);
            return;
        };
        if let Err(error) = ctx.messenger.forward_to(
            group,
            message.chat_id,
            &[message.id],
            Some(thread),
            false,
            config.send_timeout(),
        ) {
            ctx.host().error(&format!("bidibot: 转发给群失败: {error}"));
            ctx.send(message.chat_id, "⚠️ 消息暂时送不出去，请稍后再试。", None);
            return;
        }
    }

    ctx.debug(&format!(
        "bidibot: 已转发用户{user_id} 的消息到话题 {thread}"
    ));
    remember_profile(ctx, user_id, &info);
}

/// 这个错误是不是"话题没了"（主人删了话题 / 机器人被踢出话题 / 无权限）。
///
/// 超时、宿主层失败都不算：那些应当原样报错，绝不能触发重建。
fn topic_gone(error: &MessengerError) -> bool {
    match error {
        MessengerError::Op { code, message } => {
            if code == "timeout" || code == "host" {
                return false;
            }
            code == "tdlib"
                || code == "bad_args"
                || code == "missing_field"
                || message.to_ascii_lowercase().contains("topic")
        }
        _ => false,
    }
}

/// 取（必要时创建）该用户的话题，返回 forum_topic_id。
fn ensure_topic(ctx: &Arc<Ctx>, info: &UserInfo, config: &Config) -> Option<i64> {
    if let Some(record) = ctx.store.topic(info.user_id) {
        return Some(record.thread_id);
    }
    let name = topic_name(info, config.topic_name_max, info.user_id);
    let thread = match ctx.messenger.create_topic(
        config.group_id,
        &name,
        config.topic_icon_color,
        config.send_timeout(),
    ) {
        Ok(thread) => thread,
        Err(error) => {
            ctx.host().error(&format!(
                "bidibot: 在群 {} 建话题「{name}」失败: {error}（bot 需要管理员 + 管理话题权限）",
                config.group_id
            ));
            return None;
        }
    };

    let mut card_message_id = 0;
    if config.topic_card {
        card_message_id = send_topic_card(ctx, info, config, thread);
    }

    let record = TopicRecord {
        user_id: info.user_id,
        thread_id: thread,
        name: name.clone(),
        created_at: now(),
        card_message_id,
    };
    if let Err(error) = ctx.store.set_topic(record) {
        ctx.host()
            .error(&format!("bidibot: 保存话题记录失败: {error}"));
    }
    ctx.host().info(&format!(
        "bidibot: 为用户 {} 新建话题「{name}」（thread {thread}）",
        info.user_id
    ));
    Some(thread)
}

// ---------------------------------------------------------------- 群侧

/// 群里的消息：命令优先，其次是"主人在话题里回复"。
fn handle_group(ctx: &Arc<Ctx>, message: &IncomingMessage, config: &Config) {
    // 匿名管理员（messageSenderChat）无法判定是谁，直接忽略。
    let Some(sender) = message.sender_user_id else {
        return;
    };
    let topic_user = message
        .topic_thread_id
        .and_then(|thread| ctx.store.topic_user(thread));

    if cmd::handle_group(ctx, message, topic_user) {
        return;
    }
    // 群里的命令行也不往用户那边发（没被命令层吃掉的命令 = 不认识的命令）。
    if crate::cmd::parser::parse(&message.text, &config.prefixes).is_some() {
        ctx.debug(&format!(
            "bidibot: 群里收到命令，不转给用户：{}",
            message.text
        ));
        return;
    }

    // 只有主人能往用户那边发；群里其他人闲聊一律忽略。
    // 主人来自宿主全局角色（`[roles].owners`），插件段 `owner_id` 是额外那一个。
    if !roles::is_owner(&ctx.host(), config.owner_id, sender) {
        return;
    }
    let Some(user_id) = topic_user else {
        // General 里发言没有对应用户：提示一次怎么用，别静默。
        if message.topic_thread_id.is_none() {
            ctx.messenger.try_send_text_in_topic(
                message.chat_id,
                "ℹ️ 这里是对应不上用户的总话题：请在某个用户的话题里回复，或在该话题里用 /bidisay <id> <文字>。",
                None,
                Some(message.id),
                config.send_timeout(),
            );
        }
        return;
    };
    relay_to_user(ctx, message, user_id, config);
}

/// 群 → 用户：文本重新发，媒体复制内容（不带转发头）。
fn relay_to_user(ctx: &Arc<Ctx>, message: &IncomingMessage, user_id: i64, config: &Config) {
    if message.kind == "messageText" {
        let text = message.text.trim();
        if text.is_empty() {
            return;
        }
        let out = config.truncate_relay(text);
        ctx.debug(&format!(
            "bidibot: 话题回复 → 用户{user_id}（{} 字，纯文本重发、不转发）",
            out.chars().count()
        ));
        if let Err(error) =
            ctx.messenger
                .send_text_timeout(user_id, &out, None, config.send_timeout())
        {
            report_to_group(
                ctx,
                config,
                message,
                &format!("⚠️ 发送给用户 {user_id} 失败：{error}"),
            );
        }
        return;
    }

    // 非文本：用 send_copy 复制内容，不带"转发自"头部，也不暴露主人的账号。
    ctx.debug(&format!(
        "bidibot: 话题回复 → 用户{user_id}（{}，复制内容不转发头部）",
        message.kind
    ));
    if let Err(error) = ctx.messenger.forward_to(
        user_id,
        message.chat_id,
        &[message.id],
        None,
        true,
        config.send_timeout(),
    ) {
        report_to_group(
            ctx,
            config,
            message,
            &format!("⚠️ 发送给用户 {user_id} 失败：{error}"),
        );
    }
}

/// 把失败/状态提示回在群里（同一个话题里，主人看得见上下文）。
fn report_to_group(ctx: &Arc<Ctx>, config: &Config, message: &IncomingMessage, text: &str) {
    ctx.messenger.try_send_text_in_topic(
        config.group_id,
        text,
        message.topic_thread_id,
        Some(message.id),
        config.send_timeout(),
    );
}

/// 广告命中：记警告；到阈值就自动熔断（在群里提示主人）。
fn record_ad(
    ctx: &Arc<Ctx>,
    message: &IncomingMessage,
    user_id: i64,
    config: &Config,
    reason: &str,
    score: f32,
) {
    let outcome = match ctx
        .store
        .warn(user_id, reason, config.warn_cooldown_secs, now())
    {
        Ok(outcome) => outcome,
        Err(error) => {
            ctx.host().error(&format!("bidibot: 记录警告失败: {error}"));
            return;
        }
    };
    if !outcome.counted {
        ctx.debug(&format!("bidibot: 用户 {user_id} 在警告冷却期内，静默丢弃"));
        return;
    }

    let limit = config.auto_ban_after.max(1);
    if outcome.count >= limit {
        if let Err(error) = ctx.store.ban(user_id, reason, config.owner_id, true) {
            ctx.host()
                .error(&format!("bidibot: 自动熔断 {user_id} 失败: {error}"));
            return;
        }
        ctx.send(
            message.chat_id,
            &format!(
                "🚫 已停止转发你的消息（触发广告拦截 {}/{limit} 次）。\n你仍可以使用 /bidihelp 查看基础命令。",
                outcome.count
            ),
            None,
        );
        // 群里留痕：General 话题，主人一眼能看到。
        ctx.messenger.try_send_text_in_topic(
            config.group_id,
            &format!(
                "🚫 已自动熔断用户 {user_id}\n原因：{reason}（置信度 {score:.2}）\n警告：{}/{limit}\n解封用 /bidiunban {user_id}。",
                outcome.count
            ),
            None,
            None,
            config.send_timeout(),
        );
        return;
    }

    ctx.send(
        message.chat_id,
        &format!(
            "⚠️ 检测到疑似广告内容，这条消息没有转发（{}/{limit}）。\n继续发送将被限制私聊中转。",
            outcome.count
        ),
        None,
    );
}

/// 被拉黑用户的提示：按 notify_blocked_every_secs 限频。
fn notice_blocked(
    ctx: &Arc<Ctx>,
    message: &IncomingMessage,
    user_id: i64,
    config: &Config,
    reason: &str,
) {
    let at = now();
    let last = ctx.store.last_notice(user_id);
    if at.saturating_sub(last) < config.notify_blocked_every_secs {
        return;
    }
    if let Err(error) = ctx.store.set_last_notice(user_id, at) {
        ctx.host()
            .warn(&format!("bidibot: 记录提示时间失败: {error}"));
    }
    ctx.debug(&format!(
        "bidibot: 用户 {user_id} 已被拉黑（{reason}），本条只提示、不转发"
    ));
    ctx.send(
        message.chat_id,
        "🔕 你的消息已停止转发。\n基础命令仍然可用：/bidihelp",
        None,
    );
}

/// 取用户信息；TDLib 查不到（离线 / 对方注销）就退回本地档案里记的昵称与用户名。
fn user_or_profile(ctx: &Arc<Ctx>, user_id: i64) -> UserInfo {
    if let Some(info) = ctx.messenger.user_info(user_id) {
        return info;
    }
    let profile = ctx.store.profile(user_id);
    UserInfo {
        user_id,
        name: profile
            .as_ref()
            .map(|profile| profile.name.clone())
            .unwrap_or_default(),
        username: profile.and_then(|profile| profile.username),
        ..UserInfo::default()
    }
}

/// 重发名片（/bidicard）：话题存在时用，返回是否发出去了。
///
/// 语言取档案里记的最后一次检测值；翻译状态未知时卡片只写主人语言，不瞎标。
pub fn resend_card(ctx: &Arc<Ctx>, user_id: i64, thread: i64) -> bool {
    let config = ctx.config();
    let info = user_or_profile(ctx, user_id);
    send_topic_card(ctx, &info, &config, thread) != 0
}

/// 发名片：图像卡片 → 头像 + 文字 caption → 纯文字，逐级降级。
///
/// html2img 是**软依赖**：没装 / 渲染失败都不影响中转，只是退到下一档。
/// 返回名片消息 id（拿不到就是 0）。
fn send_topic_card(ctx: &Arc<Ctx>, info: &UserInfo, config: &Config, thread: i64) -> i64 {
    // 头像：下载一次；失败就当没有（卡片用首字母占位）。每一步都留痕，方便查为什么没头像。
    let avatar_path = match info.photo_file_id {
        Some(file_id) => {
            ctx.debug(&format!(
                "bidibot: 用户 {} 有头像 file_id={file_id}，开始下载",
                info.user_id
            ));
            ctx.messenger.download_file(file_id, config.send_timeout())
        }
        None => {
            ctx.debug(&format!(
                "bidibot: 用户 {} 的 TDLib 对象里没有 profile_photo，名片用首字母占位",
                info.user_id
            ));
            None
        }
    };
    let avatar_uri = avatar_path
        .as_deref()
        .map(crate::render::avatar_data_uri)
        .unwrap_or_default();
    if avatar_path.is_some() && avatar_uri.is_empty() {
        ctx.debug(&format!(
            "bidibot: 用户 {} 的头像读不出来（文件为空 / 不是 jpeg-png-webp），用首字母占位",
            info.user_id
        ));
    } else if !avatar_uri.is_empty() {
        ctx.debug(&format!(
            "bidibot: 用户 {} 的头像已内嵌（{} 字节 data URI）",
            info.user_id,
            avatar_uri.len()
        ));
    }
    let data =
        crate::render::CardData::build(info, crate::render::CardContext { avatar: avatar_uri });

    // 1) 有 html2img：渲染图片卡片（宽 card_width，2 倍图，jpg）。
    if config.card_image
        && let Some(image) = crate::render::render_card(&ctx.host(), &data, config.card_width)
    {
        match ctx.messenger.send_photo_in_topic(
            config.group_id,
            &image,
            Some(thread),
            None,
            None,
            config.send_timeout(),
        ) {
            Ok(sent) => {
                ctx.debug(&format!(
                    "bidibot: 名片用 html2img 渲染成图片（{}）",
                    image.display()
                ));
                return sent.message_id;
            }
            Err(error) => ctx.debug(&format!("bidibot: 发名片图失败，降级文字版: {error}")),
        }
    }

    // 2) 没有渲染器但有头像：头像照片 + 文字版 caption。
    if let Some(path) = avatar_path.as_deref()
        && let Ok(sent) = ctx.messenger.send_photo_in_topic(
            config.group_id,
            path,
            Some(thread),
            Some(&data.fallback_text()),
            None,
            config.send_timeout(),
        )
    {
        return sent.message_id;
    }

    // 3) 纯文字。
    ctx.messenger
        .try_send_text_in_topic(
            config.group_id,
            &data.fallback_text(),
            Some(thread),
            None,
            config.send_timeout(),
        )
        .map(|sent| sent.message_id)
        .unwrap_or(0)
}

/// 顺手更新用户档案（语言、昵称都可能变）。
fn remember_profile(ctx: &Arc<Ctx>, user_id: i64, info: &UserInfo) {
    let mut profile = ctx.store.profile(user_id).unwrap_or(Profile {
        user_id,
        name: String::new(),
        username: None,
        lang: None,
        first_seen: now(),
        base_message_id: 0,
    });
    profile.name = info.name.clone();
    profile.username = info.username.clone();
    if let Err(error) = ctx.store.put_profile(profile) {
        ctx.host()
            .error(&format!("bidibot: 保存用户档案失败: {error}"));
    }
}

// ---------------------------------------------------------------- 纯函数（可单测）

/// 话题名：昵称优先，空则"用户 <id>"；去掉换行与控制字符，按字符数截断。
pub fn topic_name(info: &UserInfo, max: usize, user_id: i64) -> String {
    // 昵称优先；没有昵称用 @用户名；都没有就用 id，保证话题一定有个可认的名字。
    let base = if !info.name.trim().is_empty() {
        info.name.clone()
    } else if let Some(username) = info.username.as_deref().filter(|name| !name.is_empty()) {
        format!("@{username}")
    } else {
        format!("用户 {user_id}")
    };
    let cleaned: String = base
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let cleaned = cleaned.trim();
    let max = max.max(1);
    if cleaned.chars().count() <= max {
        return cleaned.to_owned();
    }
    let mut out: String = cleaned.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, username: Option<&str>) -> UserInfo {
        UserInfo {
            user_id: 123456789,
            name: name.to_owned(),
            username: username.map(str::to_owned),
            is_bot: false,
            photo_file_id: None,
        }
    }

    #[test]
    fn topic_name_prefers_the_nickname() {
        assert_eq!(topic_name(&info("张三", None), 28, 1), "张三");
    }

    #[test]
    fn topic_name_falls_back_to_username_then_user_id() {
        assert_eq!(topic_name(&info("", Some("zhangsan")), 28, 42), "@zhangsan");
        assert_eq!(topic_name(&info("", None), 28, 42), "用户 42");
        assert_eq!(
            topic_name(&info("   ", None), 28, 42),
            "用户 42",
            "空白昵称也要有名字"
        );
    }

    #[test]
    fn topic_name_is_truncated_and_cleaned() {
        let long = "很".repeat(50);
        let name = topic_name(&info(&long, None), 10, 1);
        assert_eq!(name.chars().count(), 11, "10 个字符 + 省略号");
        assert!(name.ends_with('…'));

        let messy = topic_name(&info("张\n三\t", None), 28, 1);
        assert_eq!(messy, "张 三", "换行/制表要压成空格并 trim");
    }

    #[test]
    fn only_real_topic_errors_trigger_rebuild() {
        assert!(!topic_gone(&MessengerError::Timeout));
        assert!(!topic_gone(&MessengerError::Host("boom".to_owned())));
        assert!(!topic_gone(&MessengerError::Op {
            code: "timeout".to_owned(),
            message: String::new(),
        }));
        assert!(topic_gone(&MessengerError::Op {
            code: "tdlib".to_owned(),
            message: "TOPIC_DELETED".to_owned(),
        }));
        assert!(topic_gone(&MessengerError::Op {
            code: "send_failed".to_owned(),
            message: "topic not found".to_owned(),
        }));
    }
}
