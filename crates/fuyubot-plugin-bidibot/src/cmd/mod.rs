//! 命令层：把一条消息翻译成"谁的命令"，把结果回在同一条话题里。
//!
//! v0.2 起是**论坛群 + 话题**模式：
//! - 用户私聊 bot 只走基础命令（`/bidistart`、`/bidihelp`），其余私聊消息由 relay 转发进群；
//! - 管理命令全部在配置的论坛群里执行，**回复发在同一条话题里**（不要跳去 General）；
//! - 没有 `/bidiclaim`：谁是主人由配置 `owner_id` 决定；`owner_id = 0` 时管理命令
//!   一律不可用（群里还有别人，默认放权太危险）。
//!
//! 一条纪律：**只认自己的命令，而且认得的命令一定要吃掉**。relay 先调这里，返回
//! `false` 的消息才继续往下走；群里返回 `false` 的命令会被当成"主人的发言"发回给
//! 用户——把 `/bidiban` 原样发给陌生人，比什么都不做还糟。

pub mod owner;
pub mod parser;

use std::sync::Arc;

use fuyubot_plugin_api::HostExt as _;

use crate::core::config::Config;
use crate::host::{Ctx, IncomingMessage};

/// 非管理员在群里执行管理命令时的回复。
///
/// 不确认"这条命令存在"、不回显配置、也不提主人是谁：陌生人能拿到的信息越少越好
/// （宿主命令闸门通常也会先拦一道，走到这里的属于兜底）。
pub(crate) const NOT_ALLOWED: &str = "🔒 该命令不可用。";

/// 回复触发本命令的那条消息。
///
/// 必须带上 `message.topic_thread_id`：命令要回在**哪条话题里发的就回哪条话题**，
/// 否则论坛群里所有命令的答复都会堆到 General，话题模式和私聊就没区别了。
///
/// 发送失败只记一条 warn（和 [`crate::host::Ctx::send`] 一样不重试）：命令不是
/// 关键路径，重试反而可能让同一条答复出现两次。
pub(crate) fn reply(ctx: &Ctx, message: &IncomingMessage, text: &str) {
    let timeout = ctx.config().send_timeout();
    if let Err(error) = ctx.messenger.send_text_in_topic(
        message.chat_id,
        text,
        message.topic_thread_id,
        Some(message.id),
        timeout,
    ) {
        ctx.host().warn(&format!(
            "bidibot: 回复命令到 chat {}（话题 {:?}）失败: {error}",
            message.chat_id, message.topic_thread_id
        ));
    }
}

/// 私聊里的基础命令（`/bidistart`、`/bidihelp`）。返回 `true` 表示这条消息已处理。
///
/// 不叫 `/start`：那是宿主内建 start 插件已经登记的命令，同名登记会被宿主拒掉，
/// 事件也会被它先吃掉，插件根本收不到。
///
/// 私聊**不再有管理入口**：v0.2 的主人入口是群里的管理命令。这里认识的命令只有两个
/// 面向陌生人的（欢迎语 / 帮助），别的一律返回 `false`，让 relay 按普通消息转发。
pub fn handle_user(ctx: &Arc<Ctx>, message: &IncomingMessage) -> bool {
    if !message.is_private || message.sender_user_id.is_none() {
        return false;
    }
    let config = ctx.config();
    let Some(command) = parser::parse(&message.text, &config.prefixes) else {
        return false;
    };
    match command.name.as_str() {
        "bidihelp" => {
            reply(ctx, message, &help_text(false));
            true
        }
        "bidistart" => {
            reply(ctx, message, &start_text());
            true
        }
        _ => false,
    }
}

/// 群 / 话题里的命令。`topic_user` = 这条消息所在话题对应的用户（General 话题为 `None`）。
///
/// 目标用户的解析顺序见 [`owner`]：命令里的显式 id 优先，其次才是 `topic_user`。
/// 管理权限见 [`can_manage`]：`owner_id` 没配时**任何管理命令都不可用**。
pub fn handle_group(ctx: &Arc<Ctx>, message: &IncomingMessage, topic_user: Option<i64>) -> bool {
    let config = ctx.config();
    // 防御一次：relay 已经按 chat_id 过滤过，这里再确认只认配置里的那个群。
    // group_id == 0（没配）时任何群都不处理——群模式没有默认群可猜。
    if message.is_private || config.group_id == 0 || message.chat_id != config.group_id {
        return false;
    }
    let Some(command) = parser::parse(&message.text, &config.prefixes) else {
        return false;
    };
    let manager = can_manage(&config, message.sender_user_id);

    if command.name == "bidihelp" {
        reply(ctx, message, &help_text(manager));
        return true;
    }
    if owner::owns(&command.name) {
        // 认得就吃掉：返回 false 会被 relay 当成主人的发言发回给话题对应用户。
        if !manager {
            ctx.host().warn(&format!(
                "bidibot: 发送者 {:?} 在群里尝试管理命令 /{}，已拒绝（owner_id = {}）",
                message.sender_user_id, command.name, config.owner_id
            ));
            reply(ctx, message, NOT_ALLOWED);
            return true;
        }
        return owner::handle(ctx, message, &command, topic_user);
    }
    false
}

