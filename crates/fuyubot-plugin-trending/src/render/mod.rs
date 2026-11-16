//! 图像生成：Vue 卡片模板 + 榜单数据 → `fuyubot.html2img` 服务 → 本机图片文件。
//!
//! **界面不在 Rust 里拼**：整张卡片的 HTML / CSS / Vue 指令都在
//! [`assets/card.html`](../../assets/card.html) 里，本模块只做三件事：
//!
//! 1. 把 [`CardData`] 映射成 Vue 模板的 `data`（[view_data]）；
//! 2. 调宿主上的 html2img 服务（`vue: true`），拿本机图片路径；
//! 3. 把错误翻译成插件层能展示的中文。
//!
//! 渲染器只有宿主那一份（Blitz + Stylo + Taffy + Vello CPU），插件不链接渲染引擎。
//! 循环 / 条件 / 插值都在模板里：卡片墙用 `v-for`，头像有无用 `v-if` / `v-else`
//! 切「data URI 图片」和「首字母方块」。
//!
//! html2img 是**硬依赖**（`Plugin::inject`）：没有它出不了图，插件停在 Pending
//! 比启动后每条命令都失败更容易排查。

pub mod html2img;

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use crate::core::config::clamp_width;
use crate::core::model::{self, CardData, thousands};

/// 卡片模板：`<style>` + Vue 模板，编译期嵌入。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 卡片宽度允许的范围，与 `core::config::WIDTH_RANGE` 一致。
pub const MIN_WIDTH: u32 = 1200;
pub const MAX_WIDTH: u32 = 1920;

/// 单次 HTML 上限，与 html2img 默认的 `max_html_bytes`（128 KiB）对齐。
pub const MAX_HTML_BYTES: usize = 128 * 1024;

/// 交给 html2img 的视口高度下限：`auto_height` 下真实高度由内容撑开。
pub const VIEWPORT_HEIGHT: u32 = 240;

/// 卡片墙左右内边距（px），模板里的 `.wall` 必须与它一致。
pub const WALL_PADDING: u32 = 32;
/// 卡片之间的间距（px），模板里的 `gap` 必须与它一致。
pub const CARD_GAP: u32 = 16;
/// 每行几张卡片：固定 4 列。
pub const COLUMNS: u32 = 4;

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

/// 单张卡片的宽度：视口宽度去掉左右内边距与 3 个间距后四等分。
pub fn card_width(width: u32) -> u32 {
    let width = clamp_width(width);
    width.saturating_sub(WALL_PADDING * 2 + CARD_GAP * (COLUMNS - 1)) / COLUMNS
}

/// 把一张卡片渲染成图片，返回 html2img 产出的本机文件路径；`width` 是卡片宽度
/// （px，会夹到 1200–1920）。
///
/// 这是阻塞调用，只能跑在业务线程上；调用方负责在用完之后删掉 `path`
/// （html2img 自己会在 24 小时后清理）。
pub fn render_card(host: &HostApi, card: &CardData, width: u32) -> Result<PathBuf, Error> {
    if TEMPLATE.len() > MAX_HTML_BYTES {
        return Err(Error::TemplateTooLarge(TEMPLATE.len()));
    }
    let width = clamp_width(width);
    let data = view_data(card, width);
    let request = html2img::Request {
        html: TEMPLATE,
        // 卡片是 Vue 模板：循环 / 条件 / 插值都在模板里，Rust 只给数据。
        vue: true,
        data: Some(&data),
        width,
        // 短卡片不被默认的 768px 视口留白撑高；长卡片由 auto_height 撑开。
        height: Some(VIEWPORT_HEIGHT),
        // 与 whois / musiccard 一致：1 CSS px = 1 输出 px。
        scale: 1.0,
        // 卡片是深色底，JPEG 体积小、Telegram 照片上传够用。
        format: "jpg",
        auto_height: true,
        transparent: false,
        dark: true,
    };
    Ok(html2img::render(host, &request)?)
}

