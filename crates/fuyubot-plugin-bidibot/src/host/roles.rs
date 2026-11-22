//! 全局角色：主人 / 管理员归宿主管，插件不再各写一份 owner 配置。
//!
//! FuyuBot 新版本把「谁是主人」提到了 `fuyubot.toml` 的 `[roles]` 段
//! （`owners` / `admins`），并给了查询入口：
//!
//! - **服务** `fuyubot.host.commands`，方法 `role`，入参 `{"user_id": N}`，
//!   回 `{"global":"user"|"admin"|"owner"}`——宿主的权威判定，含 user 账户
//!   「`owners` 为空时自己就是主人」的默认规则；纯内存读取，不碰 TDLib，
//!   在接收循环线程上调也不会卡。
//! - **配置** `[roles].owners`：`role` 方法不可用时（老宿主 / 裁剪版）的兜底。
//!   user 账户的「自己就是主人」靠 `fuyubot.host.identity.me` 再判一次。
//!
//! `[plugin.fengyegf.bidibot].owner_id` 仍然认，语义变成**额外的主人**：
//! 老配置照常工作，新部署只写 `[roles]` 就行，不用重复配置。

use fuyubot_plugin_api::{HostApi, HostExt as _};
use serde_json::{Value, json};

/// 宿主预置的命令表服务（`role` 方法也挂在这张表上）。
pub const SERVICE: &str = "fuyubot.host.commands";
/// 查询全局角色的方法名。
pub const ROLE_METHOD: &str = "role";
/// 宿主预置的身份服务：拿自己的 user id 用。
pub const IDENTITY_SERVICE: &str = "fuyubot.host.identity";
/// 身份服务的 `me` 方法（`getMe` 的 user JSON）。
pub const ME_METHOD: &str = "me";

/// 宿主的全局角色（`user` ⊂ `admin` ⊂ `owner`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalRole {
    User,
    Admin,
    Owner,
}

impl GlobalRole {
    /// 解析宿主给的角色名；不认识的值返回 `None`（宁可当没查到，也不当主人）。
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "owner" => Some(Self::Owner),
            "admin" | "administrator" => Some(Self::Admin),
            "user" | "member" => Some(Self::User),
            _ => None,
        }
    }

    /// 日志用的短名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }

    /// `{"global":"owner"}` → `Owner`；字段名同时接受 `global` / `role`。
    pub fn from_reply(reply: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(reply).ok()?;
        let text = value
            .get("global")
            .or_else(|| value.get("role"))?
            .as_str()?;
        Self::parse(text)
    }
}

/// 问宿主这个人的全局角色；服务不可用 / 回包不认时返回 `None`。
pub fn role_of(host: &HostApi, user_id: i64) -> Option<GlobalRole> {
    let input = json!({ "user_id": user_id }).to_string();
    let reply = host.call_service(SERVICE, ROLE_METHOD, &input).ok()?;
    GlobalRole::from_reply(&reply)
}

/// 是不是主人：插件段的额外主人 `extra_owner` 或宿主的全局主人。
pub fn is_owner(host: &HostApi, extra_owner: i64, user_id: i64) -> bool {
    if extra_owner != 0 && user_id == extra_owner {
        return true;
    }
    match role_of(host, user_id) {
        Some(role) => is_owner_with(extra_owner, user_id, Some(role), &[]),
        None => is_owner_with(extra_owner, user_id, None, &owners(host)),
    }
}

/// 判定本体（纯函数，单测用）：额外主人 + 宿主角色 + 宿主主人表。
pub fn is_owner_with(
    extra_owner: i64,
    user_id: i64,
    role: Option<GlobalRole>,
    owners: &[i64],
) -> bool {
    if extra_owner != 0 && user_id == extra_owner {
        return true;
    }
    match role {
        Some(role) => role == GlobalRole::Owner,
        None => owners.contains(&user_id),
    }
}

/// 宿主的 `[roles].owners`；读不到就是空表。
pub fn owners(host: &HostApi) -> Vec<i64> {
    for (section, key) in [("roles", "owners"), ("roles", ""), ("", "roles.owners")] {
        let Some(value) = host.config_value(section, key) else {
            continue;
        };
        let list = ids(value.get("owners").unwrap_or(&value));
        if !list.is_empty() {
            return list;
        }
    }
    Vec::new()
}

