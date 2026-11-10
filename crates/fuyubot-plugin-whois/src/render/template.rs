//! 卡片模板：把 [`CardData`] 变成一整页 HTML。
//!
//! 卡片版面由固定宽度的双列信息区组成：
//!
//! ```text
//! ┌ 白底圆角卡片，p-6（24px），gap 16px ─────────────────────────────┐
//! │ [64×64 渐变方块] 域名 24px 粗体                    [状态胶囊 12px] │
//! │ ┌ 左列 568px ──────────────┐  ┌ 右列 568px ──────────────┐        │
//! │ │ 域名信息 / 服务器信息 / 其它信息 │ 注册人信息 / 域名状态 / DNS │  │
//! │ └──────────────────────────┘  └──────────────────────────┘        │
//! │              查询api来自 whoiscx.com 的公益接口（12px 灰）          │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! 几条必须遵守的 hyper-render 0.2.2 现状（都有上游原因，不是随手写法）：
//!
//! 1. **样式全部内联**：`<style>` 里的选择器不保证生效，所以不写 class；
//! 2. **flex 容器里不留空白文本节点**：模板是用 `String::push_str` 拼出来的单行
//!    HTML，标签之间没有换行/缩进 —— Blitz 会把它们当匿名 flex item，凭空多出高度；
//! 3. **宽度写死像素**：两列、信息行、状态条都用 `column_width()` /
//!    `box_inner_width()` 算出的像素值，不依赖 `flex:1` 或百分比（Blitz 对
//!    「flex-grow + 百分比子元素」算出来的宽度偏大，长文本会顶出卡片）；
//! 4. **只认系统字体**：`@font-face` 未支持，字体链见 [`FONT_SANS`] / [`FONT_MONO`]。
//!
//! 另外还有一个纯 Blitz 的坑（踩过并已绕开）：**flex 列里再套一个自动高度的
//! flex 行**，外层高度会被算多 30px（实测 64px 的头部行被撑到 94px，图标下移、
//! 整张卡片多出一条空白带）。给胶囊外面那层 flex 行显式写 `height:24px` 即可。
//!
//! 版面约定：
//!
//! - 空值兜底：通用字段空 → `—`，IP / 运营商空 → `N/A`，注册人空 → `已开启隐私保护`；
//! - 等宽字体使用 `Cascadia Mono` / `Consolas`，并在链尾补中文雅黑；
//! - 未注册域名隐藏左列，右列占满整行；
//! - 状态 / DNS 条目使用 20px 行高，给 12px 等宽字和长串折行留出空间。

use crate::core::favicon::Icon;
use crate::core::model::CardData;
use crate::render::assets;

/// 卡片内边距。
pub const PAGE_PADDING: u32 = 24;
/// 卡片内各段（头部 / 主体 / 页脚）之间的间距。
pub const SECTION_GAP: u32 = 16;
/// 两列之间的间距。
pub const COLUMN_GAP: u32 = 16;
/// 信息框内边距。
pub const BOX_PADDING: u32 = 16;
/// 信息框圆角。
pub const BOX_RADIUS: u32 = 12;
/// 信息行左侧标签列的宽度。
///
/// Blitz 对「无宽度的 flex 行 + 长值」会算出偏大的最小宽度，所以这里把标签列
/// 固定为 120px，值列使用剩余宽度，两端对齐由 `text-align:right` 实现。
/// 120px 是量出来的：最长的标签「Whois服务器:」在 16px 下约 108px。
pub const LABEL_WIDTH: u32 = 120;