/// 卡片模板要的 `data`。
///
/// 模板只读字段，不做任何函数调用或全局对象访问，所以下面这些值先在 Rust 里算好：
/// 千分位、描述截断、相对进度条、语言分布、切换标签的选中态。
///
/// 文本字段不做 HTML 转义：转义是渲染器的事（Vue 插值会转义一次），Rust 再转一次
/// 会得到 `&amp;lt;` 这种双重转义。
pub fn view_data(card: &CardData, width: u32) -> Value {
    let width = clamp_width(width);
    let items: Vec<Value> = card
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let rank = index + 1;
            json!({
                "rank": rank,
                "rank_label": format!("{rank:02}"),
                "owner": item.owner.trim(),
                "name": item.name.trim(),
                "desc": model::truncate(&item.description, model::DESC_MAX_CHARS),
                "lang": if item.language.trim().is_empty() { "未知" } else { item.language.trim() },
                "lang_color": if item.language_color.trim().is_empty() { "#8b949e" } else { item.language_color.trim() },
                "stars": thousands(item.stars),
                "stars_short": model::compact(item.stars),
                "forks": thousands(item.forks),
                "forks_short": model::compact(item.forks),
                "gain": format!("+{}", thousands(item.gained)),
                // 头像：抓到了就是 data URI，没抓到模板用 initial 兜底。
                "avatar": item.avatar.trim(),
                "initial": initial(&item.name),
            })
        })
        .collect();

    json!({
        "width": width,
        "card_width": card_width(width),
        "period": card.period.label(),
        "period_en": card.period.en(),
        "window_label": card.period.window_label(),
        "gain_label": card.period.gain_label(),
        "fetched_at": card.fetched_at.trim(),
        "count": card.items.len(),
        "items": items,
    })
}

/// 头像兜底用的首字母：仓库名的第一个字符，大写（中文原样）；空名给 `?`。
fn initial(name: &str) -> String {
    match name.trim().chars().next() {
        Some(ch) => ch.to_uppercase().collect(),
        None => "?".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Period, TrendingItem};

    fn item(
        name: &str,
        language: &str,
        color: &str,
        stars: u64,
        forks: u64,
        gained: u64,
    ) -> TrendingItem {
        TrendingItem {
            owner: "ag-ui-protocol".to_owned(),
            name: name.to_owned(),
            description: "AG-UI: the Agent-User Interaction Protocol.".to_owned(),
            language: language.to_owned(),
            language_color: color.to_owned(),
            stars,
            forks,
            gained,
            avatar: String::new(),
        }
    }

    fn card() -> CardData {
        CardData::build(
            Period::Today,
            vec![
                item("ag-ui", "TypeScript", "#3178c6", 18_364, 1_892, 1_234),
                item("rust", "", "", 100_000, 13_000, 617),
            ],
            "2026-11-16 20:50".to_owned(),
        )
    }

    #[test]
    fn card_width_matches_the_four_column_grid() {
        assert_eq!(card_width(1600), 372);
        assert_eq!(card_width(1200), 272);
        // 夹取后与边界一致。
        assert_eq!(card_width(400), card_width(1200));
    }

    #[test]
    fn view_data_has_every_template_field() {
        let data = view_data(&card(), 1600);
        assert_eq!(data["width"], 1600);
        assert_eq!(data["card_width"], 372);
        assert_eq!(data["period"], "今日");
        assert_eq!(data["period_en"], "TODAY");
        assert_eq!(data["window_label"], "过去 24 小时");
        assert_eq!(data["gain_label"], "今日");
        assert_eq!(data["count"], 2);

        let first = &data["items"][0];
        assert_eq!(first["rank"], 1);
        assert_eq!(first["rank_label"], "01");
        assert_eq!(first["owner"], "ag-ui-protocol");
        assert_eq!(first["stars"], "18,364");
        assert_eq!(first["stars_short"], "18.4k");
        assert_eq!(first["forks_short"], "1.9k");
        assert_eq!(first["gain"], "+1,234");
        // 没抓到头像：空串，模板用首字母兜底。
        assert_eq!(first["avatar"], "");
        assert_eq!(first["initial"], "A");
        // 进度条已从契约里移除。
        assert!(first.get("bar").is_none());
    }

    #[test]
    fn missing_language_and_avatar_fall_back() {
        let data = view_data(&card(), 1600);
        let second = &data["items"][1];
        assert_eq!(second["lang"], "未知");
        assert_eq!(second["lang_color"], "#8b949e");
        assert_eq!(second["stars_short"], "100k");
        assert_eq!(second["rank_label"], "02");
    }

    #[test]
    fn avatar_data_uri_passes_through() {
        let mut card = card();
        card.items[0].avatar = "data:image/png;base64,AAAA".to_owned();
        let data = view_data(&card, 1600);
        assert_eq!(data["items"][0]["avatar"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn initial_handles_edge_names() {
        assert_eq!(initial("ag-ui"), "A");
        assert_eq!(initial("  rust"), "R");
        assert_eq!(initial("中文仓库"), "中");
        assert_eq!(initial(""), "?");
    }
}
