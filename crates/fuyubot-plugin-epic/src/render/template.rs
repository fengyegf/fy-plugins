//! HTML/CSS 模板生成（纯函数，不碰渲染引擎）。
//!
//! 版式结构：
//!
//! ```text
//! ┌ 页头（高 56）：[礼物图标 32] 免费游戏 22px 粗白 …… [Epic 徽章] Epic Games Store 20px 白
//! │   ↕ 24
//! ├ 每张卡片是一列：[封面（洋红/深灰槽位） + 徽章条] ← 只有这两块在卡片里
//! │                 ↕ 12
//! │                 [标题 16px 白，最多 2 行]
//! │                 ↕ 6
//! │                 [日期 14px 灰]
//! ```
//!
//! 三条必须遵守的约定：
//!
//! 1. **样式全部写在 `style="…"` 上**。hyper-render 底层是 Blitz，`<style>` 里的
//!    选择器支持不完整，内联声明最稳（与 kfc 插件同一约定）。
//! 2. **封面槽位是纯色块**。hyper-render 0.2 不加载任何图片（`data:` URI 与 http
//!    都不行），所以槽位只画一个填色 div：有封面时填洋红 [`SLOT_MARKER_COLOR`]，
//!    供 [`compose::find_slots`](super::compose::find_slots) 定位；没封面时直接
//!    填深灰 [`NO_COVER_COLOR`]，这样「全部无封面」可以跳过整个合成阶段。
//! 3. **标题和日期在卡片容器外面**，直接画在页面背景上（参考图如此），只有封面和
//!    徽章条包在圆角卡片里。
//!
//! [`layout`] 里的几何计算同时被 [`crate::render::card`] 的兜底坐标复用，
//! 改尺寸常量时两边不会走偏。

use crate::core::epic::FreeGame;
use crate::render::RenderOptions;

/// 页头高度（CSS 像素）。
pub const HEADER_HEIGHT: u32 = 56;
/// 页头与卡片之间的间距。
pub const HEADER_GAP: u32 = 24;
/// 徽章条高度：卡片的下半部分。
pub const BADGE_HEIGHT: u32 = 22;
/// 卡片圆角（CSS 像素）；封面顶部两角、徽章底部两角同半径。
pub const CARD_RADIUS: u32 = 12;
/// 卡片（封面+徽章）与标题的间距。
pub const COVER_TITLE_GAP: u32 = 12;
/// 标题与日期的间距。
pub const TITLE_DATE_GAP: u32 = 6;
/// 标题区高度（两行 × 行高 22）。
pub const TITLE_HEIGHT: u32 = 44;
/// 日期行高度。
pub const DATE_HEIGHT: u32 = 20;
/// 行间距（多行时）。
pub const ROW_GAP: u32 = 30;
/// 空列表提示区高度。
pub const EMPTY_BODY_HEIGHT: u32 = 100;

/// 标题字号。
const TITLE_FONT_SIZE: u32 = 16;
/// 标题行高。
const TITLE_LINE_HEIGHT: u32 = 22;
/// 标题最多几行。
const TITLE_MAX_LINES: usize = 2;
/// 日期字号。
const DATE_FONT_SIZE: u32 = 14;
/// 日期最多保留多少个字符（防御异常长的接口数据，正常只有 20 出头）。
const DATE_MAX_CHARS: usize = 40;
/// 徽章条字号。
const BADGE_FONT_SIZE: u32 = 13;

/// 页头礼物图标尺寸（画布够宽时）。
const HEADER_ICON_SIZE: u32 = 32;
/// 礼物图标与「免费游戏」的间距。
const HEADER_ICON_GAP: u32 = 12;
/// 「免费游戏」字号与估算宽度（4 个全角字 + 余量）。
const HEADER_TITLE_FONT: u32 = 22;
/// 「Epic Games Store」字号与估算宽度。
const HEADER_RIGHT_FONT: u32 = 20;
/// Epic 徽章宽度与它和文字的间距。
const EPIC_BADGE_WIDTH: u32 = 30;
const EPIC_BADGE_GAP: u32 = 10;