/// 正文字体链：`Segoe UI` 管拉丁文，`Microsoft YaHei` 管中文，缺字回退无衬线。
/// Emoji 放在 Symbol 字体前，避免彩色 emoji 被渲染成黑白线稿。
pub const FONT_SANS: &str = "'Segoe UI','Segoe UI Emoji','Segoe UI Symbol','Microsoft YaHei','Microsoft YaHei UI',sans-serif";
/// 等宽字体链，用于域名状态和 DNS 服务器。
///
/// **必须带中文回退**：接口返回的状态串是「`clientDeleteProhibited （注册商设置禁止删除）`」
/// 这种「ASCII 状态码 + 全角括号 + 中文说明」的混排，只写 `Cascadia Mono/Consolas`
/// 的话全角 `（` 会变成豆腐块（实测：等宽字体没有 U+FF08 字形，hyper-render 也不会
/// 顺着字体族末尾的 generic 回退去补字形）。把雅黑排在 Consolas 之后就正常了
/// （动手前用一次性探针把每个候选字体链都渲了一遍验证）。
pub const FONT_MONO: &str =
    "'Cascadia Mono','Consolas','Microsoft YaHei','Microsoft YaHei UI',monospace";

/// 信息行文字：16px / 24px。
const ROW_FONT: u32 = 16;
const ROW_LINE_HEIGHT: u32 = 24;
/// 信息框标题：16px / 24px 粗体。
const TITLE_FONT: u32 = 16;
const TITLE_LINE_HEIGHT: u32 = 24;
/// 标题与行列表之间的间距。
const TITLE_MARGIN_BOTTOM: u32 = 12;
/// 域名：24px / 32px 特粗。
const DOMAIN_FONT: u32 = 24;
const DOMAIN_LINE_HEIGHT: u32 = 32;
/// 状态胶囊：12px / 16px。
const BADGE_FONT: u32 = 12;
const BADGE_LINE_HEIGHT: u32 = 16;
/// 胶囊上下各 4px padding 后的总高度。
///
/// **必须写死**：Blitz 在「flex 列里再套一个自动高度的 flex 行」时会把外层的
/// 高度算多 30px（实测 64px 的头部行被撑到 94px，图标因此下移、整张卡片多出
/// 一条空白带）。给胶囊外面这层 flex 行显式写 `height:24px` 之后布局就正常了。
const BADGE_HEIGHT: u32 = 24;
/// 胶囊与域名之间的垂直间距。
const BADGE_MARGIN_TOP: u32 = 6;
/// 头部渐变方块的边长。
const ICON_BOX: u32 = 64;
/// 渐变方块与域名文字的间距。
const HEADER_GAP: u32 = 16;
/// 页脚：12px / 16px + 上下 4px padding。
const FOOTER_FONT: u32 = 12;
const FOOTER_LINE_HEIGHT: u32 = 16;
const FOOTER_PADDING: u32 = 4;
/// 域名状态 / DNS 每一条：12px 等宽，行内 padding 10px。
const ITEM_FONT: u32 = 12;
const ITEM_LINE_HEIGHT: u32 = 20;
const ITEM_PADDING: u32 = 10;
/// 信息行之间的间距：域名信息框用 16px，其余用 8px。
const ROW_GAP_LOOSE: u32 = 16;
const ROW_GAP_TIGHT: u32 = 8;

/// 通用空值兜底。
pub const EMPTY: &str = "—";
/// IP / 归属地 / 运营商的空值兜底。
pub const NA: &str = "N/A";
/// 注册人 / 邮箱为空时的文案。
pub const PRIVACY: &str = "已开启隐私保护";
/// 域名状态 / DNS 列表为空时的占位行。
pub const NO_DATA: &str = "暂无数据";

/// 文字颜色。
const COLOR_LABEL: &str = "#6b7280";
const COLOR_VALUE: &str = "#1f2937";
const COLOR_TITLE: &str = "#1f2937";
const COLOR_ORANGE: &str = "#ea580c";
const COLOR_FOOTER: &str = "#a3a3a3";
/// 状态胶囊：绿色（已注册 / 未注册）与红色（已过期 / 未过期）。
const BADGE_GREEN_BG: &str = "#dcfce7";
const BADGE_GREEN_FG: &str = "#166534";
const BADGE_RED_BG: &str = "#fee2e2";
const BADGE_RED_FG: &str = "#991b1b";

