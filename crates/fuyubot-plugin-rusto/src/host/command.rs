//! 命令表登记：把 `/ocr` 写进宿主的 `fuyubot.host.commands`。
//!
//! 命令表的主人是宿主，内置 `/help` 只是读取方；登记失败只记一条 warn，
//! `/help` 少一行不影响 `/ocr` 本身。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

/// 宿主命令表服务名（契约名）。
const SERVICE: &str = "fuyubot.host.commands";

/// 一条要登记的命令。
struct Command {
    /// 不带前缀的命令名。
    name: &'static str,
    /// 一句话说明。
    description: &'static str,
    /// 谁能用（本插件不做权限判断，按实际范围登记最宽的 `user`）。
    role: &'static str,
    /// 在哪里可用。
    scopes: &'static [&'static str],
    /// 是否隐藏在 `/help`（不是授权）。
    hidden: bool,
}

/// 本插件命令表。
const COMMANDS: &[Command] = &[Command {
    name: "ocr",
    description: "识别图片里的文字：回复图片发 /ocr，或图片说明写 /ocr",
    role: "user",
    scopes: &["private", "group"],
    hidden: false,
}];

/// 把所有命令登记进宿主命令表。
pub fn register(host: &HostApi) {
    for command in COMMANDS {
        let payload = json!({
            "name": command.name,
            "description": command.description,
            "role": command.role,
            "scopes": command.scopes,
            "hidden": command.hidden,
        })
        .to_string();
        match host.call_service(SERVICE, "register", &payload) {
            Ok(reply) if reply_ok(&reply) => {
                host.info(&format!("rusto: 已登记命令 /{}（{reply}）", command.name));
            }
            Ok(reply) => host.warn(&format!(
                "rusto: 宿主命令表拒绝登记 /{}（{SERVICE}.register 回 {reply}），继续跑",
                command.name
            )),
            Err(error) => host.warn(&format!(
                "rusto: 登记命令 /{} 失败（{SERVICE}.register：{error}），/help 将少一行",
                command.name
            )),
        }
    }
}

/// 提供方成功回 `{"ok":true,…}`。
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
    fn registers_exactly_the_documented_command() {
        let command = &COMMANDS[0];
        assert_eq!(command.name, "ocr");
        assert_eq!(command.role, "user");
        assert_eq!(command.scopes, &["private", "group"]);
        assert!(!command.hidden);
    }

    #[test]
    fn reply_ok_only_accepts_ok_true() {
        assert!(reply_ok(r#"{"ok":true}"#));
        assert!(!reply_ok(r#"{"error":"dup"}"#));
        assert!(!reply_ok("not json"));
    }
}