/// 有封面、待贴图：洋红占位，合成阶段按颜色定位。
pub const SLOT_MARKER_COLOR: &str = "#ff00ff";
/// 无封面：深灰占位，直接留在这张图里。
pub const NO_COVER_COLOR: &str = "#3a3a42";

/// 卡片底色（封面和徽章会盖住，只在圆角过渡处露出一点）。
const CARD_BG: &str = "#2b2b31";
/// 标题颜色。
const TITLE_COLOR: &str = "#ffffff";
/// 日期颜色。
const DATE_COLOR: &str = "#a3a3a3";
/// 页头标题颜色（白、粗体）。
const HEADER_TITLE_COLOR: &str = "#ffffff";
/// 页头右侧文字颜色（参考图是白色）。
const HEADER_TEXT_COLOR: &str = "#ffffff";
/// 当前免费的徽章：天蓝底黑字。
const BADGE_CURRENT_BG: &str = "#38bdf8";
const BADGE_CURRENT_FG: &str = "#000000";
/// 即将推出：黑底白字。
const BADGE_UPCOMING_BG: &str = "#000000";
const BADGE_UPCOMING_FG: &str = "#ffffff";
/// 页头右侧固定文案。
const STORE_LABEL: &str = "Epic Games Store";

/// 字体栈。
///
/// **必须让排在首位的字体同时覆盖拉丁与 CJK**。hyper-render（Blitz）在
/// 「拉丁文字后紧跟 CJK 标点」时字体回退会挑错字体，`《》` 会变成豆腐块
/// （实测：`font-family:sans-serif` 下 `Upcoming 0 《标题》` 的 `《` 是方块，
/// 换成 `Microsoft YaHei` 就正常）。所以这里显式指定带完整中西文覆盖的字体，
/// 后面再补几个跨平台候选兜底。注意只能用单引号：整段写在双引号包裹的
/// `style="…"` 里，双引号会提前闭合属性。
const FONT_FAMILY: &str = "'Microsoft YaHei','SimHei','PingFang SC','Noto Sans CJK SC',sans-serif";

/// 页头礼物图标（原 Epic 礼物 SVG，`fill` 已固定成白色）。
const GIFT_SVG: &str = include_str!("../../assets/gift.svg");

/// Epic 官方 logo。
///
/// 关键点：path 必须带 `fill-rule="evenodd"`，否则字母子路径会被一起填白、
/// 整个标记退化成一块白方块（实测：不加这个属性时 `EPIC/GAMES` 完全看不见）。
/// 资产里已经把 `width/height` 去掉，交给容器定尺寸（viewBox 等比缩放）。
const EPIC_BADGE: &str = include_str!("../../assets/epic-logo.svg");

/// 页头里 logo 的容器尺寸（正方形，官方标记本身就是 32×32 的 viewBox）。
const EPIC_LOGO_BOX: u32 = 30;

/// 一张卡片需要的全部信息（由 `card.rs` 组装，与 `covers` 下标一一对应）。
pub struct Card<'a> {
    /// 游戏数据。
    pub game: &'a FreeGame,
    /// 是否属于「即将免费」（决定徽章样式与日期行格式）。
    pub upcoming: bool,
    /// 有没有可贴的封面字节（决定槽位颜色）。
    pub has_cover: bool,
}

/// 画布几何（CSS 像素）。位图尺寸 = 这里 × `options.scale`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// 每行卡片数（空列表时为 1）。
    pub cols: usize,
    /// 行数（空列表时为 0）。
    pub rows: usize,
    /// 画布宽度（CSS 像素）。
    pub width: u32,
    /// 画布高度（CSS 像素）。
    pub height: u32,
    /// 卡片本体高度：封面 + 徽章条。
    pub card_height: u32,
    /// 整列高度：卡片 + 标题 + 日期（含列间距）。
    pub column_height: u32,
}

