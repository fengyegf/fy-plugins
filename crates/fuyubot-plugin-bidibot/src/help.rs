//! 命令表登记：把 `/bidi*` 登记进**宿主**的命令表。
//!
//! 命令表的主人是宿主（进程级共享服务 `fuyubot.host.commands`，契约名，不是插件名），
//! 方法 `register` 的入参是
//!
//! ```json
//! {"name":"bidihelp","description":"…","role":"user","scopes":["group","private"],"hidden":false}
//! ```
//!
//! 成功回 `{"ok":true,"name":…,"plugin":…}`（`plugin` 由宿主按调用者栈判定，插件自报
//! 不了也不需要报），坏入参回 `{"error":"…"}`。内置 `/help` 只是这张表的读取方，
//! 本插件既不链接 help、也不 import 它的任何东西，只认服务名、方法名和这份 JSON。
//!
//! 登记是**可选**联动：服务理论上总在，万一没有（老宿主 / 裁剪版）`call_service` 会
//! 失败，这时只记一条 warn——`/help` 少一行不影响命令本身照常工作。
//!
//! v0.2 起是**群 + 话题**模式（docs/bidibot-topics-design.md 第七节）：
//! - 管理命令只在论坛群里执行 → `role = "owner"`、`scopes = ["group"]`；
//! - `/bidihelp` 私聊和群里都要能查到 → `role = "user"`、双 scope；
//! - **不再登记** `/bidiclaim`（认领机制已删除，主人由配置决定）；
//! - 欢迎语命令叫 `/bidistart`：`/start` 是宿主内建 start 插件的命令，同名登记会被拒
//!   （Telegram 内建命令，跨插件重名会被宿主拒绝；欢迎语写在 `/bidihelp` 里）。
//!
//! `role` / `scopes` / `hidden` 只是**声明**，必须与代码里实际的权限判断一致：
//! 执行权限由 [`crate::cmd::handle_group`] 的"配置的群 + owner_id（0 = 管理命令不可用）"决定，
//! 登记不会放宽也不会收紧它。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

/// 宿主预置的命令表服务名（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.host.commands";

/// 一条登记项。字段含义见插件文档「命令表与 /help」。
struct Entry {
    /// 命令名，不带 `/`，只允许 `a-z0-9_`（宿主统一转小写）。
    name: &'static str,
    /// `/help` 里那行说明。
    description: &'static str,
    /// 最低角色：`user` / `chat_admin` / `admin` / `owner`。
    role: &'static str,
    /// 允许出现的会话。管理命令只在群里；帮助私聊 / 群都要能看到。
    scopes: &'static [&'static str],
    /// 只影响是否进 `/help`，不是授权。
    hidden: bool,
}

