//! `/system`：采集系统信息 → 查询 TDLib / bot / admin → 渲染卡片 → 以照片回复。
//!
//! 流程与 TS 版 `index.ts` 一一对应：
//!
//! ```text
//! /system
//!   → getSystemInformation()          // systeminformation → core::sysinfo
//!   → getOption("version"/"commit_hash")  // TDLib
//!   → getMe() + 头像                   // TDLib
//!   → getConfig("admin").super_admin → getUser(admin) + 头像  // 配置 + TDLib
//!   → pluginManager.getPlugins()      // 宿主 list_plugins
//!   → generateImage(system.vue)       // render::render_card（hyper-render）
//!   → sendMessage({photo})            // host::messenger
//!   → 删除临时文件
//! ```
//!
//! 与 TS 版的两处主动降级（都是宿主能力决定的，见 README）：
//! - TDLib 查询失败不炸整条命令：卡片照出，对应字段显示「未知」；
//! - 管理员来自 `[plugin.system] super_admin` 配置，不配就显示「未配置」。

use std::path::PathBuf;
use std::time::Instant;

use serde_json::Value;

use crate::core::background;
use crate::core::model::{Assets, CardData, Identity, PluginChip};
use crate::core::sysinfo;
use crate::core::td;
use crate::core::td::Td;
use crate::host::{Ctx, Msg};
use crate::render;

/// 失败提示（与 TS 版文案一致）。
const FAILED: &str = "❌ 获取系统信息失败，请稍后重试。";
/// TDLib 取不到时的兜底文本（与 TS 版的「未知」一致）。
const UNKNOWN: &str = "未知";

/// 命令入口。
pub fn run(ctx: &Ctx, msg: Msg) {
    let started = Instant::now();
    match build_image(ctx) {
        Ok(jpeg) => {
            ctx.info(&format!(
                "system: 卡片已生成 {} bytes（{} ms）",
                jpeg.len(),
                started.elapsed().as_millis()
            ));
            send(ctx, msg, &jpeg);
        }
        Err(reason) => {
            ctx.error(&format!("system: 获取系统信息失败: {reason}"));
            ctx.messenger
                .try_send_text(msg.chat_id, FAILED, Some(msg.id));
        }
    }
}