/// 槽位矩形（CSS 像素，浮点便于按 scale 直接换算成设备像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// 计算画布几何。
///
/// 宽 = `padding*2 + cols*card_width + (cols-1)*gap`
/// 高 = `padding*2 + 页头 + 24 + rows*列高 + (rows-1)*行距`
/// 列高 = `cover_height + 22(徽章) + 12 + 44(标题) + 6 + 20(日期)`
pub fn layout(card_count: usize, options: &RenderOptions) -> Layout {
    let padding = options.padding;
    let card_width = options.card_width.max(1);
    let cover_height = options.cover_height.max(1);
    let gap = options.gap;
    let card_height = cover_height.saturating_add(BADGE_HEIGHT);
    let column_height = card_height
        .saturating_add(COVER_TITLE_GAP)
        .saturating_add(TITLE_HEIGHT)
        .saturating_add(TITLE_DATE_GAP)
        .saturating_add(DATE_HEIGHT);

    if card_count == 0 {
        return Layout {
            cols: 1,
            rows: 0,
            width: padding.saturating_mul(2).saturating_add(card_width),
            height: padding
                .saturating_mul(2)
                .saturating_add(HEADER_HEIGHT)
                .saturating_add(HEADER_GAP)
                .saturating_add(EMPTY_BODY_HEIGHT),
            card_height,
            column_height,
        };
    }

    let cols = card_count.min(options.max_per_row.max(1) as usize).max(1);
    let rows = card_count.div_ceil(cols);
    let width = padding
        .saturating_mul(2)
        .saturating_add((cols as u32).saturating_mul(card_width))
        .saturating_add((cols as u32 - 1).saturating_mul(gap));
    let height = padding
        .saturating_mul(2)
        .saturating_add(HEADER_HEIGHT)
        .saturating_add(HEADER_GAP)
        .saturating_add((rows as u32).saturating_mul(column_height))
        .saturating_add((rows as u32 - 1).saturating_mul(ROW_GAP));

    Layout {
        cols,
        rows,
        width,
        height,
        card_height,
        column_height,
    }
}

impl Layout {
    /// 第 `index` 张卡片封面槽位的位置（CSS 像素）。
    ///
    /// 槽位是卡片的第一块内容，所以纵向偏移 = padding + 页头 + 页头间距 + 整列偏移。
    /// 槽位检测失败时，`card.rs` 用这份解析式坐标兜底。
    pub fn slot_rect(&self, index: usize, options: &RenderOptions) -> SlotRect {
        let cols = self.cols.max(1);
        let (col, row) = (index % cols, index / cols);
        let x = options
            .padding
            .saturating_add((col as u32).saturating_mul(options.card_width + options.gap));
        let y = options
            .padding
            .saturating_add(HEADER_HEIGHT)
            .saturating_add(HEADER_GAP)
            .saturating_add((row as u32).saturating_mul(self.column_height + ROW_GAP));
        SlotRect {
            x: x as f32,
            y: y as f32,
            width: options.card_width as f32,
            height: options.cover_height as f32,
        }
    }
}

/// 生成整页 HTML。
///
/// `cards` 为空时渲染空列表提示（`options.empty_text`），页头照常显示。
pub fn document(cards: &[Card<'_>], layout: &Layout, options: &RenderOptions) -> String {
    let background = css_color(options.background);
    let header = header_html(layout, options);
    let body = if cards.is_empty() {
        empty_body(options)
    } else {
        rows_html(cards, layout, options)
    };

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>{page_title}</title></head>
<body style="margin:0;padding:0;background:{background};color:#ffffff;font-family:{font};font-size:14px;">
<div style="display:flex;flex-direction:column;width:{width}px;height:{height}px;padding:{padding}px;box-sizing:border-box;background:{background};">
{header}
<div style="height:{header_gap}px;width:100%;"></div>
{body}
</div>
</body></html>"#,
        page_title = escape(&options.title),
        background = background,
        font = FONT_FAMILY,
        width = layout.width,
        height = layout.height,
        padding = options.padding,
        header = header,
        header_gap = HEADER_GAP,
        body = body,
    )
}

/// 页头度量（CSS 像素）。
///
/// 引擎既不会自己收缩文字块，也不一定支持 `white-space:nowrap`，所以宽度必须
/// 由我们自己算准：画布够宽就用配置字号（22 / 20），不够（只剩 1 张卡片时的窄
/// 画布）就按比例缩字号，保证页头永远一行放得下、不折行也不溢出。
#[derive(Debug, Clone, Copy)]
struct HeaderMetrics {
    icon: u32,
    icon_gap: u32,
    title_font: u32,
    title_width: u32,
    right_font: u32,
    right_text_width: u32,
}