/// 生效的主人名单：`owner_id`（额外主人）+ `[roles].owners`；
/// 都为空但宿主认为「自己就是主人」时（user 账户），把账号自己算进去。
pub fn effective_owners(host: &HostApi, extra_owner: i64) -> Vec<i64> {
    let mut list: Vec<i64> = Vec::new();
    if extra_owner != 0 {
        list.push(extra_owner);
    }
    for id in owners(host) {
        if !list.contains(&id) {
            list.push(id);
        }
    }
    if list.is_empty()
        && let Some(me) = self_id(host)
        && role_of(host, me) == Some(GlobalRole::Owner)
    {
        list.push(me);
    }
    list
}

/// 宿主自己的 user id（`getMe`）；没登录就是 `None`。
pub fn self_id(host: &HostApi) -> Option<i64> {
    let reply = host.call_service(IDENTITY_SERVICE, ME_METHOD, "{}").ok()?;
    let value: Value = serde_json::from_str(&reply).ok()?;
    id(value.get("id")?)
}

/// `/bidistatus` 的主人文案。
pub fn owner_label(host: &HostApi, extra_owner: i64) -> String {
    label(&effective_owners(host, extra_owner))
}

/// 主人名单 → 一行文案（纯函数）。
pub fn label(ids: &[i64]) -> String {
    if ids.is_empty() {
        return "未配置（管理命令不可用）".to_owned();
    }
    let text = ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join("、");
    if ids.len() > 1 {
        format!("{text}（共 {} 人）", ids.len())
    } else {
        text
    }
}

/// JSON 里的 id 列表：数组、单个数字、数字字符串都认；0 与非法值丢弃。
fn ids(value: &Value) -> Vec<i64> {
    match value {
        Value::Array(items) => items.iter().filter_map(id).collect(),
        other => id(other).into_iter().collect(),
    }
}

/// 单个 id：数字或数字字符串，0 / 非法值 → `None`。
fn id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .filter(|id| *id != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_reply_is_parsed() {
        assert_eq!(
            GlobalRole::from_reply(r#"{"global":"owner"}"#),
            Some(GlobalRole::Owner)
        );
        assert_eq!(
            GlobalRole::from_reply(r#"{"global":"ADMIN"}"#),
            Some(GlobalRole::Admin)
        );
        assert_eq!(
            GlobalRole::from_reply(r#"{"role":"user"}"#),
            Some(GlobalRole::User)
        );
        assert_eq!(GlobalRole::from_reply("not json"), None);
        assert_eq!(GlobalRole::from_reply(r#"{"global":"root"}"#), None);
    }

    #[test]
    fn configured_owner_and_global_owner_both_count() {
        // 插件段配的额外主人：认。
        assert!(is_owner_with(42, 42, None, &[]));
        // 宿主说 owner：认，即使插件段没配。
        assert!(is_owner_with(0, 7, Some(GlobalRole::Owner), &[]));
        // 宿主说 admin / user：不认（管理命令只给主人）。
        assert!(!is_owner_with(0, 7, Some(GlobalRole::Admin), &[]));
        assert!(!is_owner_with(0, 7, Some(GlobalRole::User), &[]));
        // role 服务不可用：退回 [roles].owners。
        assert!(is_owner_with(0, 7, None, &[7, 8]));
        assert!(!is_owner_with(0, 9, None, &[7, 8]));
    }

    #[test]
    fn owner_list_accepts_numbers_and_strings() {
        assert_eq!(ids(&json!([111, "222", 0, "x"])), vec![111, 222]);
        assert_eq!(ids(&json!(333)), vec![333]);
        assert_eq!(ids(&json!([])), Vec::<i64>::new());
    }

    #[test]
    fn label_lists_owners_or_explains_absence() {
        assert_eq!(label(&[]), "未配置（管理命令不可用）");
        assert_eq!(label(&[111]), "111");
        assert_eq!(label(&[111, 222]), "111、222（共 2 人）");
    }
}