/// 这条消息的发送者有没有管理权限。
///
/// **严格口径**（v0.2 定稿）：
/// - `owner_id != 0`：只算这一个人；群里其他人发管理命令一律拒绝。
/// - `owner_id == 0`：管理命令**全部不可用**。以前想过"不校验 = 群成员都能管理"，
///   但群里还有别人，默认放权等于把拉黑 / 主动私聊任意用户的能力送给所有群成员。
/// - 匿名管理员（`sender_user_id` 为 `None`）没有可比对的身份，永远不算。
///
/// 抽成纯函数：单测只要一个 [`Config`]，不必造 `HostApi`。
pub fn can_manage(config: &Config, sender_user_id: Option<i64>) -> bool {
    config.owner_id != 0 && sender_user_id == Some(config.owner_id)
}

/// `/bidihelp` 的正文。`with_owner` = 发给管理员看，额外列出管理命令。
///
/// 抽成纯函数：文案本身就是"谁能做什么"的契约，单测直接比对内容；
/// 同时这里也是**安全边界**的一部分——发给陌生人的那一版不许出现任何管理命令、
/// 不许出现主人 id / 配置路径。
pub fn help_text(with_owner: bool) -> String {
    let mut out = String::from(concat!(
        "🤖 bidibot：这是双向bot\n\n",
        "你的消息会被转发到bot的管理者所配置的地方\n",
        "你如果不是bot的所有者你只能收到回复的消息\n\n",
        "可用命令：\n",
        "/bidistart  欢迎语（重新看一遍）\n",
        "/bidihelp   这条帮助",
    ));
    if with_owner {
        out.push_str(concat!(
            "\n\n",
            "管理命令（在该用户的话题里直接发；General 话题里要显式带 user id）：\n",
            "/bidistatus                  运行状态与计数\n",
            "/bidilist                    拉黑列表（前 20 条）\n",
            "/bidiban [id] [原因]         拉黑：不带 id 就拉黑当前话题的用户\n",
            "/bidiunban [id]              解除拉黑\n",
            "/bidiwarn [id]               查看某用户的警告数\n",
            "/bidiclear [id]              清零警告（不解除拉黑）\n",
            "/bidicard                    在当前话题里重发用户名片\n",
            "/bidiword add|del|list [词]  维护广告关键词\n",
            "/bidisay [id] <文字>         以 bot 身份主动给用户发一条（不带转发痕迹）",
        ));
    }
    out
}

/// `/bidistart` 的欢迎语。
///
/// 私聊里唯一能主动发给陌生人的两句话之一：只说"消息会转发到管理群 + 有 /bidihelp"，
/// 不透露群 id、不暴露主人身份。
pub fn start_text() -> String {
    String::from(concat!(
        "👋 你好，这里是私聊代收。\n\n",
        "直接把想说的话发给我，会转给管理员；管理员的回复也会以我的身份发回给你。\n",
        "广告内容会被拦下，严禁骚扰\n\n",
        "/bidihelp 查看命令。",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_for_strangers_lists_only_public_commands() {
        let text = help_text(false);
        assert!(text.contains("/bidihelp"), "{text}");
        assert!(text.contains("/bidistart"), "{text}");
        for management in [
            "/bidiban",
            "/bidiunban",
            "/bidilist",
            "/bidiwarn",
            "/bidiclear",
            "/bidilang",
            "/bidiword",
            "/bidistatus",
            "/bidisay",
            "/bidiclaim",
        ] {
            assert!(
                !text.contains(management),
                "陌生人帮助里不该有 {management}"
            );
        }
    }

    #[test]
    fn help_for_managers_lists_every_management_command() {
        let text = help_text(true);
        for command in [
            "/bidiban",
            "/bidiunban",
            "/bidilist",
            "/bidiwarn",
            "/bidiclear",
            "/bidiword",
            "/bidistatus",
            "/bidisay",
            "/bidicard",
        ] {
            assert!(text.contains(command), "管理帮助里缺 {command}");
        }
        assert!(!text.contains("/bidiclaim"), "认领命令已删除: {text}");
        // 话题模式的关键提示：不带 id 时用当前话题的用户。
        assert!(text.contains("不带 id"), "{text}");
        assert!(text.contains("General"), "{text}");
    }

    #[test]
    fn public_texts_never_leak_identifiers() {
        // 回归口径：发给非管理员的任何文案都不出现数字（可能是 id）、@ 用户名、
        // 配置路径。管理帮助里的 "[id]" 是占位符，不构成泄漏。
        for text in [help_text(false), start_text(), NOT_ALLOWED.to_owned()] {
            assert!(
                !text.chars().any(|ch| ch.is_ascii_digit()),
                "文案里不能有数字（可能是 id）: {text}"
            );
            assert!(!text.contains('@'), "文案里不能有用户名: {text}");
            assert!(!text.contains("toml"), "不应指导陌生人去改配置: {text}");
        }
    }

    #[test]
    fn start_text_points_at_help_and_not_at_claiming() {
        let text = start_text();
        assert!(text.ends_with("/bidihelp 查看命令。"), "{text}");
        assert!(!text.contains("/bidiclaim"), "{text}");
    }

    #[test]
    fn management_permission_follows_owner_id() {
        let mut config = Config::default();
        // owner_id == 0：严格口径——不信任任何发送者，管理命令一律不可用。
        assert!(!can_manage(&config, Some(1)));
        assert!(!can_manage(&config, None));
        // 配了 owner_id：只认这个人；匿名管理员没有可比对的身份，不算。
        config.owner_id = 42;
        assert!(can_manage(&config, Some(42)));
        assert!(!can_manage(&config, Some(43)));
        assert!(!can_manage(&config, None));
    }
}