impl HeaderMetrics {
    /// 页头左块（图标 + 标题）占的宽度。
    fn left_width(&self) -> u32 {
        self.icon + self.icon_gap + self.title_width
    }
}

fn header_metrics(inner: u32) -> HeaderMetrics {
    // 「免费游戏」= 4 个全角字；「Epic Games Store」实测约 8.1em，按 8.3em 预留余量。
    let title_width = HEADER_TITLE_FONT * 4 + 8;
    let right_text_width = HEADER_RIGHT_FONT * 83 / 10;
    let fixed = HEADER_ICON_SIZE + HEADER_ICON_GAP + EPIC_BADGE_WIDTH + EPIC_BADGE_GAP;
    let available = inner.saturating_sub(fixed);

    if available >= title_width + right_text_width {
        return HeaderMetrics {
            icon: HEADER_ICON_SIZE,
            icon_gap: HEADER_ICON_GAP,
            title_font: HEADER_TITLE_FONT,
            title_width,
            right_font: HEADER_RIGHT_FONT,
            right_text_width,
        };
    }

    // 窄画布：左右两块等比缩小，字号设下限，右边留 4px 余量。
    let ratio = available as f32 / (title_width + right_text_width) as f32;
    let title_font = ((HEADER_TITLE_FONT as f32 * ratio).floor() as u32).max(12);
    let right_font = ((HEADER_RIGHT_FONT as f32 * ratio).floor() as u32).max(10);
    let title_width = title_font * 4 + 4;
    let right_text_width = available
        .saturating_sub(title_width + 4)
        .max(right_font * 4);
    HeaderMetrics {
        icon: HEADER_ICON_SIZE,
        icon_gap: HEADER_ICON_GAP,
        title_font,
        title_width,
        right_font,
        right_text_width,
    }
}

/// 页头：左「礼物图标 + 免费游戏」，右「Epic 徽章 + Epic Games Store」。
///
/// 文字块全部定宽（引擎不会自己收缩，靠 `flex-grow` 占位把右侧推到底）；
/// 画布很窄时按 [`header_metrics`] 缩字号，宁可字小也不折行。
fn header_html(layout: &Layout, options: &RenderOptions) -> String {
    let inner = layout
        .width
        .saturating_sub(options.padding.saturating_mul(2));
    let metrics = header_metrics(inner);
    let left_width = metrics.left_width();

    format!(
        r#"<div style="display:flex;flex-direction:row;align-items:center;width:100%;height:{header}px;">
<div style="display:flex;flex-direction:row;align-items:center;width:{left}px;height:{header}px;">
<div style="width:{icon}px;height:{icon}px;margin-right:{icon_gap}px;">{gift}</div>
<div style="width:{title_width}px;height:{header}px;line-height:{header}px;font-size:{title_font}px;font-weight:bold;color:{title_color};white-space:nowrap;">{title}</div>
</div>
<div style="flex-grow:1;height:{header}px;"></div>
<div style="display:flex;flex-direction:row;align-items:center;height:{header}px;">
<div data-epic-logo="1" style="width:{logo_box}px;height:{logo_box}px;">{badge}</div>
<div style="width:{right_text}px;height:{header}px;line-height:{header}px;margin-left:{badge_gap}px;font-size:{right_font}px;color:{text_color};text-align:right;white-space:nowrap;">{store}</div>
</div>
</div>"#,
        header = HEADER_HEIGHT,
        left = left_width,
        icon = metrics.icon,
        icon_gap = metrics.icon_gap,
        gift = GIFT_SVG,
        title_width = metrics.title_width,
        title_font = metrics.title_font,
        title_color = HEADER_TITLE_COLOR,
        title = escape(&options.title),
        badge = EPIC_BADGE,
        logo_box = EPIC_LOGO_BOX,
        right_text = metrics.right_text_width,
        right_font = metrics.right_font,
        badge_gap = EPIC_BADGE_GAP,
        text_color = HEADER_TEXT_COLOR,
        store = STORE_LABEL,
    )
}

