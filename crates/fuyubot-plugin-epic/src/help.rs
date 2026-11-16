//! `/help` 联动：把本插件的命令登记进**宿主的命令表**。
//!
//! 命令表的主人是宿主（`fuyubot.host.commands`），不是某个 help 插件：登记的条目
//! 带 `role` / `scopes` / `hidden`，宿主按它们决定谁能看见、在哪看见；图片版
//! `/help` 只是这张表的读取方（`list`）。
//!
//! 本插件不链接 help、也不 import 它的任何东西：只认服务名、方法名和这份 JSON 协议。
//!
//! 联动是**可选**的：宿主服务启动时就在，但登记失败（例如跨插件同名）也只记一条
//! warn——`/help` 少一行命令不影响 `/epic` 本身。
//!
//! ABI v6：条目归属由宿主按调用者栈判定（`current_plugin`），插件自报不了，也不需要
//! 报；旧的 `fuyubot.help.commands` 只是兼容别名，新代码不用。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

/// 宿主预置的命令表服务（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.host.commands";

/// 一条登记项。字段含义见插件文档「命令表与 /help」。
struct Entry {
    /// 命令名，不带 `/`，只允许 `a-z0-9_`（宿主统一转小写）。
    name: &'static str,
    /// `/help` 里那行说明。
    description: &'static str,
    /// 最低角色：`user` / `chat_admin` / `admin` / `owner`。
    role: &'static str,
    /// 允许出现的会话：`private` / `group` / `channel`。
    scopes: &'static [&'static str],
    /// 只影响是否进 `/help`，不是授权。
    hidden: bool,
}

/// 本插件登记进宿主命令表的条目。
///
/// `/epic` 对所有人开放，私聊与群组都能用；频道里没有「回命令」的会话上下文、
/// 出图成本也高，所以不登记 `channel`。别名 `/epicfree` 写进说明里，不单独占一行。
const COMMANDS: &[Entry] = &[Entry {
    name: "epic",
    description: "Epic 当前免费 / 即将免费的游戏卡片（别名 /epicfree）",
    role: "user",
    scopes: &["private", "group"],
    hidden: false,
}];

/// 把所有命令登记进宿主命令表。
///
/// 失败只记日志、不返回错误：这是可选联动，插件自己的命令照常工作。
pub(crate) fn register(host: &HostApi) {
    for entry in COMMANDS {
        let payload = payload(entry);
        match host.call_service(SERVICE, "register", &payload) {
            Ok(reply) if reply_ok(&reply) => {
                host.info(&format!(
                    "epic: 已向宿主命令表登记 /{}：{reply}",
                    entry.name
                ));
            }
            Ok(reply) => {
                host.warn(&format!(
                    "epic: 宿主命令表拒绝登记 /{}（{SERVICE}.register 回 {reply}），继续跑",
                    entry.name
                ));
            }
            Err(error) => {
                // AbiError 不细分「服务不在」和「方法不在」，日志里把两个名字都打出来。
                host.warn(&format!(
                    "epic: 登记命令 /{} 到宿主命令表失败（{SERVICE}.register：{error}），/help 将少一行",
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

/// 提供方成功回 `{"ok":true,"name":…,"plugin":…}`，失败回 `{"error":"…"}`——两者都是 `Ok(String)`。
fn reply_ok(reply: &str) -> bool {
    serde_json::from_str::<Value>(reply)
        .ok()
        .and_then(|value| value.get("ok").and_then(Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_matches_the_service_protocol() {
        let entry = &COMMANDS[0];
        let value: Value = serde_json::from_str(&payload(entry)).unwrap();
        assert_eq!(value["name"], "epic");
        assert_eq!(value["description"], entry.description);
        assert_eq!(value["role"], "user");
        assert_eq!(value["scopes"], json!(["private", "group"]));
        assert_eq!(value["hidden"], false);
    }

    #[test]
    fn only_explicit_ok_counts_as_success() {
        assert!(reply_ok(r#"{"ok":true,"name":"epic","plugin":"epic"}"#));
        assert!(!reply_ok(r#"{"error":"缺 name 或为空"}"#));
        assert!(!reply_ok(""));
    }

    #[test]
    fn every_command_is_registerable() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in COMMANDS {
            assert!(
                !entry.name.is_empty() && !entry.name.starts_with('/'),
                "{}",
                entry.name
            );
            assert!(
                entry
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{} 只允许 a-z0-9_",
                entry.name
            );
            assert!(seen.insert(entry.name), "命令名重复: {}", entry.name);
            assert!(
                matches!(entry.role, "user" | "chat_admin" | "admin" | "owner"),
                "{} 的 role 非法: {}",
                entry.name,
                entry.role
            );
            assert!(!entry.scopes.is_empty(), "{} 没有 scopes", entry.name);
            for scope in entry.scopes {
                assert!(
                    matches!(*scope, "private" | "group" | "channel"),
                    "{} 的 scope 非法: {scope}",
                    entry.name
                );
            }
            assert!(!entry.description.is_empty(), "{}", entry.name);
        }
    }
}
