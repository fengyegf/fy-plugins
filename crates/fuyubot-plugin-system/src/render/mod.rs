//! 图像生成：卡片数据 → Vue 模板 + `data` → `fuyubot.html2img` 服务 → 本机图片文件。
//!
//! **界面不在 Rust 里拼**：整张卡片的 HTML / CSS / Vue 指令都在
//! [`assets/card.html`](../../assets/card.html) 里（`<style>` + Vue 模板，
//! 正文来自 TS 版 `system.vue` 的 `<template>`），本模块只做三件事：
//!
//! 1. 把业务模型映射成 Vue 模板的 `data`（[`view_data`]）；
//! 2. 调宿主上的 html2img 服务（`vue: true`），拿本机图片路径；
//! 3. 把错误翻译成插件层能展示的中文。
//!
//! 与 TS 版 `E:\Fuyu_TDBot\plugins\system`（Vue + satori + resvg）的对应关系：
//!
//! | TS 版（Vue + satori + resvg） | 这里（Vue 模板 + html2img） |
//! | --- | --- |
//! | `system.vue` 模板 → `satoriVue` | `assets/card.html` 的 Vue 模板 |
//! | satori 的 flex 布局子集 | html2img 里的 Blitz + Stylo + Taffy 布局 |
//! | 背景图 + 白色蒙版（CSS） | 模板里的渐变层 + `<img :src="background">` |
//! | `<img>` 头像（data URI） | 头像缩到 ≤192px JPEG 后同样写 `data:image/jpeg` |
//!
//! 分工：
//!
//! - [`avatar`]：头像字节 → ≤192px JPEG → `data:` URI（控制 HTML 体积）；
//! - [`html2img`]：服务客户端，拼入参 / 调服务 / 解出 `path`；
//! - [`samples`]：离线样张（不联网也能生成 data）。
//!
//! html2img 是**硬依赖**（`Plugin::inject`）：没有它出不了图，插件停在 Pending
//! 比启动后每条命令都失败更容易排查。

pub mod avatar;
pub mod html2img;
pub mod samples;

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::{Map, Value, json};

use crate::core::model::{Assets, CardData};

/// 卡片设计尺寸（与 `system.vue` 的 `w-350 h-200` 按 4px 间距换算一致）。
pub const WIDTH: u32 = 1400;
pub const HEIGHT: u32 = 800;

/// 输出倍率：与老版本地渲染一致，1400×800 CSS px 1:1 出图。
pub const SCALE: f32 = 1.0;
/// 输出格式：不透明背景，JPEG 体积小、Telegram 照片上传够用。
pub const FORMAT: &str = "jpg";

/// 卡片模板：`<style>` + Vue 模板，编译期嵌入。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 面板里最多画出的插件胶囊数：超出的折成 `+N`（纯数据层限额，不做坐标计算）。
pub const MAX_PLUGIN_CHIPS: usize = 12;

/// 单次 HTML 上限，与 html2img 服务配置的默认值（128 KiB）对齐。
///
/// 超了就在发请求前报错，不把注定被服务拒绝的请求发过去；头像已经缩到 ≤192px，
/// 正常卡片（两张头像 + 版面）大约 20–40 KiB。
pub const MAX_HTML_BYTES: usize = 128 * 1024;

/// 空数据兜底。
pub const NO_DATA: &str = "暂无数据";
/// 名字为空时的兜底（与 TS 版 `bot.name` 的展示一致）。
pub const UNKNOWN: &str = "未知";
/// 没配管理员时的占位身份。
pub const UNCONFIGURED: &str = "未配置管理员";
/// 没配管理员时的用户名占位（模板里会显示成 `@未配置` 的位置）。
const UNCONFIGURED_USERNAME: &str = "未配置";

/// 渲染失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 渲染服务调用失败（服务不在 / 模板编译失败 / 渲染报错 / 应答不合法）。
    #[error("渲染服务失败: {0}")]
    Service(#[from] html2img::Error),
    /// 模板本身超过服务上限（编译期就该发现，留作兜底）。
    #[error("卡片模板 {0} 字节，超过 128 KiB 上限")]
    TemplateTooLarge(usize),
}

/// 把卡片渲染成本机图片文件，返回 html2img 产出的路径。
///
/// `background_url` 是 `[plugin.system] background_url`：公网 URL 写进模板的
/// `data.background`，由 html2img 去拉；不是公网 `http(s)` 就传空串，模板用
/// `v-if` 把背景图整个隐藏，只留渐变。
///
/// 调用方负责在用完之后删掉 `path`（html2img 自己会在 24 小时后清理）。
pub fn render_card(
    host: &HostApi,
    card: &CardData,
    assets: &Assets,
    background_url: Option<&str>,
) -> Result<PathBuf, Error> {
    if TEMPLATE.len() > MAX_HTML_BYTES {
        return Err(Error::TemplateTooLarge(TEMPLATE.len()));
    }
    let data = view_data(card, assets, background_url);
    let request = html2img::Request {
        html: TEMPLATE,
        // 卡片是 Vue 模板：循环 / 条件 / 插值都在模板里，Rust 只给数据。
        vue: true,
        data: Some(&data),
        width: WIDTH,
        height: Some(HEIGHT),
        scale: SCALE,
        format: FORMAT,
        auto_height: false,
        transparent: false,
        dark: false,
    };
    Ok(html2img::render(host, &request)?)
}