/// 按行渲染卡片列；行内列用 `margin-right` 拉开间距（不依赖 flex `gap`）。
fn rows_html(cards: &[Card<'_>], layout: &Layout, options: &RenderOptions) -> String {
    let cols = layout.cols.max(1);
    let mut html = String::with_capacity(cards.len() * 1024);
    for (row_index, chunk) in cards.chunks(cols).enumerate() {
        let margin = if row_index == 0 { 0 } else { ROW_GAP };
        html.push_str(&format!(
            r#"<div style="display:flex;flex-direction:row;width:100%;margin-top:{margin}px;">"#
        ));
        for (col_index, card) in chunk.iter().enumerate() {
            let last = col_index + 1 == chunk.len();
            html.push_str(&column_html(
                card,
                options,
                layout.column_height,
                if last { 0 } else { options.gap },
            ));
        }
        html.push_str("</div>\n");
    }
    html
}

/// 一列：卡片（封面 + 徽章）→ 12 → 标题 → 6 → 日期。
///
/// `min-height` 固定成布局预留的列高：标题只有一行时列高不会被压缩，行距和
/// 画布高度才是确定的（否则单行标题会让下一行整体上移）。
fn column_html(
    card: &Card<'_>,
    options: &RenderOptions,
    column_height: u32,
    margin_right: u32,
) -> String {
    let width = options.card_width.max(1);
    format!(
        r#"<div data-column="1" style="display:flex;flex-direction:column;width:{width}px;min-height:{column_height}px;margin-right:{margin}px;">
{card}
{caption}
</div>"#,
        width = width,
        column_height = column_height,
        margin = margin_right,
        card = card_html(card, options),
        caption = caption_html(card, options),
    )
}

/// 卡片本体：**只有**封面槽位 + 徽章条，圆角靠 `overflow:hidden` 和两端显式圆角。
fn card_html(card: &Card<'_>, options: &RenderOptions) -> String {
    let width = options.card_width.max(1);
    let cover_height = options.cover_height.max(1);
    let radius = CARD_RADIUS;
    let slot_color = if card.has_cover {
        SLOT_MARKER_COLOR
    } else {
        NO_COVER_COLOR
    };
    let (badge_bg, badge_fg, badge_text) = if card.upcoming {
        (BADGE_UPCOMING_BG, BADGE_UPCOMING_FG, "即将推出")
    } else {
        (BADGE_CURRENT_BG, BADGE_CURRENT_FG, "免费游戏")
    };

    format!(
        r#"<div data-card="1" style="display:flex;flex-direction:column;width:{width}px;background:{card_bg};border-radius:{radius}px;overflow:hidden;">
<div data-slot="cover" style="width:{width}px;height:{cover_height}px;background:{slot_color};border-radius:{radius}px {radius}px 0 0;"></div>
<div data-badge="1" style="width:{width}px;height:{badge_height}px;line-height:{badge_height}px;text-align:center;background:{badge_bg};color:{badge_fg};font-size:{badge_font}px;border-radius:0 0 {radius}px {radius}px;">{badge_text}</div>
</div>"#,
        width = width,
        card_bg = CARD_BG,
        radius = radius,
        cover_height = cover_height,
        slot_color = slot_color,
        badge_height = BADGE_HEIGHT,
        badge_bg = badge_bg,
        badge_fg = badge_fg,
        badge_font = BADGE_FONT_SIZE,
        badge_text = badge_text,
    )
}

/// 卡片下方浮在页面背景上的文字：标题（最多两行）+ 日期。
///
/// 标题**不写死高度**：只占实际行数（1 行 22 / 2 行 44），日期紧跟其后——参考图里
/// 单行标题的日期就贴在标题下面，而不是隔着一个空行。`max-height` 兜住异常长的
/// 标题（超出两行的部分裁掉），画布高度仍按两行预留，所以不会溢出画布。
fn caption_html(card: &Card<'_>, options: &RenderOptions) -> String {
    let width = options.card_width.max(1);
    let title = escape(&fit_title(&card.game.title, title_budget(width)));
    let date = escape(&truncate_chars(&date_text(card), DATE_MAX_CHARS));

    format!(
        r#"<div data-title="1" style="width:{width}px;max-height:{title_height}px;margin-top:{title_gap}px;font-size:{title_font}px;line-height:{title_line}px;color:{title_color};overflow:hidden;">{title}</div>
<div data-date="1" style="width:{width}px;height:{date_height}px;margin-top:{date_gap}px;font-size:{date_font}px;line-height:{date_height}px;color:{date_color};overflow:hidden;white-space:nowrap;">{date}</div>"#,
        width = width,
        title_height = TITLE_HEIGHT,
        title_gap = COVER_TITLE_GAP,
        title_font = TITLE_FONT_SIZE,
        title_line = TITLE_LINE_HEIGHT,
        title_color = TITLE_COLOR,
        title = title,
        date_height = DATE_HEIGHT,
        date_gap = TITLE_DATE_GAP,
        date_font = DATE_FONT_SIZE,
        date_color = DATE_COLOR,
        date = date,
    )
}

/// 空列表：居中一行灰字提示。
fn empty_body(options: &RenderOptions) -> String {
    format!(
        r#"<div style="height:{height}px;width:100%;line-height:{height}px;text-align:center;font-size:16px;color:{muted};">{text}</div>"#,
        height = EMPTY_BODY_HEIGHT,
        muted = DATE_COLOR,
        text = escape(&options.empty_text),
    )
}

/// 日期行文案：
///
/// - 当前免费：`当前免费 {end_date} 23:00截止`
/// - 即将免费：`免费{start_date} - {end_date}`（「免费」后没有空格，日期之间是 ` - `）
fn date_text(card: &Card<'_>) -> String {
    if card.upcoming {
        format!("免费{} - {}", card.game.start_date, card.game.end_date)
    } else if card.game.end_date.is_empty() {
        "当前免费".to_owned()
    } else {
        format!("当前免费 {} 23:00截止", card.game.end_date)
    }
}

/// 标题的宽度预算，单位是「全角字符数」：两行，每行 = floor(卡片宽 / 字号)。
///
/// 标题与卡片同宽（不内缩），所以 280px / 16px = 17.5，一行实际只放得下 17 个
/// 全角字，取 floor 留 1 字符余量，宁可早点截断也不要让第三行被裁掉。
fn title_budget(card_width: u32) -> f32 {
    let per_line = card_width.max(1) / TITLE_FONT_SIZE.max(1);
    (per_line.max(1) * TITLE_MAX_LINES as u32) as f32
}

/// 按「字符宽度」截断标题并补省略号。
///
/// 不依赖 CSS `text-overflow: ellipsis`——hyper-render 不一定支持，而且两行溢出
/// 的表现也不可控；这里按全角/半角宽度估算截断点，保证不会超出标题区。
pub fn fit_title(title: &str, budget: f32) -> String {
    if budget <= 0.0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0.0f32;
    let mut truncated = false;

    for ch in title.chars() {
        let width = char_width(ch);
        if used + width > budget {
            truncated = true;
            break;
        }
        used += width;
        out.push(ch);
    }

    if truncated {
        // 给省略号腾出 1 个全角宽度的位置。
        while used + 1.0 > budget {
            let Some(ch) = out.pop() else { break };
            used -= char_width(ch);
        }
        out.push('…');
    }
    out
}

/// 单字符宽度（单位：全角字符 = 1.0；ASCII ≈ 0.55）。
fn char_width(ch: char) -> f32 {
    if is_wide(ch) { 1.0 } else { 0.55 }
}

/// 是否宽字符（CJK / 全角标点 / 韩文 / 假名 / 中西文混排里按全角渲染的破折号）。
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F
        | 0x2013..=0x2014
        | 0x2026
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD
    )
}

/// 按字符数硬截断（不补省略号，用于日期这类短文本）。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// HTML 转义。标题来自接口，可能含 `<`、`&`、引号，必须转义。
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// `[r, g, b, a]` → CSS `rgba(...)`。
fn css_color(rgba: [u8; 4]) -> String {
    format!(
        "rgba({},{},{},{:.3})",
        rgba[0],
        rgba[1],
        rgba[2],
        rgba[3] as f32 / 255.0
    )
}