/// 取数据 + 渲染。任何「非致命」的外部依赖失败都在这里降级，只有渲染失败才上抛。
fn build_image(ctx: &Ctx) -> Result<Vec<u8>, String> {
    // 1. 系统信息（CPU 两次采样约 200ms）。
    let (snapshot, collect_ms) = sysinfo::collect_timed();
    ctx.info(&format!(
        "system: 系统信息采集完成（{collect_ms} ms）：{} 条使用率 / {} 块磁盘",
        snapshot.gauges.len(),
        snapshot.disks.len()
    ));

    let timeout = ctx.config.td_timeout;
    let avatar_timeout = ctx.config.avatar_timeout;
    let td = Td::new(ctx.host);

    // 2. TDLib 版本（`getOption` 的 optionValueString）。
    let version = td.option_string("version", timeout).unwrap_or_else(|| {
        ctx.warn("system: getOption(\"version\") 失败，卡片显示「未知」");
        UNKNOWN.to_owned()
    });
    let commit_hash = td.option_string("commit_hash", timeout).unwrap_or_else(|| {
        ctx.warn("system: getOption(\"commit_hash\") 失败，卡片显示「未知」");
        UNKNOWN.to_owned()
    });

    // 3. 机器人自己（`getMe`）与头像。
    let (bot, bot_photo) = match td.get_me(timeout) {
        Ok(user) => {
            let identity = td::identity_of(&user);
            let photo = match td.avatar(&user, avatar_timeout) {
                Ok(bytes) => {
                    ctx.info(&format!(
                        "system: 机器人头像已就绪（{} bytes）",
                        bytes.len()
                    ));
                    Some(bytes)
                }
                Err(reason) => {
                    ctx.warn(&format!("system: 机器人头像不可用（{reason}），使用占位图"));
                    None
                }
            };
            (identity, photo)
        }
        Err(error) => {
            ctx.warn(&format!(
                "system: getMe 失败（{error}），卡片里的 bot 一栏用兜底值"
            ));
            (
                Identity {
                    name: UNKNOWN.to_owned(),
                    username: UNKNOWN.to_owned(),
                    id: 0,
                },
                None,
            )
        }
    };

    // 4. 管理员（配置 + `getUser`）；没配就是 None。
    let (admin, admin_photo) = match ctx.config.super_admin {
        Some(user_id) => match td.get_user(user_id, timeout) {
            Ok(user) => {
                let identity = td::identity_of(&user);
                let photo = match td.avatar(&user, avatar_timeout) {
                    Ok(bytes) => {
                        ctx.info(&format!(
                            "system: 管理员头像已就绪（{} bytes）",
                            bytes.len()
                        ));
                        Some(bytes)
                    }
                    Err(reason) => {
                        ctx.warn(&format!("system: 管理员头像不可用（{reason}），使用占位图"));
                        None
                    }
                };
                (Some(identity), photo)
            }
            Err(error) => {
                ctx.warn(&format!(
                    "system: getUser({user_id}) 失败（{error}），admin 一栏只显示 id"
                ));
                (
                    Some(Identity {
                        name: UNKNOWN.to_owned(),
                        username: UNKNOWN.to_owned(),
                        id: user_id,
                    }),
                    None,
                )
            }
        },
        None => (None, None),
    };

    // 5. 宿主进程里加载的插件（名字 + 状态；ABI 没有版本号）。
    let plugins = read_plugins(ctx);
    ctx.info(&format!(
        "system: 宿主里 {} 个插件（{}）",
        plugins.len(),
        plugins
            .iter()
            .map(|chip| chip.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // 6. 背景图（可选，失败退回渐变）。
    let background = background::load(&ctx.config);
    if background.is_none() {
        ctx.info("system: 背景图不可用，卡片使用渐变背景");
    }

    let card = CardData {
        bot,
        admin,
        tdlib: crate::core::model::TdlibInfo {
            version,
            commit_hash,
        },
        gauges: snapshot.gauges,
        disks: snapshot.disks,
        plugins,
    };
    let assets = Assets {
        bot_photo,
        admin_photo,
        background,
    };

    // 7. 渲染（hyper-render + image 合成）。
    let jpeg = render::render_card(&card, &assets).map_err(|error| error.to_string())?;
    Ok(jpeg)
}

/// 把 JPEG 写到临时文件、以照片回复，然后删除临时文件。
fn send(ctx: &Ctx, msg: Msg, jpeg: &[u8]) {
    let path = temp_path(msg);
    if let Err(error) = std::fs::write(&path, jpeg) {
        ctx.error(&format!(
            "system: 写临时文件失败 {}: {error}",
            path.display()
        ));
        ctx.messenger
            .try_send_text(msg.chat_id, "❌ 临时文件写入失败", Some(msg.id));
        return;
    }

    match ctx.messenger.send_photo(msg.chat_id, &path, Some(msg.id)) {
        Ok(sent) => ctx.info(&format!(
            "system: 已发送系统信息卡片（chat={} message_id={}）",
            sent.chat_id, sent.message_id
        )),
        Err(error) => {
            ctx.error(&format!("system: 发送图片失败: {error}"));
            ctx.messenger
                .try_send_text(msg.chat_id, FAILED, Some(msg.id));
        }
    }

    // 上传走的是本地文件，发完就删；失败也删，别在系统临时目录里堆图。
    if let Err(error) = std::fs::remove_file(&path) {
        ctx.warn(&format!(
            "system: 删除临时文件失败 {}: {error}",
            path.display()
        ));
    }
}

/// 临时文件路径：每次命令一个，重名也不会互相覆盖。
fn temp_path(msg: Msg) -> PathBuf {
    std::env::temp_dir().join(format!("fuyubot-system-{}-{}.jpg", msg.chat_id, msg.id))
}

/// 读宿主的插件清单：`{"count":N,"plugins":[{"name","state",…}]}`。
fn read_plugins(ctx: &Ctx) -> Vec<PluginChip> {
    let raw = ctx.host.list_plugins();
    let Some(value) = serde_json::from_str::<Value>(&raw).ok() else {
        ctx.warn("system: 宿主 list_plugins 返回的不是 JSON，插件一栏留空");
        return Vec::new();
    };
    let Some(items) = value.get("plugins").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(Value::as_str)?.to_owned();
            let state = item
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            Some(PluginChip { name, state })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_path_is_unique_per_message() {
        let first = temp_path(Msg { chat_id: 1, id: 2 });
        let second = temp_path(Msg { chat_id: 1, id: 3 });
        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains("fuyubot-system-1-2.jpg"));
    }
}
