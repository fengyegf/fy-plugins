//! `/help` 联动：把本插件的命令登记进**宿主的命令表**。
//!
//! 命令表的主人是宿主（`fuyubot.host.commands`），不是某个 help 插件：登记的条目带
//! `role` / `scopes` / `hidden`，宿主按它们决定谁能看见、在哪看见；图片版 `/help`
//! 只是这张表的读取方（`list`），渲染与发回都由它自己做。
//!
//! 本插件不链接 help、也不 import 它的任何东西：只认服务名、方法名和这份 JSON 协议：
//!
//! - `register` 入参 `{"name":…, "description":…, "role":…, "scopes":[…], "hidden":…}`；
//! - 成功回 `{"ok":true,"name":…,"plugin":…}`，失败回 `{"error":"…"}`。
//!
//! 命令表由宿主预置（启动时就在），所以**不需要** `soft_inject` 去等 help；但登记仍然
//! 可能失败（跨插件重名、旧宿主没有这个服务），那也只记一条 warn：`/help` 少一行命令
//! 不影响 ppxfy 本身。
//!
//! ABI v6：条目归属由宿主按调用者栈判定（`current_plugin`），插件自报不了，也不需要报；
//! 旧的 `fuyubot.help.commands` 只是兼容别名，新代码不用。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

/// 宿主预置的命令表服务（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.host.commands";

/// 一条登记项。字段含义见插件文档「命令表与 /help」。
struct Entry {
    /// 命令名，不带 `/`，只允许 `a-z0-9_`（宿主统一转小写）。
    name: &'static str,
    /// `/help` 里那行说明。别名写进说明里，不单独占一行。
    description: &'static str,
    /// 最低角色：`user` / `chat_admin` / `admin` / `owner`。
    role: &'static str,
    /// 允许出现的会话：`private` / `group` / `channel`。
    scopes: &'static [&'static str],
    /// 只影响是否进 `/help`，**不是**授权。
    hidden: bool,
}

/// 本插件登记进宿主命令表的条目。
///
/// 四条命令都没有权限判断（匹配到前缀就执行），私聊与群组都能用，所以一律
/// `role = "user"`、`scopes = ["private", "group"]`、`hidden = false`——登记值必须
/// 和代码里实际的权限判断一致，不能借登记去收紧 / 放宽执行权限。
///
/// 只登记主命令名；历史长名（`downloadcommand` / `galgamecommand`）与短别名
/// （`/d` / `/p`）写进说明里。
const COMMANDS: &[Entry] = &[
    Entry {
        name: "download",
        description: "下载链接、解压并打包上传（别名 /d）",
        role: "user",
        scopes: &["private", "group"],
        hidden: false,
    },
    Entry {
        name: "galgame",
        description: "解析 DMM / DLSite 作品页生成分享消息（别名 /p）",
        role: "user",
        scopes: &["private", "group"],
        hidden: false,
    },
    Entry {
        name: "cancel",
        description: "取消当前聊天的下载任务",
        role: "user",
        scopes: &["private", "group"],
        hidden: false,
    },
    Entry {
        name: "clearcache",
        description: "清理没有任务登记的下载缓存",
        role: "user",
        scopes: &["private", "group"],
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
                    "ppxfy: 已向宿主命令表登记 /{}：{reply}",
                    entry.name
                ));
            }
            Ok(reply) => {
                host.warn(&format!(
                    "ppxfy: 宿主命令表拒绝登记 /{}（{SERVICE}.register 回 {reply}），继续跑",
                    entry.name
                ));
            }
            Err(error) => {
                // AbiError 不细分「服务不在」和「方法不在」，日志里把两个名字都打出来。
                host.warn(&format!(
                    "ppxfy: 登记命令 /{} 到宿主命令表失败（{SERVICE}.register：{error}），/help 将少一行",
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
        // 四个能力字段一个都不能少：role / scopes / hidden 是宿主过滤的依据。
        assert_eq!(value["name"], "download");
        assert_eq!(value["description"], entry.description);
        assert_eq!(value["role"], "user");
        assert_eq!(value["scopes"], json!(["private", "group"]));
        assert_eq!(value["hidden"], false);
        for key in ["name", "description", "role", "scopes", "hidden"] {
            assert!(value.get(key).is_some(), "缺字段 {key}");
        }
    }

    #[test]
    fn every_entry_carries_the_four_fields() {
        for entry in COMMANDS {
            let value: Value = serde_json::from_str(&payload(entry)).unwrap();
            assert!(value["name"].is_string(), "{}", entry.name);
            assert!(value["description"].is_string(), "{}", entry.name);
            assert!(
                matches!(
                    value["role"].as_str(),
                    Some("user" | "chat_admin" | "admin" | "owner")
                ),
                "{} 的 role 非法",
                entry.name
            );
            assert!(
                value["scopes"]
                    .as_array()
                    .is_some_and(|scopes| !scopes.is_empty()),
                "{} 没有 scopes",
                entry.name
            );
            assert!(value["hidden"].is_boolean(), "{}", entry.name);
        }
    }

    #[test]
    fn only_explicit_ok_counts_as_success() {
        assert!(reply_ok(
            r#"{"ok":true,"name":"download","plugin":"ppxfy"}"#
        ));
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
            assert!(!entry.description.is_empty(), "{}", entry.name);
            assert!(!entry.scopes.is_empty(), "{} 没有 scopes", entry.name);
            for scope in entry.scopes {
                assert!(
                    matches!(*scope, "private" | "group" | "channel"),
                    "{} 的 scope 非法: {scope}",
                    entry.name
                );
            }
        }
    }
}