/// 把卡片数据映射成 Vue 模板要的 `data`（不调服务）。
///
/// 数据契约（与 `assets/card.html` 一一对应）：
///
/// - `bot` / `admin`：`{name, username, id, photo}`；`admin` 另带 `configured`，
///   没配管理员时也传占位对象，模板用 `v-if="admin.configured"` 切到配置提示；
/// - `tblib`：`{version, commitHash}`（键名沿用 TS 模板的拼写）；
/// - `system.system`：`{标签: {usage}}`（CPU / RAM，空数据给一行 `暂无数据` 占位）；
/// - `system.source`：`{标签: {used, total, usagePercent}}`（空数据同理）；
/// - `plugins`：`{插件名: {name, version}}`，`version` 是运行状态标签
///   （宿主 ABI 没有版本号，蓝色后缀沿用原模板的位置显示「运行 / 等待 / 失败」）；
///   最多 [`MAX_PLUGIN_CHIPS`] 枚，超出的折成 `+N`；
/// - `plugin_count` / `plugin_overflow`：插件总数与折成 `+N` 的数量
///   （模板底部「共 N 个插件」与空态判断）；
/// - `background`：公网 `http(s)` 背景 URL，空串表示不显示背景图。
///
/// 头像缩到 ≤192px 再转 `data:` URI；取不到照片就传空串，模板画渐变占位。
pub fn view_data(card: &CardData, assets: &Assets, background_url: Option<&str>) -> Value {
    let bot_photo = assets
        .bot_photo
        .as_deref()
        .and_then(avatar::data_uri)
        .unwrap_or_default();
    let admin_photo = assets
        .admin_photo
        .as_deref()
        .and_then(avatar::data_uri)
        .unwrap_or_default();

    let admin = match &card.admin {
        Some(admin) => json!({
            "configured": true,
            "name": or_unknown(&admin.name),
            "username": or_unknown(&admin.username),
            "id": admin.id,
            "photo": admin_photo,
        }),
        None => json!({
            "configured": false,
            "name": UNCONFIGURED,
            "username": UNCONFIGURED_USERNAME,
            "id": 0,
            "photo": admin_photo,
        }),
    };

    // CPU / RAM：标签做键，值只留模板需要的 usage；空数据给一行占位。
    let mut gauges = Map::new();
    for gauge in card.gauges.iter().take(2) {
        gauges.insert(
            or_unknown(&gauge.label).to_owned(),
            json!({ "usage": gauge.usage.min(100) }),
        );
    }
    if gauges.is_empty() {
        gauges.insert(
            NO_DATA.to_owned(),
            json!({ "usage": 0, "placeholder": true }),
        );
    }

    // 磁盘：标签（「磁盘C」/「根目录」/「内部存储」）做键，最多 4 块。
    let mut disks = Map::new();
    for disk in card.disks.iter().take(4) {
        disks.insert(
            or_unknown(&disk.label).to_owned(),
            json!({
                "used": disk.used,
                "total": disk.total,
                "usagePercent": disk.usage_percent.min(100),
            }),
        );
    }
    if disks.is_empty() {
        disks.insert(
            NO_DATA.to_owned(),
            json!({
                "used": "",
                "total": "",
                "usagePercent": 0,
                "placeholder": true,
            }),
        );
    }

    // 已加载插件：名字做键（同名插件不会出现），值里的 version 放状态标签。
    // 胶囊最多画 MAX_PLUGIN_CHIPS 枚，其余折成 `+N`（数据层限额，模板只负责显示）。
    let shown = card.plugins.len().min(MAX_PLUGIN_CHIPS);
    let mut plugins = Map::new();
    for chip in card.plugins.iter().take(shown) {
        let name = or_unknown(&chip.name);
        plugins.insert(
            name.to_owned(),
            json!({ "name": name, "version": chip.state_label() }),
        );
    }

    json!({
        "bot": {
            "name": or_unknown(&card.bot.name),
            "username": or_unknown(&card.bot.username),
            "id": card.bot.id,
            "photo": bot_photo,
        },
        "admin": admin,
        "tblib": {
            "version": or_unknown(&card.tdlib.version),
            "commitHash": or_unknown(&card.tdlib.commit_hash),
        },
        "system": {
            "system": gauges,
            "source": disks,
        },
        "plugins": plugins,
        "plugin_count": card.plugins.len(),
        "plugin_overflow": card.plugins.len().saturating_sub(shown),
        "background": public_image_url(background_url.unwrap_or("")).unwrap_or(""),
    })
}

/// 空串换成兜底文案（返回 `&` 借用，不复制）。
fn or_unknown(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.is_empty() { UNKNOWN } else { trimmed }
}

/// 只把公网 `http(s)` 图片写进模板。
///
/// `file://`、相对路径、`data:`（背景太大）都直接丢弃：html2img 会拒绝这类图片，
/// 与其让渲染器报错，不如提前退化成渐变。
pub fn public_image_url(url: &str) -> Option<&str> {
    let trimmed = url.trim();
    let is_public = trimmed
        .get(..8)
        .is_some_and(|head| head.eq_ignore_ascii_case("https://"))
        || trimmed
            .get(..7)
            .is_some_and(|head| head.eq_ignore_ascii_case("http://"));
    is_public.then_some(trimmed)
}
