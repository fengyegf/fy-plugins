//! `/help` 联动：把本插件的命令登记进**宿主的**命令表。
//!
//! 命令表的主人是宿主，不是 `/help` 插件：宿主在根上预置**进程级共享服务**
//! `fuyubot.host.commands`，方法 `register` 的入参是
//! `{"name":…,"description":…,"role":…,"scopes":[…],"hidden":…}`；成功回
//! `{"ok":true,"name":…,"plugin":…}`，坏入参回 `{"error":"…"}`（见插件文档
//! 「命令表与 /help」）。`plugin` 由宿主按调用者栈判定，插件自报不了、也不需要报。
//!
//! `/help` 只是**读取方**：它调同一张表的 `list`，按 `role` / `scopes` / `hidden`
//! 过滤后渲染。所以本插件不链接 help、也不 import 它的任何东西，只认服务名、
//! 方法名和这份 JSON 协议；旧名 `fuyubot.help.commands` 是宿主的兼容别名，新代码
//! 不用它，也不需要为它写 `soft_inject`（宿主服务在插件启动前就在）。
//!
//! 登记失败只记一条 warn：`/musiccard` 照常工作，只是不进 `/help`
//! （也不受宿主命令闸门保护）。
//!
//! `role` / `scopes` / `hidden` 必须与**代码里实际的权限判断**一致，别借登记去
//! 收紧或放宽执行权限——执行权限仍由本插件自己的命令匹配决定。

use fuyubot_plugin_api::{HostApi, HostExt};

/// 宿主命令表的服务名（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.host.commands";

/// 一条要登记进宿主命令表的命令；字段与 `register` 的入参一一对应。
///
/// `role` / `scopes` / `hidden` 写的是本插件**真实的**权限口径，见模块注释。
struct Command {
    /// 不带 `/` 与 `@Bot`：`/help` 渲染时自己加前缀。
    name: &'static str,
    /// `/help` 里的那行说明。
    description: &'static str,
    /// 最低角色：本插件的命令没有角色判断 → `user`。
    role: &'static str,
    /// 允许出现的会话：私聊 + 群（与宿主的默认值一致）。
    scopes: &'static [&'static str],
    /// `true` 只是不进 `/help`，不是授权；用户命令用 `false`。
    hidden: bool,
}

/// 本插件登记的命令。
const COMMANDS: &[Command] = &[Command {
    name: "musiccard",
    description: "生成网易云音乐卡片：歌名、链接或歌曲 ID",
    role: "user",
    scopes: &["private", "group"],
    hidden: false,
}];

/// 把所有命令登记进宿主命令表。
///
/// 失败只记日志、不返回错误：登记失败不影响 `/musiccard` 本身。
pub(crate) fn register(host: &HostApi) {
    for command in COMMANDS {
        let payload = payload(command);
        match host.call_service(SERVICE, "register", &payload) {
            Ok(reply) if reply_ok(&reply) => {
                host.info(&format!(
                    "musiccard: 已向宿主命令表登记 /{}：{reply}",
                    command.name
                ));
            }
            Ok(reply) => {
                host.warn(&format!(
                    "musiccard: 宿主拒绝登记命令 /{}（{SERVICE}.register 回 {reply}），继续跑",
                    command.name
                ));
            }
            Err(error) => {
                // AbiError 不细分「服务不在」和「方法不在」，日志里把两个名字都打出来。
                host.warn(&format!(
                    "musiccard: 登记命令 /{} 到宿主命令表失败（{SERVICE}.register：{error}），/help 将少一行",
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

/// 宿主成功回 `{"ok":true,"name":…,"plugin":…}`，失败回 `{"error":"…"}`——两者都是 `Ok`。
fn reply_ok(reply: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(reply)
        .ok()
        .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_matches_the_service_protocol() {
        let value: serde_json::Value = serde_json::from_str(&payload(&COMMANDS[0])).unwrap();
        // 登记字段必须齐全：名字、说明、角色、会话、是否隐藏。
        assert_eq!(value["name"], "musiccard");
        assert_eq!(value["role"], "user");
        assert_eq!(value["scopes"], serde_json::json!(["private", "group"]));
        assert_eq!(value["hidden"], false);
        assert!(
            value["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        // `plugin` 由宿主判定，插件自报不了，也不该塞进入参。
        assert!(value.get("plugin").is_none());
    }

    #[test]
    fn only_explicit_ok_counts_as_success() {
        assert!(reply_ok(
            r#"{"ok":true,"name":"musiccard","plugin":"musiccard"}"#
        ));
        assert!(!reply_ok(r#"{"error":"缺 name 或为空"}"#));
        assert!(!reply_ok(""));
    }

    #[test]
    fn every_command_has_plausible_metadata() {
        for command in COMMANDS {
            assert!(
                !command.name.is_empty() && !command.name.starts_with('/'),
                "{}",
                command.name
            );
            assert!(!command.description.is_empty(), "{}", command.name);
            assert!(
                matches!(command.role, "user" | "chat_admin" | "admin" | "owner"),
                "{}",
                command.name
            );
            assert!(!command.scopes.is_empty(), "{}", command.name);
            for scope in command.scopes {
                assert!(
                    matches!(*scope, "private" | "group" | "channel"),
                    "{}",
                    command.name
                );
            }
        }
    }
}