/// 本插件登记进宿主命令表的条目。
const COMMANDS: &[Entry] = &[
    Entry {
        name: "bidistart",
        description: "bidibot 欢迎语：双向私聊中转怎么用",
        role: "user",
        scopes: &["group", "private"],
        hidden: false,
    },
    Entry {
        name: "bidihelp",
        description: "bidibot 私聊代收：用法与命令帮助",
        role: "user",
        scopes: &["group", "private"],
        hidden: false,
    },
    Entry {
        name: "bidicard",
        description: "在当前话题里重发这个用户的名片（图像卡片 / 文字版）",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidistatus",
        description: "运行状态：群 / 管理员 / 翻译 / 广告 / 话题数",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidilist",
        description: "拉黑列表（前 20 条）",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidiban",
        description: "拉黑用户：在该用户的话题里直接发，或用 /bidiban <id> 指定",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidiunban",
        description: "解除拉黑：在该用户的话题里直接发，或用 /bidiunban <id> 指定",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidiwarn",
        description: "查看某用户的广告警告数：/bidiwarn [id]",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidiclear",
        description: "清零某用户的广告警告：/bidiclear [id]",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidiword",
        description: "维护广告关键词：/bidiword add|del|list [词]",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
    Entry {
        name: "bidisay",
        description: "以 bot 身份主动给用户发一条：/bidisay [id] <文字>",
        role: "owner",
        scopes: &["group"],
        hidden: false,
    },
];

/// 把所有命令登记进宿主命令表。
///
/// 失败只记日志、不返回错误：这是可选联动，插件自己的命令照常工作。
pub(crate) fn register(host: &HostApi) {
    for entry in COMMANDS {
        let payload = payload(entry);
        match host.call_service(SERVICE, "register", &payload) {
            Ok(reply) if reply_ok(&reply) => {
                host.info(&format!(
                    "bidibot: 已向宿主命令表登记 /{}：{reply}",
                    entry.name
                ));
            }
            Ok(reply) => {
                host.warn(&format!(
                    "bidibot: 宿主命令表拒绝登记 /{}（{SERVICE}.register 回 {reply}），继续跑",
                    entry.name
                ));
            }
            Err(error) => {
                // AbiError 不细分「服务不在」和「方法不在」，日志里把两个名字都打出来。
                host.warn(&format!(
                    "bidibot: 登记命令 /{} 到宿主命令表失败（{SERVICE}.register：{error}），/help 将少一行",
                    entry.name
                ));
            }
        }
    }
}

/// `register` 的入参：名字 + 说明 + 能力过滤用的三个字段。
/// 抽成纯函数，单测不必造 `HostApi`。
fn payload(entry: &Entry) -> String {
    json!({
        "name": entry.name,
        "description": entry.description,
        "role": entry.role,
        "scopes": entry.scopes,
        "hidden": entry.hidden,
    })
    .to_string()
}

/// 提供方成功回 `{"ok":true,…}`，失败回 `{"error":"…"}`——两者都是 `Ok(String)`。
fn reply_ok(reply: &str) -> bool {
    serde_json::from_str::<Value>(reply)
        .ok()
        .and_then(|value| value.get("ok").and_then(Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> &'static Entry {
        COMMANDS
            .iter()
            .find(|entry| entry.name == name)
            .expect("命令必须登记")
    }

    #[test]
    fn payload_matches_the_service_protocol() {
        let value: Value =
            serde_json::from_str(&payload(entry("bidihelp"))).expect("入参必须是合法 JSON");
        assert_eq!(value["name"], "bidihelp");
        assert_eq!(value["description"], entry("bidihelp").description);
        assert_eq!(value["role"], "user");
        assert_eq!(value["scopes"], json!(["group", "private"]));
        assert_eq!(value["hidden"], false);
        // 不许漏字段：宿主按这四个键过滤，缺 role / scopes 会被当默认值。
        for key in ["name", "description", "role", "scopes", "hidden"] {
            assert!(value.get(key).is_some(), "缺字段 {key}: {value}");
        }
    }

    #[test]
    fn only_explicit_ok_counts_as_success() {
        assert!(reply_ok(
            r#"{"ok":true,"name":"bidihelp","plugin":"bidibot"}"#
        ));
        assert!(!reply_ok(r#"{"error":"缺 name 或为空"}"#));
        assert!(!reply_ok(r#"{"ok":false}"#));
        assert!(!reply_ok("not json"));
        assert!(!reply_ok(""));
    }

    #[test]
    fn every_command_has_a_plain_name_and_sane_registration() {
        let roles = ["user", "chat_admin", "admin", "owner"];
        let scopes = ["private", "group", "channel"];
        let mut seen = std::collections::BTreeSet::new();
        for entry in COMMANDS {
            assert!(!entry.name.is_empty(), "命令名为空");
            assert!(!entry.name.starts_with('/'), "{} 不该带 /", entry.name);
            assert!(
                entry
                    .name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
                "命令名只允许 a-z0-9_：{}",
                entry.name
            );
            assert!(seen.insert(entry.name), "命令名重复：{}", entry.name);
            assert!(!entry.description.is_empty(), "{}", entry.name);
            assert!(roles.contains(&entry.role), "未知 role：{}", entry.role);
            assert!(!entry.scopes.is_empty(), "{}", entry.name);
            for scope in entry.scopes {
                assert!(scopes.contains(scope), "未知 scope：{scope}");
            }
            assert!(!entry.hidden, "{} 应当出现在 /help 里", entry.name);
        }
    }

    #[test]
    fn management_commands_are_owner_role_and_group_only() {
        for entry in COMMANDS.iter().filter(|entry| entry.role == "owner") {
            // 管理入口只在论坛群里：登记到私聊/群外场景会出现"看得见、用不了"。
            assert_eq!(entry.scopes, ["group"], "{}", entry.name);
        }
        // 普通用户能用的两条：欢迎语与帮助，私聊和群都要能查到。
        for name in ["bidistart", "bidihelp"] {
            let entry = entry(name);
            assert_eq!(entry.role, "user", "{name}");
            assert_eq!(entry.scopes, ["group", "private"], "{name}");
        }
    }

    #[test]
    fn removed_commands_are_not_registered() {
        // v0.2 删掉了 /bidiclaim（主人由配置决定），/start 是宿主内建命令。
        for name in ["bidiclaim", "start"] {
            assert!(
                !COMMANDS.iter().any(|entry| entry.name == name),
                "{name} 不该再登记"
            );
        }
    }
}