/// 卡片可用宽度（去掉左右各 `PAGE_PADDING`）。
pub fn content_width(width: u32) -> u32 {
    width.saturating_sub(2 * PAGE_PADDING)
}

/// 单列宽度：`(width - 48 - 16) / 2`（1200 时 568）。
///
/// 内部先夹到 [`super::clamp_width`] 的范围：正常链路里 [`build`] / [`render_card`]
/// 已经夹过一次，这里再夹是为了让单独调用它的地方（测试、调样式）也拿到确定几何。
pub fn column_width(width: u32) -> u32 {
    let width = super::clamp_width(width);
    (content_width(width).saturating_sub(COLUMN_GAP) / 2).max(200)
}

/// 信息框内容宽度（列宽去掉左右 padding）。
pub fn box_inner_width(column: u32) -> u32 {
    column.saturating_sub(2 * BOX_PADDING).max(120)
}

/// 生成整页 HTML。`width` 是卡片宽度（px），内部会先按 [`super::clamp_width`] 夹取。
///
/// `icon` 是抓到的网站图标：有就画在头部方块里，没有就回退到内置的渐变地球。
pub fn build(card: &CardData, width: u32, icon: Option<&Icon>) -> String {
    let width = super::clamp_width(width);
    let content = content_width(width);
    let column = column_width(width);

    let mut html = String::with_capacity(16 * 1024);
    html.push_str(&format!(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"><title>whois</title></head><body style="margin:0;padding:0;background:#ffffff;font-size:16px;font-family:{FONT_SANS};"><div style="display:flex;flex-direction:column;align-items:stretch;box-sizing:border-box;width:{width}px;padding:{PAGE_PADDING}px;background:#ffffff;gap:{SECTION_GAP}px;">"##
    ));
    html.push_str(&header(card, content, icon));

    html.push_str(&format!(
        r##"<div style="display:flex;flex-direction:row;align-items:flex-start;width:{content}px;gap:{COLUMN_GAP}px;">"##
    ));
    if card.is_available == 0 {
        html.push_str(&left_column(card, column));
    }
    // 未注册时左列整列隐藏，右列占满整行。
    let right_column_width = if card.is_available == 0 {
        column
    } else {
        content
    };
    html.push_str(&right_column(card, right_column_width));
    html.push_str("</div>");

    html.push_str(&footer(content));
    html.push_str("</div></body></html>");
    html
}

/// 头部：图标 + 域名 + 状态胶囊。
///
/// 图标优先用抓到的网站图标（浅灰底圆角方块 + 等比缩放的 `<image>`）；拿不到就回退
/// 到内置的渐变地球方块，两种情况下方块尺寸与位置完全一致。
fn header(card: &CardData, content: u32, icon: Option<&Icon>) -> String {
    let text_width = content.saturating_sub(ICON_BOX + HEADER_GAP);
    let domain = display(&card.domain, EMPTY);
    let (badge_bg, badge_fg) = if card.status_is_danger() {
        (BADGE_RED_BG, BADGE_RED_FG)
    } else {
        (BADGE_GREEN_BG, BADGE_GREEN_FG)
    };
    // 危险状态使用较重的字重，常规状态保持常规字重。
    let badge_weight = if card.status_is_danger() { 600 } else { 400 };
    let badge = escape(card.status_label());

    // 圆角裁切交给外层方块（`overflow:hidden` + `border-radius`），SVG 里不再写 clipPath。
    let (icon_style, icon_inner) = match icon.and_then(|icon| assets::favicon(icon, ICON_BOX)) {
        Some(svg) => ("background:#f3f4f6;overflow:hidden;".to_owned(), svg),
        None => (
            format!("background-image:{};", assets::HEADER_GRADIENT),
            assets::GLOBE_HEADER.to_owned(),
        ),
    };

    format!(
        r##"<div style="display:flex;flex-direction:row;align-items:center;width:{content}px;"><div style="display:flex;flex-direction:row;align-items:center;justify-content:center;width:{ICON_BOX}px;height:{ICON_BOX}px;border-radius:16px;{icon_style}flex-shrink:0;">{icon_inner}</div><div style="display:flex;flex-direction:column;width:{text_width}px;margin-left:{HEADER_GAP}px;"><span style="display:block;width:{text_width}px;font-size:{DOMAIN_FONT}px;font-weight:800;line-height:{DOMAIN_LINE_HEIGHT}px;color:#111827;word-break:break-word;overflow-wrap:anywhere;">{domain}</span><div style="display:flex;flex-direction:row;align-items:center;justify-content:flex-start;height:{BADGE_HEIGHT}px;margin-top:{BADGE_MARGIN_TOP}px;"><span style="display:flex;flex-direction:row;align-items:center;justify-content:center;padding:4px 12px;border-radius:9999px;font-size:{BADGE_FONT}px;line-height:{BADGE_LINE_HEIGHT}px;font-weight:{badge_weight};color:{badge_fg};background:{badge_bg};flex-shrink:0;">{badge}</span></div></div></div>"##
    )
}

