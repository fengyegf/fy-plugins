//! 卡片数据模型：渲染层与业务层之间的唯一契约。
//!
//! 对照 TS 版 `system.vue` 的 `defineProps`：
//!
//! | `system.vue` | 这里 |
//! | --- | --- |
//! | `bot: { name, username, id, photo? }` | [`Identity`] + [`Assets::bot_photo`] |
//! | `admin: { name, username, id, photo? }` | `Option<`[`Identity`]`>` + [`Assets::admin_photo`] |
//! | `tblib: { version, commitHash }` | [`TdlibInfo`] |
//! | `system.system: { CPU/RAM: { usage } }` | [`Gauge`] 列表 |
//! | `system.source: { 盘符: { used, total, usagePercent } }` | [`DiskUsage`] 列表 |
//! | `plugins: { [key]: { name, version } }` | [`PluginChip`] 列表 |
//!
//! 两点与 TS 版不同（都是宿主能力决定的，见 README 的「与 TS 版的差异」）：
//!
//! - 管理员改成可选的 `[plugin.system] super_admin` 配置——Rust 宿主没有 TS 版那套
//!   `[admin]` 数据库段；
//! - 插件清单只有名字与状态（宿主 `list_plugins` 的字段），没有版本号。
//!
//! 图片不放进 [`CardData`]：它们来自网络/本地文件，取不到时只该降级成占位图，
//! 不该让整张卡片失败，所以单独放在 [`Assets`] 里。

/// 机器人或管理员的可展示身份。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// 显示名（`first_name`；为空时业务层会兜底成「未知」）。
    pub name: String,
    /// 可编辑用户名（不带 `@`；为空时渲染层显示 `@未知`）。
    pub username: String,
    /// Telegram 用户 id（0 表示未知）。
    pub id: i64,
}

/// TDLib 的版本信息（来自 `getOption("version")` / `getOption("commit_hash")`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TdlibInfo {
    /// 版本号，取不到时是「未知」。
    pub version: String,
    /// 提交哈希，取不到时是「未知」。
    pub commit_hash: String,
}

/// 一条使用率进度条（CPU / RAM）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gauge {
    /// 标签：`CPU` / `RAM`。
    pub label: String,
    /// 使用率百分比，夹在 0–100。
    pub usage: u8,
}

/// 一块磁盘的占用情况。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskUsage {
    /// 直接显示在卡片上的标签：
    /// Windows 是「磁盘C」，Linux 是「根目录」/挂载点，Android 是「内部存储」/「外部存储」。
    pub label: String,
    /// 已用空间，已格式化成 `12.34 GB`。
    pub used: String,
    /// 总空间，已格式化成 `100.00 GB`。
    pub total: String,
    /// 使用率百分比，夹在 0–100。
    pub usage_percent: u8,
}

/// 已加载插件的一枚胶囊。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginChip {
    /// 插件名（`define_plugin!` 登记的名字）。
    pub name: String,
    /// 宿主报告的状态：`active` / `pending` / `failed` / 其它。
    pub state: String,
}

impl PluginChip {
    /// 状态的中文短标签；未知状态原样返回。
    pub fn state_label(&self) -> &str {
        match self.state.as_str() {
            "active" => "运行",
            "pending" => "等待",
            "failed" => "失败",
            other if !other.is_empty() => other,
            _ => "未知",
        }
    }
}

/// 一张系统信息卡片的全部文字数据。
#[derive(Debug, Clone, Default)]
pub struct CardData {
    /// 机器人自己（`getMe`）。
    pub bot: Identity,
    /// 配置里的超级管理员（`[plugin.system] super_admin` + `getUser`）；没配就是 `None`。
    pub admin: Option<Identity>,
    /// TDLib 版本。
    pub tdlib: TdlibInfo,
    /// CPU / RAM 两条（SWAP 已从卡片上移除）。
    pub gauges: Vec<Gauge>,
    /// 磁盘（最多 4 块，渲染层还会再兜一次）。
    pub disks: Vec<DiskUsage>,
    /// 宿主里已加载的插件。
    pub plugins: Vec<PluginChip>,
}

/// 渲染所需的位图素材（取不到就是 `None`，渲染层画占位图）。
#[derive(Debug, Clone, Default)]
pub struct Assets {
    /// 机器人的头像原图字节（jpeg / png）。
    pub bot_photo: Option<Vec<u8>>,
    /// 管理员的头像原图字节（jpeg / png）。
    pub admin_photo: Option<Vec<u8>>,
    /// 背景图原图字节（jpeg / png）；`None` 时用纯渐变背景。
    pub background: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_state_labels_are_chinese_for_known_states() {
        assert_eq!(
            PluginChip {
                name: "system".to_owned(),
                state: "active".to_owned()
            }
            .state_label(),
            "运行"
        );
        assert_eq!(
            PluginChip {
                name: "x".to_owned(),
                state: "failed".to_owned()
            }
            .state_label(),
            "失败"
        );
        // 未知状态不吞掉，原样给出来（宿主以后加了状态也能看见）。
        assert_eq!(
            PluginChip {
                name: "x".to_owned(),
                state: "activating".to_owned()
            }
            .state_label(),
            "activating"
        );
    }

    #[test]
    fn defaults_are_empty_but_renderable() {
        let card = CardData::default();
        assert!(card.admin.is_none());
        assert!(card.gauges.is_empty());
        assert_eq!(card.bot.id, 0);
    }
}
