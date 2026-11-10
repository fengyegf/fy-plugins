//! `/help` 联动：把本插件的命令登记进内置 help 插件的命令表。
//!
//! 内置 `/help` 把命令表暴露成**进程级共享服务** `fuyubot.help.commands`，
//! 方法 `register` 的入参是 `{"name": "...", "description": "..."}`：成功回
//! `{"ok":true}`，坏入参回 `{"error":"…"}`（见插件文档「插件联动」）。
//!
//! 本插件不链接 help、也不 import 它的任何东西：只认服务名、方法名和这份 JSON 协议。
//!
//! 联动是**可选**的：`soft_inject` 只负责让我们排在 help 后面，不保证它一定在
//! （help 卡在硬依赖上或被卸载时 `call_service` 就会失败）。所以失败只记一条 warn：
//! `/help` 少一行命令不影响 `/musiccard` 本身。

use fuyubot_plugin_api::{HostApi, HostExt};

/// 提供方注册的服务名（契约名，不是插件名）。
pub(crate) const SERVICE: &str = "fuyubot.help.commands";

/// 本插件登记进 `/help` 的命令：(命令名, 一句话说明)。
///
/// 名字不带前缀（`/help` 渲染时自己加），别名写进说明里。
const COMMANDS: &[(&str, &str)] = &[("musiccard", "生成网易云音乐卡片：歌名、链接或歌曲 ID")];

/// 把所有命令登记进 `/help`。
///
/// 失败只记日志、不返回错误：这是可选联动，插件自己的命令照常工作。
pub(crate) fn register(host: &HostApi) {
    for (name, description) in COMMANDS {
        let payload = payload(name, description);
        match host.call_service(SERVICE, "register", &payload) {
            Ok(reply) if reply_ok(&reply) => {
                host.info(&format!("musiccard: 已向 /help 登记命令 /{name}：{reply}"));
            }
            Ok(reply) => {
                host.warn(&format!(
                    "musiccard: /help 拒绝登记命令 /{name}（{SERVICE}.register 回 {reply}），继续跑"
                ));
            }
            Err(error) => {
                // AbiError 不细分「服务不在」和「方法不在」，日志里把两个名字都打出来。
                host.warn(&format!(
                    "musiccard: 登记命令 /{name} 到 /help 失败（{SERVICE}.register：{error}），/help 将少一行"
                ));
            }
        }
    }
}

/// `register` 的入参。抽成纯函数，单测不必造 `HostApi`。
fn payload(name: &str, description: &str) -> String {
    serde_json::json!({ "name": name, "description": description }).to_string()
}

/// 提供方成功回 `{"ok":true}`，失败回 `{"error":"…"}`——两者都是 `Ok(String)`。
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
        let value: serde_json::Value = serde_json::from_str(&payload("musiccard", "说明")).unwrap();
        assert_eq!(value["name"], "musiccard");
        assert_eq!(value["description"], "说明");
    }

    #[test]
    fn only_explicit_ok_counts_as_success() {
        assert!(reply_ok(r#"{"ok":true}"#));
        assert!(!reply_ok(r#"{"error":"缺 name 或为空"}"#));
        assert!(!reply_ok(""));
    }

    #[test]
    fn every_command_has_a_plain_name() {
        for (name, description) in COMMANDS {
            assert!(!name.is_empty() && !name.starts_with('/'), "{name}");
            assert!(!description.is_empty(), "{name}");
        }
    }
}