/// 左列：域名信息 / 服务器信息 / 其它信息。
fn left_column(card: &CardData, column: u32) -> String {
    let inner = box_inner_width(column);
    let value_width = value_width(inner);

    let domain_rows = vec![
        row(
            inner,
            value_width,
            "注册商:",
            &display(&card.registrar_name, EMPTY),
            COLOR_VALUE,
        ),
        row(
            inner,
            value_width,
            "注册日期:",
            &display(&card.creation_time, EMPTY),
            COLOR_VALUE,
        ),
        // 到期日期使用橙色强调。
        row(
            inner,
            value_width,
            "到期日期:",
            &display(&card.expiration_time, EMPTY),
            COLOR_ORANGE,
        ),
        row(
            inner,
            value_width,
            "注册天数:",
            &format!("{}天", card.creation_days),
            COLOR_VALUE,
        ),
        row(
            inner,
            value_width,
            "距离到期:",
            &format!("{}天", card.valid_days),
            COLOR_VALUE,
        ),
    ];

    let mut html = format!(
        r##"<div style="display:flex;flex-direction:column;width:{column}px;gap:{SECTION_GAP}px;">"##
    );
    html.push_str(&info_box(
        column,
        "域名信息",
        Some(assets::GLOBE_BLUE),
        ROW_GAP_LOOSE,
        &domain_rows,
    ));

    let ip = display(&card.ip, NA);
    let isp = display(&card.isp, NA);
    let location = ip_location(card);
    let server_rows = vec![
        row(
            inner,
            value_width,
            "域名:",
            &display(&card.domain, EMPTY),
            COLOR_VALUE,
        ),
        row(inner, value_width, "服务器IP:", &ip, COLOR_VALUE),
        row(inner, value_width, "IP归属地:", &location, COLOR_ORANGE),
        row(inner, value_width, "网络运营商:", &isp, COLOR_VALUE),
        row(
            inner,
            value_width,
            "查询时间:",
            &display(&card.query_time, EMPTY),
            COLOR_VALUE,
        ),
    ];
    html.push_str(&info_box(
        column,
        "服务器信息",
        Some(assets::SERVER_BLUE),
        ROW_GAP_TIGHT,
        &server_rows,
    ));

    let other_rows = vec![
        row(
            inner,
            value_width,
            "Whois服务器:",
            &display(&card.whois_server, EMPTY),
            COLOR_VALUE,
        ),
        row(
            inner,
            value_width,
            "域名后缀:",
            &display(&card.domain_suffix, EMPTY),
            COLOR_VALUE,
        ),
    ];
    html.push_str(&info_box(
        column,
        "其它信息",
        Some(assets::INFO_DARK),
        ROW_GAP_TIGHT,
        &other_rows,
    ));
    html.push_str("</div>");
    html
}

