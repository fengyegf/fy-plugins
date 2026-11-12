//! 图像生成：Vue 卡片模板 + 数据 → `fuyubot.html2img` 服务 → 本机图片文件。
//!
//! **界面不在 Rust 里拼**：整张卡片的 HTML / CSS / Vue 指令都在
//! [`assets/card.html`](../../assets/card.html) 里（`<style>` + Vue 模板），
//! 本模块只做三件事：
//!
//! 1. 把 [`CardData`] 映射成 Vue 模板的 `data`（[`serde_json::Value`]）；
//! 2. 调宿主上的 html2img 服务（`vue: true`），拿本机图片路径；
//! 3. 把错误翻译成插件层能展示的中文。
//!
//! 渲染器只有宿主那一份（hyper-render 的本地 fork：Blitz + Stylo + Taffy + Vello CPU），
//! 插件不再链接 hyper-render，也不再自己拼 HTML 字符串。循环 / 条件 / 插值全在模板里：
//! 域名状态与 DNS 用 `v-for`，未注册域名隐藏左列用 `v-show`，网站图标有无用
//! `v-if` / `v-else`，状态徽标配色用 `v-if` 分支。
//!
//! 本机只保留一处图片预处理：网站图标先由 [`crate::core::favicon`] 缩到 ≤64px，
//! 这里再编码成 `data:image/png;base64,…` 塞进模板的 `icon` 字段（[`icon`]）。
//! 其余图形都是模板里的内联 SVG，Rust 不参与。
//!
//! html2img 是**硬依赖**（`Plugin::inject`）：没有它出不了图，插件停在 Pending
//! 比启动后每条命令都失败更容易排查。
//!
//! 调用约束：`render` 是**同步阻塞**的，只能在业务线程里调（宿主回调线程上等渲染
//! 会卡住 TDLib 接收）；产物是服务临时目录里的文件（24 小时内有效），命令层发完即删。

pub mod html2img;
pub mod icon;

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use crate::core::favicon::Icon;
use crate::core::model::CardData;
use icon::data_uri as icon_data_uri;

/// 卡片模板：`<style>` + Vue 模板，编译期嵌入。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 卡片宽度允许的范围，与 `core::config` 的夹取范围一致。
///
/// 下限 900 不是拍脑袋：两列信息框每列只剩 418px，值列约 266px，再窄 `status`
/// 文本就会一两行地折；上限 1600：Telegram 再大也没必要。
pub const MIN_WIDTH: u32 = 900;
pub const MAX_WIDTH: u32 = 1600;

/// 交给 html2img 的视口高度。
///
/// `auto_height: true` 时 `height` 只是**下限**：不传会落到服务默认的 768，
/// 未注册域名那种只有一列的短卡片会因此底部留白。200 比任何一张卡片都矮，真实高度
/// 完全由内容撑开。
pub const VIEWPORT_HEIGHT: u32 = 200;

/// 单次 HTML 上限，与 html2img 默认的 `max_html_bytes`（128 KiB）对齐。
pub const MAX_HTML_BYTES: usize = 128 * 1024;

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

/// 把宽度夹到 `[MIN_WIDTH, MAX_WIDTH]`。
pub fn clamp_width(width: u32) -> u32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// 把一张卡片渲染成图片，返回 html2img 产出的本机文件路径；`width` 是卡片宽度
/// （px，会夹到 900–1600）。
///
/// `icon` 是抓到的网站图标：`None` 时模板回退到内置的渐变地球图标。
///
/// 这是阻塞调用，只能跑在业务线程上；调用方负责在用完之后删掉 `path`
/// （html2img 自己会在 24 小时后清理）。
pub fn render_card(
    host: &HostApi,
    card: &CardData,
    width: u32,
    icon: Option<&Icon>,
) -> Result<PathBuf, Error> {
    if TEMPLATE.len() > MAX_HTML_BYTES {
        return Err(Error::TemplateTooLarge(TEMPLATE.len()));
    }
    let width = clamp_width(width);
    let data = view_data(card, width, icon);
    let request = html2img::Request {
        html: TEMPLATE,
        // 卡片是 Vue 模板：循环 / 条件 / 插值都在模板里，Rust 只给数据。
        vue: true,
        data: Some(&data),
        width,
        // 短卡片不被默认的 768px 视口留白撑高；长卡片由 auto_height 撑开。
        height: Some(VIEWPORT_HEIGHT),
        // 与老版本地渲染一致：1200 CSS px 宽、1:1 输出。
        scale: 1.0,
        // 白底卡片，JPEG 体积小、Telegram 照片上传够用。
        format: "jpg",
        auto_height: true,
        transparent: false,
        dark: false,
    };
    Ok(html2img::render(host, &request)?)
}

/// 卡片模板要的 `data`：`CardData` 的字段 + 模板需要的派生值。
///
/// 键名与 `whois.vue` 的 props 一致（snake_case）；模板只读字段，不做任何函数调用
/// 或全局对象访问，所以下面这些值先在 Rust 里算好：
///
/// - `creation_years`：原来的 `Math.floor(creation_days / 365)`（用 `div_euclid`
///   对齐 JS 的向下取整）；
/// - `ip_location`：`country` + 空格 + `city`，都为空时 `N/A`；
/// - `icon`：favicon 的 PNG data URI，没有图标时是空串（模板用 `v-if` 判断）。
///
/// 文本字段在这里只做 `trim`：**不做 HTML 转义**。转义是渲染器的事——
/// vue-template-html 的插值和属性绑定都会转义一次，Rust 再转一次会得到
/// `&amp;lt;` 这种双重转义。
pub fn view_data(card: &CardData, width: u32, icon: Option<&Icon>) -> Value {
    json!({
        "width": clamp_width(width),
        "icon": icon.and_then(icon_data_uri).unwrap_or_default(),
        // 徽标分支：is_expire === 0 → 已注册 / 未注册，否则 已过期 / 未过期。
        "is_available": card.is_available,
        "is_expire": card.is_expire,
        "domain": card.domain.trim(),
        "domain_suffix": card.domain_suffix.trim(),
        "query_time": card.query_time.trim(),
        "registrant_name": card.registrant_name.trim(),
        "registrant_email": card.registrant_email.trim(),
        "registrar_name": card.registrar_name.trim(),
        "creation_time": card.creation_time.trim(),
        "expiration_time": card.expiration_time.trim(),
        "creation_days": card.creation_days,
        "valid_days": card.valid_days,
        "creation_years": card.creation_days.div_euclid(365),
        "domain_status": clean_list(&card.domain_status),
        "name_server": clean_list(&card.name_server),
        "whois_server": card.whois_server.trim(),
        "ip": card.ip.trim(),
        "country": card.country.trim(),
        "city": card.city.trim(),
        "area": card.area.trim(),
        "isp": card.isp.trim(),
        "ip_location": ip_location(card),
    })
}

/// 列表字段清洗：去掉首尾空白与空串。
///
/// 模板的 `v-for` 只负责遍历，没有过滤能力；空列表由模板的
/// `v-if="domain_status.length === 0"` 补一行「暂无数据」。
fn clean_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

/// IP 归属地：`country` + 空格 + `city`，都为空时 `N/A`。
fn ip_location(card: &CardData) -> String {
    let country = card.country.trim();
    let city = card.city.trim();
    match (country.is_empty(), city.is_empty()) {
        (true, true) => "N/A".to_owned(),
        (false, true) => country.to_owned(),
        (true, false) => city.to_owned(),
        (false, false) => format!("{country} {city}"),
    }
}
