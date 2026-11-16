//! 命令表联动：把本插件的命令登记进**宿主**的命令表。
//!
//! FuyuBot 0.1.5 起命令表归宿主：进程级共享服务 `fuyubot.host.commands`（契约名，
//! 不是插件名），方法 `register` 的入参是
//!
//! ```json
//! {"name":"kfc","description":"…","role":"user","scopes":["private","group"],"hidden":false}
//! ```
//!
//! 成功回 `{"ok":true,"name":…,"plugin":…}`（`plugin` 由宿主按调用者栈判定，插件
//! 自报不了也不需要报），坏入参回 `{"error":"…"}`。
//!
//! 内置 `/help` 只是这张表的**读取方**（`fuyubot.host.commands.list`），命令表不再
//! 由它提供；所以本插件既不链接 help、也不 import 它的任何东西，更不需要为了它写
//! `soft_inject`——宿主预置的服务启动时就在。这里只认服务名、方法名和这份 JSON 协议。
//!
//! 登记是**可选**的：服务理论上总在，万一没有（老宿主 / 裁剪版）`call_service` 会失败，
//! 这时只记一条 warn：`/help` 少一行命令不影响 `/kfc` 本身。

use fuyubot_plugin_api::{HostApi, HostExt};

/// 宿主预置的命令表服务名（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.host.commands";

/// 一条要登记进宿主命令表的命令。
///
/// `role` / `scopes` / `hidden` 只是**声明**，必须与代码里实际的权限判断一致：
/// 登记不放宽也不收紧执行权限（执行仍由各插件自己的命令匹配决定）。
struct Command {
    /// 命令名，不带 `/`，只允许 `a-z0-9_`；跨插件重名会被宿主拒绝。
    name: &'static str,
    /// 一句话说明，`/help` 里就显示这个。
    description: &'static str,
    /// `user` / `chat_admin` / `admin` / `owner`，默认 `user`。
    role: &'static str,
    /// `private` / `group` / `channel`，默认 `private` + `group`。
    scopes: &'static [&'static str],
    /// `true` 只是不进 `/help`，**不是授权**。
    hidden: bool,
}

/// 本插件登记进命令表的命令。
///
/// `/kfc` 在代码里没有任何权限判断：谁都能发，私聊和群都能用，所以
/// `role = "user"`、`scopes = ["private", "group"]`、`hidden = false`。
const COMMANDS: &[Command] = &[Command {
    name: "kfc",
    description: "点个肯德基：随机一句疯话渲染成推特卡片",
    role: "user",
    scopes: &["private", "group"],
    hidden: false,
}];

/// 把所有命令登记进宿主的命令表。
///
/// 失败只记日志、不返回错误：这是可选联动，插件自己的命令照常工作。
pub(crate) fn register(host: &HostApi) {
    for command in COMMANDS {
        let payload = payload(command);
        match host.call_service(SERVICE, "register", &payload) {
            Ok(reply) if reply_ok(&reply) => {
                host.info(&format!(
                    "kfc: 已向宿主命令表登记 /{}：{reply}",
                    command.name
                ));
            }
            Ok(reply) => {
                host.warn(&format!(
                    "kfc: 宿主命令表拒绝登记 /{}（{SERVICE}.register 回 {reply}），继续跑",
                    command.name
                ));
            }
            Err(error) => {
                // AbiError 不细分「服务不在」和「方法不在」，日志里把两个名字都打出来。
                host.warn(&format!(
                    "kfc: 登记命令 /{} 到宿主命令表失败（{SERVICE}.register：{error}），/help 将少一行",
                    command.name
                ));
            }
        }
    }
}

/// `register` 的入参。抽成纯函数，单测不必造 `HostApi`。
fn payload(command: &Command) -> String {
    serde_json::json!({
        "name": command.name,
        "description": command.description,
        "role": command.role,
        "scopes": command.scopes,
        "hidden": command.hidden,
    })
    .to_string()
}

/// 提供方成功回 `{"ok":true,…}`，失败回 `{"error":"…"}`——两者都是 `Ok(String)`。
fn reply_ok(reply: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(reply)
        .ok()
        .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 登记入参的五个字段都要在：name / description / role / scopes / hidden。
    #[test]
    fn payload_matches_the_service_protocol() {
        let value: serde_json::Value = serde_json::from_str(&payload(&COMMANDS[0])).unwrap();
        assert_eq!(value["name"], "kfc");
        assert_eq!(value["description"], COMMANDS[0].description);
        assert_eq!(value["role"], "user");
        assert_eq!(value["scopes"], serde_json::json!(["private", "group"]));
        assert_eq!(value["hidden"], false);
        // 不许漏字段：宿主按这四个键过滤，缺 role / scopes 会被当默认值。
        for key in ["name", "description", "role", "scopes", "hidden"] {
            assert!(value.get(key).is_some(), "缺字段 {key}: {value}");
        }
    }

    #[test]
    fn only_explicit_ok_counts_as_success() {
        assert!(reply_ok(r#"{"ok":true}"#));
        assert!(reply_ok(r#"{"ok":true,"name":"kfc","plugin":"kfc"}"#));
        assert!(!reply_ok(r#"{"error":"缺 name 或为空"}"#));
        assert!(!reply_ok(""));
    }

    #[test]
    fn every_command_has_a_plain_name_and_sane_registration() {
        let roles = ["user", "chat_admin", "admin", "owner"];
        let scopes = ["private", "group", "channel"];
        for command in COMMANDS {
            assert!(!command.name.is_empty(), "命令名为空");
            assert!(!command.name.starts_with('/'), "{}", command.name);
            assert!(
                command
                    .name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
                "命令名只允许 a-z0-9_：{}",
                command.name
            );
            assert!(!command.description.is_empty(), "{}", command.name);
            assert!(roles.contains(&command.role), "未知 role：{}", command.role);
            assert!(!command.scopes.is_empty(), "{}", command.name);
            for scope in command.scopes {
                assert!(scopes.contains(scope), "未知 scope：{scope}");
            }
        }
    }
}