/// 右列：注册人信息 / 域名状态 / DNS 服务器。
fn right_column(card: &CardData, column: u32) -> String {
    let inner = box_inner_width(column);
    let value_width = value_width(inner);

    let mut html = format!(
        r##"<div style="display:flex;flex-direction:column;width:{column}px;gap:{SECTION_GAP}px;">"##
    );

    let rows = vec![
        row(
            inner,
            value_width,
            "注册人:",
            &display(&card.registrant_name, PRIVACY),
            COLOR_VALUE,
        ),
        row(
            inner,
            value_width,
            "注册邮箱:",
            &display(&card.registrant_email, PRIVACY),
            COLOR_VALUE,
        ),
    ];
    html.push_str(&info_box(
        column,
        "注册人信息",
        Some(assets::USER_PURPLE),
        ROW_GAP_TIGHT,
        &rows,
    ));

    html.push_str(&item_box(column, "域名状态", &card.domain_status));
    html.push_str(&item_box(column, "DNS服务器", &card.name_server));
    html.push_str("</div>");
    html
}

/// 页脚。
fn footer(content: u32) -> String {
    format!(
        r##"<div style="display:flex;flex-direction:row;align-items:center;justify-content:center;width:{content}px;"><span style="display:block;padding:{FOOTER_PADDING}px;font-size:{FOOTER_FONT}px;line-height:{FOOTER_LINE_HEIGHT}px;color:{COLOR_FOOTER};">查询api来自 whoiscx.com 的公益接口</span></div>"##
    )
}

/// 值列宽度：信息框内容宽去掉标签列。
fn value_width(inner: u32) -> u32 {
    inner.saturating_sub(LABEL_WIDTH).max(80)
}

/// 一条「标签 + 值」信息行。
fn row(inner: u32, value_width: u32, label: &str, value: &str, color: &str) -> String {
    format!(
        r##"<div style="display:flex;flex-direction:row;align-items:flex-start;width:{inner}px;"><span style="display:block;width:{LABEL_WIDTH}px;flex-shrink:0;font-size:{ROW_FONT}px;line-height:{ROW_LINE_HEIGHT}px;color:{COLOR_LABEL};">{label}</span><span style="display:block;width:{value_width}px;font-size:{ROW_FONT}px;line-height:{ROW_LINE_HEIGHT}px;font-weight:600;color:{color};text-align:right;word-break:break-word;overflow-wrap:anywhere;">{value}</span></div>"##
    )
}

/// 一个信息框（可选标题图标）。
fn info_box(column: u32, title: &str, icon: Option<&str>, row_gap: u32, rows: &[String]) -> String {
    let inner = box_inner_width(column);
    format!(
        r##"<div style="display:flex;flex-direction:column;box-sizing:border-box;width:{column}px;padding:{BOX_PADDING}px;background:#f9fafb;border-radius:{BOX_RADIUS}px;">{header}<div style="display:flex;flex-direction:column;width:{inner}px;gap:{row_gap}px;">{rows}</div></div>"##,
        header = box_title(title, icon),
        rows = rows.concat(),
    )
}

/// 标题行：图标 + 16px 粗体标题；无图标时只有标题（域名状态 / DNS 服务器）。
fn box_title(title: &str, icon: Option<&str>) -> String {
    let title_html = format!(
        r##"<span style="display:block;font-size:{TITLE_FONT}px;line-height:{TITLE_LINE_HEIGHT}px;font-weight:bold;color:{COLOR_TITLE};flex-shrink:0;">{title}</span>"##,
        title = escape(title),
    );
    match icon {
        Some(icon) => format!(
            r##"<div style="display:flex;flex-direction:row;align-items:center;width:100%;gap:8px;margin-bottom:{TITLE_MARGIN_BOTTOM}px;">{icon}{title_html}</div>"##
        ),
        None => format!(
            r##"<div style="display:flex;flex-direction:row;align-items:center;width:100%;margin-bottom:{TITLE_MARGIN_BOTTOM}px;">{title_html}</div>"##
        ),
    }
}

