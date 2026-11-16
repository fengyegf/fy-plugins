//! `/help` 联动：把本插件的命令登记进宿主的命令表。
//!
//! 命令表的主人是**宿主**，不是内置 `/help` 插件：宿主把命令表暴露成进程级共享服务
//! `fuyubot.host.commands`，方法 `register` 的入参是
//! `{"name":…,"description":…,"role":…,"scopes":[…],"hidden":false}`：成功回
//! `{"ok":true,"name":…,"plugin":…}`（`plugin` 由宿主按调用者栈判定，插件自报不了），
//! 坏入参 / 跨插件重名回 `{"error":…}`（见插件文档「命令表与 /help」）。
//!
//! 内置 `/help` 只是命令表的**读取方**：`fuyubot.host.commands.list` 之后按
//! `role` / `scopes` / `hidden` 过滤、按 `plugin` 分组渲染。所以只要这里登记得对，
//! `/help` 就能正确显示；本插件不链接 help、也不 import 它的任何东西，只认服务名、
//! 方法名和这份 JSON 协议。
//!
//! 宿主服务启动时就在，**不需要** `soft_inject` 排队。登记失败只记一条 warn：
//! `/help` 少一行命令不影响 `/whois` 本身。

use fuyubot_plugin_api::{HostApi, HostExt};

/// 宿主命令表服务名（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.host.commands";

/// 一条要登记进宿主命令表的命令。
struct Command {
    /// 不带 `/`，只用 `a-z0-9_`；跨插件重名会被宿主拒绝。
    name: &'static str,
    /// 一句话说明（`/help` 里显示）。
    description: &'static str,
    /// `user` / `chat_admin` / `admin` / `owner`——必须与本插件的实际权限判断一致。
    role: &'static str,
    /// `private` / `group` / `channel`。
    scopes: &'static [&'static str],
    /// `true` 只是不进 `/help` 列表，**不是授权**。
    hidden: bool,
}

/// 本插件登记的命令表。
///
/// `/whois` 没有权限判断：私聊、群聊里谁发都执行，因此按最宽的实际范围登记
/// `role = "user"`、`scopes = ["private", "group"]`（不登记 `channel`，频道里没有
/// 用户命令语义）。登记只是自描述，执行权限仍由本插件自己的命令匹配决定。
const COMMANDS: &[Command] = &[Command {
    name: "whois",
    description: "查询域名注册信息与服务器 IP 归属，渲染成信息卡片",
    role: "user",
    scopes: &["private", "group"],
    hidden: false,
}];

/// 把所有命令登记进宿主命令表。
///
/// 失败只记日志、不返回错误：`/help` 少一行不影响插件自己的命令。
pub(crate) fn register(host: &HostApi) {
    for command in COMMANDS {
        let payload = payload(command);
        match host.call_service(SERVICE, "register", &payload) {
            Ok(reply) if reply_ok(&reply) => {
                host.info(&format!("whois: 已登记命令 /{}（{reply}）", command.name));
            }
            Ok(reply) => {
                host.warn(&format!(
                    "whois: 宿主命令表拒绝登记 /{}（{SERVICE}.register 回 {reply}），继续跑",
                    command.name
                ));
            }
            Err(error) => {
                // AbiError 不细分「服务不在」和「方法不在」，日志里把两个名字都打出来。
                host.warn(&format!(
                    "whois: 登记命令 /{} 失败（{SERVICE}.register：{error}），/help 将少一行",
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

/// 提供方成功回 `{"ok":true,…}`，失败回 `{"error":…}`——两者都是 `Ok(String)`。
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
    fn payload_has_every_service_field() {
        let command = &COMMANDS[0];
        let value: serde_json::Value = serde_json::from_str(&payload(command)).unwrap();
        assert_eq!(value["name"], "whois");
        assert_eq!(
            value["description"],
            "查询域名注册信息与服务器 IP 归属，渲染成信息卡片"
        );
        assert_eq!(value["role"], "user");
        assert_eq!(value["scopes"], serde_json::json!(["private", "group"]));
        assert_eq!(value["hidden"], false);
        assert_eq!(
            value.as_object().unwrap().len(),
            5,
            "只登记契约字段: {value}"
        );
    }

    #[test]
    fn registration_matches_the_actual_permissions() {
        for command in COMMANDS {
            // 命令没有权限判断（谁都能发），角色不能比 user 更严。
            assert_eq!(command.role, "user", "{}", command.name);
            // 私聊与群聊都能用；频道不登记。
            assert_eq!(command.scopes, &["private", "group"], "{}", command.name);
            // hidden 只是列表可见性，本插件没有需要藏起来的命令。
            assert!(!command.hidden, "{}", command.name);
        }
    }

    #[test]
    fn only_explicit_ok_counts_as_success() {
        assert!(reply_ok(r#"{"ok":true,"name":"whois","plugin":"whois"}"#));
        assert!(!reply_ok(r#"{"error":"缺 name 或为空"}"#));
        assert!(!reply_ok(""));
    }

    #[test]
    fn every_command_has_a_plain_name() {
        for command in COMMANDS {
            assert!(
                !command.name.is_empty() && !command.name.starts_with('/'),
                "{}",
                command.name
            );
            assert!(
                command
                    .name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
                "{}",
                command.name
            );
            assert!(!command.description.is_empty(), "{}", command.name);
        }
    }
}