/// 列表型信息框（域名状态 / DNS 服务器）：每条一个白底圆角行，12px 等宽字体。
fn item_box(column: u32, title: &str, values: &[String]) -> String {
    let inner = box_inner_width(column);
    let items: Vec<String> = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| item(inner, &escape(value)))
        .collect();
    let rows = if items.is_empty() {
        // 空列表补一行占位，避免卡片出现空框。
        item(inner, &escape(NO_DATA))
    } else {
        items.concat()
    };
    info_box(column, title, None, ROW_GAP_LOOSE, &[rows])
}

/// 列表里的一行（白底圆角，等宽字体）。
fn item(inner: u32, text: &str) -> String {
    let text_width = inner.saturating_sub(2 * ITEM_PADDING).max(40);
    format!(
        r##"<div style="display:flex;flex-direction:row;align-items:center;box-sizing:border-box;width:{inner}px;padding:{ITEM_PADDING}px;background:#ffffff;border-radius:8px;"><span style="display:block;width:{text_width}px;font-family:{FONT_MONO};font-size:{ITEM_FONT}px;line-height:{ITEM_LINE_HEIGHT}px;color:#1f2937;word-break:break-word;overflow-wrap:anywhere;">{text}</span></div>"##
    )
}

/// IP 归属地：`country` + 空格 + `city`，都为空时 `N/A`。
fn ip_location(card: &CardData) -> String {
    let country = card.country.trim();
    let city = card.city.trim();
    let text = match (country.is_empty(), city.is_empty()) {
        (true, true) => String::new(),
        (false, true) => country.to_owned(),
        (true, false) => city.to_owned(),
        (false, false) => format!("{country} {city}"),
    };
    escape(&display(&text, NA))
}

/// 显示用文本：空串换成兜底文案；返回值已做 HTML 转义。
fn display(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        escape(fallback)
    } else {
        escape(trimmed)
    }
}

/// HTML 转义。值来自接口，可能带 `& < > " '`，不转义会被解析成标签。
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 保守估算卡片高度（px），给 [`crate::render::render_card`] 做「渲之前先拦住」用。
///
/// 光栅化的位图是「宽 × 高 × 4 字节」，等渲完再读 PNG 头就太晚了。这里按「每个字符
/// 都占满一个字宽」估算折行（中文确实如此，英文会高估），所以结果只会偏大：
/// 宁可提前报「太长」，也不去分配一张几百 MB 的位图。
pub fn estimate_height(card: &CardData, width: u32) -> u32 {
    let width = super::clamp_width(width);
    let content = content_width(width);
    let column = column_width(width);
    let inner = box_inner_width(column);
    let value_w = value_width(inner);
    let itext_width = inner.saturating_sub(2 * ITEM_PADDING);

    // 每行能放几个「满宽」字符，再折行；至少 1 行。
    let lines = |text: &str, available: u32, font: u32| -> u64 {
        let per_line = (available / font.max(1)).max(1) as u64;
        (text.chars().count() as u64).div_ceil(per_line).max(1)
    };
    let row_height = |text: &str| lines(text, value_w, ROW_FONT) * ROW_LINE_HEIGHT as u64;
    let item_height = |text: &str| {
        lines(text, itext_width, ITEM_FONT) * ITEM_LINE_HEIGHT as u64 + 2 * ITEM_PADDING as u64
    };
    // 一个信息框：上下 padding + 标题行（含 mb）+ 各信息行 + 行间距。
    let box_height = |rows: &[u64], gap: u32| -> u64 {
        let body: u64 = rows.iter().sum::<u64>() + gap as u64 * rows.len().saturating_sub(1) as u64;
        2 * BOX_PADDING as u64 + TITLE_LINE_HEIGHT as u64 + TITLE_MARGIN_BOTTOM as u64 + body
    };

    let left_boxes = [
        box_height(
            &[
                row_height(card.registrar_name.trim()),
                row_height(card.creation_time.trim()),
                row_height(card.expiration_time.trim()),
                ROW_LINE_HEIGHT as u64,
                ROW_LINE_HEIGHT as u64,
            ],
            ROW_GAP_LOOSE,
        ),
        box_height(
            &[
                row_height(card.domain.trim()),
                row_height(card.ip.trim()),
                row_height(&ip_location(card)),
                row_height(card.isp.trim()),
                row_height(card.query_time.trim()),
            ],
            ROW_GAP_TIGHT,
        ),
        box_height(
            &[
                row_height(card.whois_server.trim()),
                row_height(card.domain_suffix.trim()),
            ],
            ROW_GAP_TIGHT,
        ),
    ];
    let left_height = left_boxes.iter().sum::<u64>() + 2 * SECTION_GAP as u64;

    let count_items = |values: &[String]| -> Vec<u64> {
        let rows: Vec<u64> = values
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(item_height)
            .collect();
        if rows.is_empty() {
            vec![item_height(NO_DATA)]
        } else {
            rows
        }
    };
    let status_rows = count_items(&card.domain_status);
    let dns_rows = count_items(&card.name_server);
    let right_boxes = [
        box_height(
            &[
                row_height(card.registrant_name.trim()),
                row_height(card.registrant_email.trim()),
            ],
            ROW_GAP_TIGHT,
        ),
        box_height(&status_rows, ROW_GAP_LOOSE),
        box_height(&dns_rows, ROW_GAP_LOOSE),
    ];
    let right_height = right_boxes.iter().sum::<u64>() + 2 * SECTION_GAP as u64;

    // 头部：图标方块 64px 与「域名 + 胶囊」取高者。
    let domain_lines = lines(
        card.domain.trim(),
        content.saturating_sub(ICON_BOX + HEADER_GAP),
        DOMAIN_FONT,
    );
    let header_height = (domain_lines * DOMAIN_LINE_HEIGHT as u64
        + BADGE_MARGIN_TOP as u64
        + BADGE_LINE_HEIGHT as u64
        + 2 * 4)
        .max(ICON_BOX as u64);

    let columns = if card.is_available == 0 {
        left_height.max(right_height)
    } else {
        // 左列整列隐藏时右列占满整行，可用宽度大不相同，重新估一遍。
        let full_inner = box_inner_width(content);
        let full_value = value_width(full_inner);
        let mut right = 0;
        for rows in [
            count_items(&card.domain_status),
            count_items(&card.name_server),
        ] {
            right += box_height(&rows, ROW_GAP_LOOSE);
        }
        right += 2 * SECTION_GAP as u64;
        right += box_height(
            &[
                lines(card.registrant_name.trim(), full_value, ROW_FONT) * ROW_LINE_HEIGHT as u64,
                lines(card.registrant_email.trim(), full_value, ROW_FONT) * ROW_LINE_HEIGHT as u64,
            ],
            ROW_GAP_TIGHT,
        );
        right
    };

    let total = 2 * PAGE_PADDING as u64
        + 2 * SECTION_GAP as u64
        + header_height
        + columns
        + FOOTER_LINE_HEIGHT as u64
        + 2 * FOOTER_PADDING as u64;
    u32::try_from(total).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 的纯色图标，够编码成 PNG。
    fn icon() -> Icon {
        Icon {
            width: 2,
            height: 2,
            rgba: vec![255; 16],
        }
    }

    #[test]
    fn header_falls_back_to_gradient_without_icon() {
        let html = build(&CardData::default(), 1200, None);
        assert!(html.contains(assets::HEADER_GRADIENT));
        assert!(html.contains(assets::GLOBE_HEADER));
    }

    #[test]
    fn header_uses_favicon_when_available() {
        let html = build(&CardData::default(), 1200, Some(&icon()));
        assert!(html.contains("data:image/png;base64,"));
        assert!(!html.contains(assets::GLOBE_HEADER));
        assert!(!html.contains(assets::HEADER_GRADIENT));
    }

    #[test]
    fn icon_box_geometry_is_unchanged_by_favicon() {
        let html = build(&CardData::default(), 1200, Some(&icon()));
        assert!(html.contains("width:64px;height:64px;border-radius:16px;"));
    }
}
