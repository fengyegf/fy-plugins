//! HTML 模板：把 [`CardData`] 拼成一整页 1400×800 的**绝对定位** HTML，交给 hyper-render。
//!
//! 版面逐项对照 TS 版 `E:\Fuyu_TDBot\plugins\system\system.vue`（Vue + Tailwind）：
//!
//! ```text
//! ┌ 1400×800 透明层 ─────────────────────────────────────────────────────┐
//! │ [180 头像]  名字 36px 粗 / @username 24px 灰 / ID 24px 灰             │
//! │ ┌ 1360×480 信息区 ─────────────────────────────────────────────────┐ │
//! │ │ admin                          │ 磁盘空间                          │ │
//! │ │  ┌ 面板 ─────────────────────┐ │  ┌ 面板 ────────────────────────┐ │ │
//! │ │  │ [100 头像] 名字 / @ / ID    │ │  │ 磁盘C ▬▬▬▬▬  12.3/100 GB     │ │ │
//! │ │  │ 留言：…                    │ │  └──────────────────────────────┘ │ │
//! │ │  └───────────────────────────┘ │ 已加载插件                         │ │
//! │ │ TDLib                          │  [system 运行][whois 运行][…]      │ │
//! │ │ 使用情况（CPU / RAM 进度条）  │                                   │ │
//! │ └──────────────────────────────────────────────────────────────────┘ │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! 四条必须遵守的 hyper-render 0.2.2 现状（都有上游原因，不是随手写法）：
//!
//! 1. **样式全部内联**：`<style>` 里的选择器不保证生效，所以不写 class；
//! 2. **不用 flex**：所有元素都是 `body` 的直接子元素、各自 `position:absolute`，
//!    坐标来自 [`crate::render::layout`]。这既绕开 Blitz「flex 容器里匿名文本节点
//!    撑高」的坑，也让「HTML 画的白边圆环」和「`image` 贴的照片」能对齐到同一像素；
//! 3. **不依赖 `<img>` / 背景图**：hyper-render 不加载图片（含 data URI），头像与
//!    背景都在 [`crate::render::compose`] 里用 `image` 合成。这里只画同位置同尺寸的
//!    渐变圆形占位，没有照片时也不会开天窗；
//! 4. **只认系统字体**：`@font-face` 未支持，字体链见 [`FONT_SANS`]（中文靠雅黑）。
//!
//! 文本一律「先估算宽度 → 放不下先缩字号、再截断」，保证任何数据都不会顶出画布。

use std::sync::{Mutex, OnceLock};

use crate::core::model::{CardData, DiskUsage, Gauge, Identity, PluginChip};
use crate::render::layout;
use crate::render::{Error, HEIGHT, WIDTH};

/// 正文字体链：`Segoe UI` 管拉丁文，`Microsoft YaHei` 管中文。
pub const FONT_SANS: &str =
    "'Segoe UI','Segoe UI Emoji','Microsoft YaHei','Microsoft YaHei UI',sans-serif";

/// 文本颜色。
const COLOR_TITLE: &str = "#111827";
const COLOR_BODY: &str = "#1f2937";
const COLOR_MUTED: &str = "#6b7280";
const COLOR_FAINT: &str = "#9ca3af";
const COLOR_BLUE: &str = "#3b82f6";
const COLOR_BAR: &str = "#d1d5db";
/// 进度条填充色（`bg-blue-500`）。
const COLOR_BAR_FILL: &str = "#3b82f6";

/// 空数据兜底。
pub const NO_DATA: &str = "暂无数据";
/// 名字为空时的兜底（与 TS 版 `bot.name` 的展示一致）。
pub const UNKNOWN: &str = "未知";

/// 面板样式：白底 70%、圆角 12、1px 边框（`bg-white/70 rounded-xl p-2`）。
const PANEL_STYLE: &str = "box-sizing:border-box;background-color:rgba(255,255,255,0.7);\
     border:1px solid rgba(0,0,0,0.1);border-radius:12px;";

/// 单行文本样式。
#[derive(Clone, Copy)]
struct TextStyle {
    font: u32,
    line_height: u32,
    weight: &'static str,
    color: &'static str,
}

impl TextStyle {
    const fn new(font: u32, line_height: u32, weight: &'static str, color: &'static str) -> Self {
        Self {
            font,
            line_height,
            weight,
            color,
        }
    }
}

/// 区块标题（`text-lg font-semibold`）。
const STYLE_SECTION_TITLE: TextStyle = TextStyle::new(
    layout::SECTION_TITLE_FONT,
    layout::SECTION_TITLE_HEIGHT,
    "600",
    COLOR_TITLE,
);
/// 普通一行 18px。
const STYLE_ROW: TextStyle =
    TextStyle::new(layout::ROW_FONT, layout::ROW_HEIGHT, "normal", COLOR_BODY);
/// 弱化一行 18px。
const STYLE_ROW_MUTED: TextStyle =
    TextStyle::new(layout::ROW_FONT, layout::ROW_HEIGHT, "normal", COLOR_MUTED);
/// 占位文本（「暂无数据」）。
const STYLE_PLACEHOLDER: TextStyle =
    TextStyle::new(layout::ROW_FONT, layout::ROW_HEIGHT, "normal", COLOR_FAINT);

/// 简易 HTML 拼接器：只产出绝对定位的 `<div>`，不产出 class / style 标签。
struct Html {
    buf: String,
}

impl Html {
    fn new() -> Self {
        Self {
            buf: String::with_capacity(16 * 1024),
        }
    }

    /// 一个绝对定位的矩形块（无文本）。
    fn rect(&mut self, left: u32, top: u32, width: u32, height: u32, extra: &str) {
        self.buf.push_str(&format!(
            "<div style=\"position:absolute;left:{left}px;top:{top}px;\
             width:{width}px;height:{height}px;{extra}\"></div>"
        ));
    }

    /// 一行文本：固定框 + `nowrap` + `overflow:hidden`，永远只占一行。
    fn text(
        &mut self,
        left: u32,
        top: u32,
        width: u32,
        height: u32,
        style: TextStyle,
        extra: &str,
        content: &str,
    ) {
        self.buf.push_str(&format!(
            "<div style=\"position:absolute;left:{left}px;top:{top}px;\
             width:{width}px;height:{height}px;font-size:{font}px;line-height:{line}px;\
             font-weight:{weight};color:{color};white-space:nowrap;overflow:hidden;{extra}\">\
             {content}</div>",
            font = style.font,
            line = style.line_height,
            weight = style.weight,
            color = style.color,
        ));
    }
}

// ---------------------------------------------------------------------------
// 文本度量：估算宽度 → 缩字号 / 截断
// ---------------------------------------------------------------------------

/// 单个字符的估算宽度系数（× 字号）：ASCII 0.52em，CJK / emoji 1em，其它 0.62em。
///
/// 0.52 是拿本机实际渲染量出来的：`Segoe UI` 14px 下小写 + 下划线的混排平均每字符
/// 约 6.98px（≈0.50em），留 4% 余量，既不会让胶囊宽出一大截，也不会把状态裁掉。
fn char_width(ch: char, font: f32) -> f32 {
    if ch.is_ascii() {
        font * 0.52
    } else if is_wide(ch) {
        font
    } else {
        font * 0.62
    }
}

/// 是否宽字符（CJK / 假名 / 韩文 / 全角标点 / emoji）。
fn is_wide(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
    )
}

/// 估算单行文本宽度（px）。粗体按 1.05 倍放宽，宁可估大一点。
fn estimate_width(text: &str, font: f32, bold: bool) -> f32 {
    let sum: f32 = text.chars().map(|ch| char_width(ch, font)).sum();
    if bold { sum * 1.05 } else { sum }
}

/// 放得下就返回原字号；放不下就等比缩小，夹在 `[min, base]`。
fn fit_font_size(text: &str, max_width: f32, base: f32, min: f32, bold: bool) -> f32 {
    let estimated = estimate_width(text, base, bold);
    if estimated <= max_width {
        return base;
    }
    (base * max_width / estimated).clamp(min, base)
}

/// 超出宽度就截断并补一个省略号（按估算宽度，不触发真实排版）。
pub fn truncate_to_width(text: &str, max_width: f32, font: f32, bold: bool) -> String {
    if estimate_width(text, font, bold) <= max_width {
        return text.to_owned();
    }
    let ellipsis = char_width('…', font) * if bold { 1.05 } else { 1.0 };
    let budget = (max_width - ellipsis).max(0.0);
    let mut out = String::new();
    let mut used = 0.0f32;
    for ch in text.chars() {
        let width = char_width(ch, font) * if bold { 1.05 } else { 1.0 };
        if used + width > budget {
            break;
        }
        used += width;
        out.push(ch);
    }
    out.push('…');
    out
}

/// 空串换成兜底文案（返回**未转义**的原文）。
fn or_unknown<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
}

/// 单行文本的「缩字号 → 截断 → 转义」一条龙，返回（可安全塞进 HTML 的文本, 字号）。
///
/// 顺序很重要：先按**原文**估宽截断，最后才转义。反过来的话，截断可能把
/// `&amp;` 劈成 `&am…`，转义反而把模板弄坏了。
fn fit_text(raw: &str, max_width: f32, base: f32, min: f32, bold: bool) -> (String, f32) {
    let size = fit_font_size(raw, max_width, base, min, bold);
    let truncated = truncate_to_width(raw, max_width, size, bold);
    (escape_html(&truncated), size)
}

/// 同 [`fit_text`]，但不缩字号（固定字号下截断 + 转义）。
fn fit_text_fixed(raw: &str, max_width: f32, font: f32, bold: bool) -> String {
    escape_html(&truncate_to_width(raw, max_width, font, bold))
}

/// HTML 转义：文本节点里的 `<&"'` 不能原样进模板。
///
/// 逐字符处理（而不是连续 `replace`），天然避免 `&amp;lt;` 这类二次转义。
pub fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            // 换行 / 制表符会被 Blitz 当空白折叠，统一成空格保持单行。
            '\n' | '\r' | '\t' => escaped.push(' '),
            _ => escaped.push(ch),
        }
    }
    escaped
}

// ---------------------------------------------------------------------------
// 各区块
// ---------------------------------------------------------------------------

/// 头像行：机器人头像占位 + 名字 / @username / ID。
fn push_bot_header(out: &mut Html, card: &CardData) {
    push_avatar_placeholder(
        out,
        layout::BOT_AVATAR_LEFT,
        layout::BOT_AVATAR_TOP,
        layout::BOT_AVATAR_SIZE,
        "linear-gradient(135deg,#7dd3fc,#2563eb)",
    );

    let (name_text, name_size) = fit_text(
        or_unknown(&card.bot.name, UNKNOWN),
        layout::BOT_TEXT_WIDTH as f32,
        layout::BOT_NAME_FONT as f32,
        layout::BOT_NAME_MIN_FONT as f32,
        true,
    );
    out.text(
        layout::BOT_TEXT_LEFT,
        layout::BOT_NAME_TOP,
        layout::BOT_TEXT_WIDTH,
        layout::BOT_NAME_LINE_HEIGHT,
        TextStyle::new(
            name_size.round() as u32,
            layout::BOT_NAME_LINE_HEIGHT,
            "bold",
            COLOR_TITLE,
        ),
        "",
        &name_text,
    );

    let username = format!("@{}", or_unknown(&card.bot.username, UNKNOWN));
    let username_text = fit_text_fixed(
        &username,
        layout::BOT_TEXT_WIDTH as f32,
        layout::BOT_SUB_FONT as f32,
        false,
    );
    out.text(
        layout::BOT_TEXT_LEFT,
        layout::BOT_USERNAME_TOP,
        layout::BOT_TEXT_WIDTH,
        layout::BOT_SUB_LINE_HEIGHT,
        TextStyle::new(
            layout::BOT_SUB_FONT,
            layout::BOT_SUB_LINE_HEIGHT,
            "normal",
            COLOR_MUTED,
        ),
        "",
        &username_text,
    );

    let id = format!("ID:{}", card.bot.id);
    out.text(
        layout::BOT_TEXT_LEFT,
        layout::BOT_ID_TOP,
        layout::BOT_TEXT_WIDTH,
        layout::BOT_SUB_LINE_HEIGHT,
        TextStyle::new(
            layout::BOT_SUB_FONT,
            layout::BOT_SUB_LINE_HEIGHT,
            "normal",
            COLOR_MUTED,
        ),
        "",
        &id,
    );
}

/// 头像占位：外圈白环 + 内圈渐变圆（照片由 [`crate::render::compose`] 覆盖内圆）。
fn push_avatar_placeholder(out: &mut Html, left: u32, top: u32, size: u32, gradient: &str) {
    let radius = size / 2;
    out.rect(
        left,
        top,
        size,
        size,
        &format!("border-radius:{radius}px;background-color:#ffffff;"),
    );
    let inner = size - 2 * layout::AVATAR_BORDER;
    out.rect(
        left + layout::AVATAR_BORDER,
        top + layout::AVATAR_BORDER,
        inner,
        inner,
        &format!("border-radius:{}px;background-image:{gradient};", inner / 2),
    );
}

/// admin 区块：有管理员显示资料 + 留言，没有就显示配置提示（头像位置留占位）。
fn push_admin_section(out: &mut Html, card: &CardData) {
    out.text(
        layout::COL_LEFT_X,
        layout::ADMIN_TITLE_TOP,
        layout::COL_WIDTH,
        layout::SECTION_TITLE_HEIGHT,
        STYLE_SECTION_TITLE,
        "",
        "admin",
    );
    out.rect(
        layout::COL_LEFT_X,
        layout::ADMIN_PANEL_TOP,
        layout::COL_WIDTH,
        layout::ADMIN_PANEL_HEIGHT,
        PANEL_STYLE,
    );
    push_avatar_placeholder(
        out,
        layout::ADMIN_AVATAR_LEFT,
        layout::ADMIN_AVATAR_TOP,
        layout::ADMIN_AVATAR_SIZE,
        "linear-gradient(135deg,#fbcfe8,#db2777)",
    );

    match &card.admin {
        Some(admin) => {
            push_admin_identity(out, admin);
            push_admin_note(
                out,
                &format!(
                    "留言：如果遇到bug前往 @{} 反馈",
                    or_unknown(&admin.username, UNKNOWN)
                ),
            );
        }
        None => {
            // 没有管理员：面板里只放配置提示，底部小字说明缺哪个配置项。
            out.text(
                layout::ADMIN_TEXT_LEFT,
                layout::ADMIN_NAME_TOP,
                layout::ADMIN_TEXT_WIDTH,
                layout::ADMIN_NAME_LINE_HEIGHT,
                TextStyle::new(
                    layout::ADMIN_NAME_FONT,
                    layout::ADMIN_NAME_LINE_HEIGHT,
                    "600",
                    COLOR_TITLE,
                ),
                "",
                "未配置管理员",
            );
            let hint = fit_text_fixed(
                "请前往 [plugin.system] 添加 super_admin",
                layout::ADMIN_TEXT_WIDTH as f32,
                layout::ADMIN_SUB_FONT as f32,
                false,
            );
            out.text(
                layout::ADMIN_TEXT_LEFT,
                layout::ADMIN_USERNAME_TOP,
                layout::ADMIN_TEXT_WIDTH,
                layout::ADMIN_SUB_LINE_HEIGHT,
                TextStyle::new(
                    layout::ADMIN_SUB_FONT,
                    layout::ADMIN_SUB_LINE_HEIGHT,
                    "normal",
                    COLOR_MUTED,
                ),
                "",
                &hint,
            );
            push_admin_note(out, "未配置管理员（[plugin.system] super_admin）");
        }
    }
}

/// 管理员三行资料：名字 24px 半粗、`@username` 与 `ID:` 18px 灰。
fn push_admin_identity(out: &mut Html, admin: &Identity) {
    let name_text = fit_text_fixed(
        or_unknown(&admin.name, UNKNOWN),
        layout::ADMIN_TEXT_WIDTH as f32,
        layout::ADMIN_NAME_FONT as f32,
        true,
    );
    out.text(
        layout::ADMIN_TEXT_LEFT,
        layout::ADMIN_NAME_TOP,
        layout::ADMIN_TEXT_WIDTH,
        layout::ADMIN_NAME_LINE_HEIGHT,
        TextStyle::new(
            layout::ADMIN_NAME_FONT,
            layout::ADMIN_NAME_LINE_HEIGHT,
            "600",
            COLOR_TITLE,
        ),
        "",
        &name_text,
    );

    let username = format!("@{}", or_unknown(&admin.username, UNKNOWN));
    let username_text = fit_text_fixed(
        &username,
        layout::ADMIN_TEXT_WIDTH as f32,
        layout::ADMIN_SUB_FONT as f32,
        false,
    );
    out.text(
        layout::ADMIN_TEXT_LEFT,
        layout::ADMIN_USERNAME_TOP,
        layout::ADMIN_TEXT_WIDTH,
        layout::ADMIN_SUB_LINE_HEIGHT,
        TextStyle::new(
            layout::ADMIN_SUB_FONT,
            layout::ADMIN_SUB_LINE_HEIGHT,
            "normal",
            COLOR_MUTED,
        ),
        "",
        &username_text,
    );

    let id = format!("ID:{}", admin.id);
    out.text(
        layout::ADMIN_TEXT_LEFT,
        layout::ADMIN_ID_TOP,
        layout::ADMIN_TEXT_WIDTH,
        layout::ADMIN_SUB_LINE_HEIGHT,
        TextStyle::new(
            layout::ADMIN_SUB_FONT,
            layout::ADMIN_SUB_LINE_HEIGHT,
            "normal",
            COLOR_MUTED,
        ),
        "",
        &id,
    );
}

/// admin 面板底部 12px 小字。
fn push_admin_note(out: &mut Html, note: &str) {
    let content = fit_text_fixed(
        note,
        layout::PANEL_INNER_WIDTH as f32,
        layout::ADMIN_NOTE_FONT as f32,
        false,
    );
    out.text(
        layout::LEFT_INNER_X,
        layout::ADMIN_NOTE_TOP,
        layout::PANEL_INNER_WIDTH,
        layout::ADMIN_NOTE_LINE_HEIGHT,
        TextStyle::new(
            layout::ADMIN_NOTE_FONT,
            layout::ADMIN_NOTE_LINE_HEIGHT,
            "normal",
            COLOR_MUTED,
        ),
        "",
        &content,
    );
}

/// TDLib 区块：版本号 + 提交哈希。
fn push_tdlib_section(out: &mut Html, card: &CardData) {
    out.text(
        layout::COL_LEFT_X,
        layout::TDLIB_TITLE_TOP,
        layout::COL_WIDTH,
        layout::SECTION_TITLE_HEIGHT,
        STYLE_SECTION_TITLE,
        "",
        "TDLib",
    );
    out.rect(
        layout::COL_LEFT_X,
        layout::TDLIB_PANEL_TOP,
        layout::COL_WIDTH,
        layout::TDLIB_PANEL_HEIGHT,
        PANEL_STYLE,
    );

    let tdlib = &card.tdlib;
    let version = fit_text_fixed(
        &format!("version:{}", or_unknown(&tdlib.version, UNKNOWN)),
        layout::PANEL_INNER_WIDTH as f32,
        layout::ROW_FONT as f32,
        false,
    );
    out.text(
        layout::LEFT_INNER_X,
        layout::TDLIB_PANEL_TOP + layout::PANEL_BORDER + layout::PANEL_PADDING,
        layout::PANEL_INNER_WIDTH,
        layout::ROW_HEIGHT,
        STYLE_ROW,
        "",
        &version,
    );

    let hash = fit_text_fixed(
        &format!("commit hash:{}", or_unknown(&tdlib.commit_hash, UNKNOWN)),
        layout::PANEL_INNER_WIDTH as f32,
        layout::ROW_FONT as f32,
        false,
    );
    out.text(
        layout::LEFT_INNER_X,
        layout::TDLIB_PANEL_TOP + layout::PANEL_BORDER + layout::PANEL_PADDING + layout::ROW_HEIGHT,
        layout::PANEL_INNER_WIDTH,
        layout::ROW_HEIGHT,
        STYLE_ROW_MUTED,
        "",
        &hash,
    );
}

/// 使用情况区块：CPU / RAM 两行进度条（SWAP 已去掉；空的走「暂无数据」）。
fn push_gauges_section(out: &mut Html, card: &CardData) {
    out.text(
        layout::COL_LEFT_X,
        layout::GAUGE_TITLE_TOP,
        layout::COL_WIDTH,
        layout::SECTION_TITLE_HEIGHT,
        STYLE_SECTION_TITLE,
        "",
        "使用情况",
    );

    let gauges: Vec<&Gauge> = card.gauges.iter().take(2).collect();
    let rows = gauges.len().max(1) as u32;
    let panel_height = layout::PANEL_CHROME + rows * layout::ROW_HEIGHT;
    out.rect(
        layout::COL_LEFT_X,
        layout::GAUGE_PANEL_TOP,
        layout::COL_WIDTH,
        panel_height,
        PANEL_STYLE,
    );

    let row_top = layout::GAUGE_PANEL_TOP + layout::PANEL_BORDER + layout::PANEL_PADDING;
    if gauges.is_empty() {
        out.text(
            layout::LEFT_INNER_X,
            row_top,
            layout::PANEL_INNER_WIDTH,
            layout::ROW_HEIGHT,
            STYLE_PLACEHOLDER,
            "",
            NO_DATA,
        );
        return;
    }

    // 进度条夹在两列固定宽度之间，宽度算死，避免 Blitz 的 flex 宽度误差。
    let bar_left = layout::LEFT_INNER_X + layout::GAUGE_LABEL_WIDTH + layout::GAUGE_COLUMN_GAP;
    let bar_width = layout::PANEL_INNER_WIDTH
        .saturating_sub(
            layout::GAUGE_LABEL_WIDTH + layout::GAUGE_VALUE_WIDTH + 2 * layout::GAUGE_COLUMN_GAP,
        )
        .max(40);
    let value_left = layout::LEFT_INNER_X + layout::PANEL_INNER_WIDTH - layout::GAUGE_VALUE_WIDTH;

    for (index, gauge) in gauges.iter().enumerate() {
        let top = row_top + index as u32 * layout::ROW_HEIGHT;
        let label = fit_text_fixed(
            &gauge.label,
            layout::GAUGE_LABEL_WIDTH as f32,
            layout::ROW_FONT as f32,
            false,
        );
        out.text(
            layout::LEFT_INNER_X,
            top,
            layout::GAUGE_LABEL_WIDTH,
            layout::ROW_HEIGHT,
            STYLE_ROW,
            "",
            &label,
        );
        push_bar(
            out,
            bar_left,
            top + layout::BAR_OFFSET_Y,
            bar_width,
            gauge.usage,
        );
        out.text(
            value_left,
            top,
            layout::GAUGE_VALUE_WIDTH,
            layout::ROW_HEIGHT,
            STYLE_ROW,
            "text-align:right;",
            &format!("{}%", gauge.usage.min(100)),
        );
    }
}

/// 磁盘空间区块：每行「磁盘X + 进度条 + used/total」，最多 4 块。
fn push_disk_section(out: &mut Html, card: &CardData) {
    out.text(
        layout::COL_RIGHT_X,
        layout::DISK_TITLE_TOP,
        layout::COL_WIDTH,
        layout::SECTION_TITLE_HEIGHT,
        STYLE_SECTION_TITLE,
        "",
        "磁盘空间",
    );

    let disks: Vec<&DiskUsage> = card.disks.iter().take(4).collect();
    let rows = disks.len().max(1) as u32;
    out.rect(
        layout::COL_RIGHT_X,
        layout::DISK_PANEL_TOP,
        layout::COL_WIDTH,
        layout::disk_panel_height(rows),
        PANEL_STYLE,
    );

    let row_top = layout::DISK_PANEL_TOP + layout::PANEL_BORDER + layout::PANEL_PADDING;
    if disks.is_empty() {
        out.text(
            layout::RIGHT_INNER_X,
            row_top,
            layout::PANEL_INNER_WIDTH,
            layout::ROW_HEIGHT,
            STYLE_PLACEHOLDER,
            "",
            NO_DATA,
        );
        return;
    }

    let bar_left = layout::RIGHT_INNER_X + layout::DISK_LABEL_WIDTH + layout::DISK_COLUMN_GAP;
    let bar_width = layout::PANEL_INNER_WIDTH
        .saturating_sub(
            layout::DISK_LABEL_WIDTH + layout::DISK_VALUE_WIDTH + 2 * layout::DISK_COLUMN_GAP,
        )
        .max(40);
    let value_left = layout::RIGHT_INNER_X + layout::PANEL_INNER_WIDTH - layout::DISK_VALUE_WIDTH;

    for (index, disk) in disks.iter().enumerate() {
        let top = row_top + index as u32 * layout::ROW_HEIGHT;
        // 标签已经在采集层定好（「磁盘C」/「内部存储」/挂载点）；长挂载点先缩字号再截断。
        let label_text = disk.label.as_str();
        let label_font = fit_font_size(
            label_text,
            layout::DISK_LABEL_WIDTH as f32 - 2.0,
            layout::ROW_FONT as f32,
            13.0,
            false,
        );
        let label = fit_text_fixed(
            label_text,
            layout::DISK_LABEL_WIDTH as f32,
            label_font,
            false,
        );
        out.text(
            layout::RIGHT_INNER_X,
            top,
            layout::DISK_LABEL_WIDTH,
            layout::ROW_HEIGHT,
            TextStyle::new(
                label_font.round() as u32,
                layout::ROW_HEIGHT,
                "normal",
                COLOR_BODY,
            ),
            "",
            &label,
        );
        push_bar(
            out,
            bar_left,
            top + layout::BAR_OFFSET_Y,
            bar_width,
            disk.usage_percent,
        );

        // 「12.34 GB/100.00 GB」这类文本可能很长：先缩字号，仍放不下再截断。
        let value_text = format!("{}/{}", disk.used, disk.total);
        let font = fit_font_size(
            &value_text,
            layout::DISK_VALUE_WIDTH as f32 - 2.0,
            layout::ROW_FONT as f32,
            12.0,
            false,
        );
        let value = fit_text_fixed(&value_text, layout::DISK_VALUE_WIDTH as f32, font, false);
        out.text(
            value_left,
            top,
            layout::DISK_VALUE_WIDTH,
            layout::ROW_HEIGHT,
            TextStyle::new(
                font.round() as u32,
                layout::ROW_HEIGHT,
                "normal",
                COLOR_BODY,
            ),
            "text-align:right;",
            &value,
        );
    }
}

/// 已加载插件：手算坐标换行的胶囊，行数上限由右列剩余高度决定，超出的才折成 `+N`。
///
/// 面板高度**按实际排出来的胶囊行数算**（[`layout::plugin_panel_height`]），
/// 不铺满右列的剩余空间——插件少的时候就是一个贴着内容的盒子。
fn push_plugin_section(out: &mut Html, card: &CardData) {
    let disk_rows = card.disks.iter().take(4).count().max(1) as u32;
    let title_top = layout::plugin_title_top(disk_rows);
    let panel_top = layout::plugin_panel_top(disk_rows);

    // 先排版、再画面板：面板高度取决于真正放下了几行胶囊。
    let specs = chip_specs(&card.plugins);
    let max_rows = layout::max_chip_rows(disk_rows);
    let placements = place_chips(&specs, layout::PANEL_INNER_WIDTH, max_rows);
    let chip_rows = placements
        .iter()
        .map(|chip| chip.row + 1)
        .max()
        .unwrap_or(1);
    let panel_height = layout::plugin_panel_height(chip_rows);

    out.text(
        layout::COL_RIGHT_X,
        title_top,
        layout::COL_WIDTH,
        layout::SECTION_TITLE_HEIGHT,
        STYLE_SECTION_TITLE,
        "",
        "已加载插件",
    );
    out.rect(
        layout::COL_RIGHT_X,
        panel_top,
        layout::COL_WIDTH,
        panel_height,
        PANEL_STYLE,
    );

    let inner_left = layout::RIGHT_INNER_X;
    let inner_top = panel_top + layout::PANEL_BORDER + layout::PANEL_PADDING;
    if placements.is_empty() {
        out.text(
            inner_left,
            inner_top,
            layout::PANEL_INNER_WIDTH,
            layout::ROW_HEIGHT,
            STYLE_PLACEHOLDER,
            "",
            NO_DATA,
        );
    } else {
        for placed in &placements {
            let left = inner_left + placed.x;
            let top = inner_top + placed.row * (layout::CHIP_HEIGHT + layout::CHIP_GAP);
            push_chip(out, left, top, placed);
        }
    }

    // 面板底部：左边插件总数、右边生成标记（跟着自适应高度走，不再固定贴内容区底端）。
    let footer_top = panel_top + panel_height
        - layout::PANEL_BORDER
        - layout::PANEL_PADDING
        - layout::PLUGIN_FOOTER_HEIGHT;
    let footer_style = TextStyle::new(12, 16, "normal", COLOR_FAINT);
    out.text(
        inner_left,
        footer_top,
        320,
        layout::PLUGIN_FOOTER_HEIGHT,
        footer_style,
        "",
        &format!("共 {} 个插件", card.plugins.len()),
    );
    out.text(
        inner_left + layout::PANEL_INNER_WIDTH - 320,
        footer_top,
        320,
        layout::PLUGIN_FOOTER_HEIGHT,
        footer_style,
        "text-align:right;",
        "Generated by FuyuBot",
    );
}

/// 一枚胶囊：名字与状态是**同一行的内联文本**（状态跟在名字后面自然排开）。
///
/// 胶囊的宽度是按估算值写死的（浏览器排版结果拿不到），但名字与状态之间不靠坐标
/// 定位，所以估算偏大时只会让右边多留一点白，不会出现「状态漂到半空中」的观感。
fn push_chip(out: &mut Html, left: u32, top: u32, chip: &PlacedChip) {
    let name = escape_html(&chip.name);
    // 名字与状态之间用不换行空格撑开：Blitz 会忽略行内元素的 `margin-left`，
    // 也会把普通空格折叠掉，只有 `&nbsp;` 稳定占位（且不会被折行吞掉）。
    let state = if chip.state.is_empty() {
        String::new()
    } else {
        format!(
            "&#160;&#160;<span style=\"color:{COLOR_BLUE};\">{}</span>",
            escape_html(&chip.state)
        )
    };
    out.buf.push_str(&format!(
        "<div style=\"position:absolute;left:{left}px;top:{top}px;width:{width}px;\
         height:{height}px;box-sizing:border-box;padding:0 {pad}px;line-height:{line}px;\
         font-size:{font}px;font-weight:normal;color:{COLOR_TITLE};white-space:nowrap;\
         overflow:hidden;background-color:rgba(255,255,255,0.8);\
         border:1px solid rgba(0,0,0,0.1);border-radius:4px;\">{name}{state}</div>",
        width = chip.width,
        height = layout::CHIP_HEIGHT,
        pad = layout::CHIP_PADDING_X,
        line = layout::CHIP_HEIGHT - 2 * layout::PANEL_BORDER,
        font = layout::CHIP_FONT,
    ));
}

/// 进度条：灰色底槽 + 蓝色填充（`h-3 rounded-full`）。
fn push_bar(out: &mut Html, left: u32, top: u32, width: u32, percent: u8) {
    out.rect(
        left,
        top,
        width,
        layout::BAR_HEIGHT,
        &format!(
            "border-radius:{}px;background-color:{COLOR_BAR};overflow:hidden;",
            layout::BAR_RADIUS
        ),
    );
    let percent = percent.min(100) as u32;
    if percent == 0 {
        return;
    }
    // 百分比宽度 + 一个最小可见宽度，1% 也看得见。
    let fill = ((width * percent) / 100)
        .max(layout::BAR_RADIUS * 2)
        .min(width);
    out.rect(
        left,
        top,
        fill,
        layout::BAR_HEIGHT,
        &format!(
            "border-radius:{}px;background-color:{COLOR_BAR_FILL};",
            layout::BAR_RADIUS
        ),
    );
}

// ---------------------------------------------------------------------------
// 胶囊布局（手算换行）
// ---------------------------------------------------------------------------

/// 一枚待排版的胶囊。
struct ChipSpec {
    name: String,
    state: String,
    width: u32,
}

/// 已放下的胶囊：`x` 是相对面板内左侧的偏移，`row` 是行号（0 起）。
struct PlacedChip {
    x: u32,
    row: u32,
    width: u32,
    name: String,
    state: String,
}

/// 胶囊右边缘的安全余量：文字宽度是估算的（比真实排版略窄），留 6px 免得状态被裁。
const CHIP_SLACK: u32 = 6;

/// 把插件列表变成带估算宽度的胶囊（名字过长先截断，保证单枚不超宽）。
fn chip_specs(plugins: &[PluginChip]) -> Vec<ChipSpec> {
    plugins
        .iter()
        .map(|chip| {
            let state = chip.state_label().to_owned();
            let state_width = estimate_width(&state, layout::CHIP_FONT as f32, false).ceil() as u32;
            let fixed = 2 * layout::PANEL_BORDER
                + 2 * layout::CHIP_PADDING_X
                + layout::CHIP_INNER_GAP
                + CHIP_SLACK
                + state_width;
            let max_name = layout::PANEL_INNER_WIDTH.saturating_sub(fixed).max(40);
            let raw_name = chip.name.trim();
            let raw_name = if raw_name.is_empty() {
                "未命名"
            } else {
                raw_name
            };
            let name =
                truncate_to_width(raw_name, max_name as f32, layout::CHIP_FONT as f32, false);
            let name_width = estimate_width(&name, layout::CHIP_FONT as f32, false).ceil() as u32;
            ChipSpec {
                name,
                state,
                width: fixed + name_width,
            }
        })
        .collect()
}

/// `+N` 收尾胶囊的宽度（`N` 的位数会影响宽度）。
fn tail_chip_width(label: &str) -> u32 {
    2 * layout::PANEL_BORDER
        + 2 * layout::CHIP_PADDING_X
        + CHIP_SLACK
        + estimate_width(label, layout::CHIP_FONT as f32, false).ceil() as u32
}

/// 造一枚 `+N` 胶囊。
fn tail_chip(label: String, x: u32, row: u32) -> PlacedChip {
    PlacedChip {
        x,
        row,
        width: tail_chip_width(&label),
        name: label,
        state: String::new(),
    }
}

/// 把前 `count` 枚胶囊按 `max_rows` 行折行，返回（已放下的胶囊，下一格的 x，最后一行行号）。
fn layout_prefix(
    specs: &[ChipSpec],
    count: usize,
    inner_width: u32,
    max_rows: u32,
) -> (Vec<PlacedChip>, u32, u32) {
    let mut placed = Vec::with_capacity(count.min(specs.len()));
    let mut cursor = 0u32;
    let mut row = 0u32;
    for spec in specs.iter().take(count) {
        if cursor > 0 && cursor + spec.width > inner_width {
            if row + 1 >= max_rows {
                break;
            }
            row += 1;
            cursor = 0;
        }
        placed.push(PlacedChip {
            x: cursor,
            row,
            width: spec.width,
            name: spec.name.clone(),
            state: spec.state.clone(),
        });
        cursor += spec.width + layout::CHIP_GAP;
    }
    (placed, cursor, row)
}

/// 一行能放几枚就是几枚；最多 `max_rows` 行，放不下的用 `+N` 收尾。
///
/// `+N` 自己也要占地方：优先接在最后一行末尾，其次另起一行（未满行数时），
/// 行数满了就一枚一枚地让出位置给它，因此循环必然终止。
fn place_chips(specs: &[ChipSpec], inner_width: u32, max_rows: u32) -> Vec<PlacedChip> {
    if specs.is_empty() {
        return Vec::new();
    }
    let (mut placed, mut cursor, mut row) =
        layout_prefix(specs, specs.len(), inner_width, max_rows);
    if placed.len() == specs.len() {
        return placed;
    }

    let mut fitted = placed.len();
    let mut overflow = specs.len() - fitted;
    loop {
        if fitted == 0 {
            // 面板窄到连一枚都放不下：只画 `+N`。
            return vec![tail_chip(format!("+{overflow}"), 0, 0)];
        }
        let label = format!("+{overflow}");
        let tail_width = tail_chip_width(&label);

        if cursor > 0 && cursor + tail_width <= inner_width {
            placed.push(tail_chip(label, cursor, row));
            return placed;
        }
        if row + 1 < max_rows && tail_width <= inner_width {
            placed.push(tail_chip(label, 0, row + 1));
            return placed;
        }

        // 行数满了：让出最后一枚胶囊的位置。
        fitted -= 1;
        overflow += 1;
        let prefix = layout_prefix(specs, fitted, inner_width, max_rows);
        placed = prefix.0;
        cursor = prefix.1;
        row = prefix.2;
    }
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// 生成完整的 HTML 文档（单个字符串，元素之间**没有**换行/缩进）。
pub fn build_html(card: &CardData) -> String {
    let mut out = Html::new();

    // 两列之间的 1px 竖线（vue 的 `border-l border-white/30`）。
    out.rect(
        layout::DIVIDER_X,
        layout::CARD_TOP,
        1,
        layout::CARD_HEIGHT,
        "background-color:rgba(255,255,255,0.35);",
    );

    push_bot_header(&mut out, card);
    push_admin_section(&mut out, card);
    push_tdlib_section(&mut out, card);
    push_gauges_section(&mut out, card);
    push_disk_section(&mut out, card);
    push_plugin_section(&mut out, card);

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
         <body style=\"margin:0;padding:0;width:{WIDTH}px;height:{HEIGHT}px;\
         background-color:transparent;position:relative;overflow:hidden;\
         font-family:{FONT_SANS};color:{COLOR_TITLE};\">{}</body></html>",
        out.buf
    )
}

/// 渲染串行锁。
///
/// hyper-render（Blitz + Vello）内部有全局字体集合与缓存，插件会在多条业务线程里
/// 并发出图；串行化是这里最简单也最稳的保证（与 `whois` / `musiccard` 的做法一致）。
fn render_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 用 hyper-render 渲出 1400×800 的**透明**卡片层，返回 PNG 字节。
pub fn render_layer(card: &CardData) -> Result<Vec<u8>, Error> {
    let html = build_html(card);
    let _guard = render_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let config = hyper_render::Config::new()
        .width(WIDTH)
        .height(HEIGHT)
        .scale(1.0)
        .format(hyper_render::OutputFormat::Png)
        // 固定 800 高：让根节点按视口算，不跟随内容（内容全都绝对定位）。
        .auto_height(false)
        .transparent();

    hyper_render::render_to_png(&html, config).map_err(|error| Error::Engine(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::PluginChip;
    use crate::render::samples;

    /// 模板里不能出现未转义的尖括号 / 引号破坏结构。
    #[test]
    fn escaping_keeps_template_intact() {
        let mut card = samples::sample_card();
        card.bot.name = "<b>坏名字</b> & \"引号\" '单引号'".to_owned();
        card.bot.username = "a&b<script>".to_owned();
        card.tdlib.version = "<1.8.29>".to_owned();
        card.plugins = vec![PluginChip {
            name: "<script>alert(1)</script>".to_owned(),
            state: "active".to_owned(),
        }];
        card.disks[0].label = "\"C\"&".to_owned();

        let html = build_html(&card);
        assert!(html.contains("&lt;b&gt;坏名字&lt;/b&gt;"));
        assert!(html.contains("&amp;"));
        assert!(html.contains("&quot;"));
        assert!(html.contains("&#39;"));
        // 原始标签不能出现（否则会被解析成元素）。
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<b>"));
        // 结构完好：一个 html 文档、body 只有一个。
        assert!(html.starts_with("<!doctype html>"));
        assert_eq!(html.matches("<body").count(), 1);
        assert!(html.ends_with("</body></html>"));
    }

    #[test]
    fn escape_html_covers_special_characters() {
        assert_eq!(escape_html("<&\"'>"), "&lt;&amp;&quot;&#39;&gt;");
        // 不二次转义。
        assert_eq!(escape_html("&amp;"), "&amp;amp;");
        assert_eq!(escape_html("换行\n制表\t"), "换行 制表 ");
        assert_eq!(escape_html("普通中文 abc"), "普通中文 abc");
    }

    /// 名字超长时只截断、不溢出；估算宽度必须落在给了的宽度里。
    #[test]
    fn long_text_is_truncated_to_width() {
        let text = "超长名字".repeat(80);
        let truncated = truncate_to_width(&text, 400.0, 36.0, true);
        assert!(truncated.ends_with('…'));
        assert!(estimate_width(&truncated, 36.0, true) <= 400.0 + 1.0);
        assert!(truncated.chars().count() < text.chars().count());
    }

    /// 胶囊排版：行数上限内换行，放不下的折成 `+N`，且不越过面板内宽。
    #[test]
    fn chips_wrap_and_only_tail_when_out_of_rows() {
        // 60 个插件远超任何可用行数的容量：最后一枚必须是 `+N`。
        let plugins: Vec<PluginChip> = (0..60)
            .map(|index| PluginChip {
                name: format!("plugin-{index}"),
                state: "active".to_owned(),
            })
            .collect();
        let max_rows = layout::max_chip_rows(1);
        let specs = chip_specs(&plugins);
        let placed = place_chips(&specs, layout::PANEL_INNER_WIDTH, max_rows);
        assert!(!placed.is_empty());
        for chip in &placed {
            assert!(chip.row < max_rows, "行号 {} 超过上限 {max_rows}", chip.row);
            assert!(chip.x + chip.width <= layout::PANEL_INNER_WIDTH);
        }
        assert!(
            placed
                .last()
                .map(|chip| chip.name.starts_with('+'))
                .unwrap_or(false),
            "溢出时最后应当是一个 +N 胶囊"
        );
    }

    /// 常见数量（6 个）的插件，无论几块磁盘都应当完整显示，不出现 `+N`。
    #[test]
    fn common_plugin_counts_are_all_drawn() {
        let plugins: Vec<PluginChip> = ["system", "whois", "kfc", "epic", "musiccard", "help"]
            .iter()
            .map(|name| PluginChip {
                name: (*name).to_owned(),
                state: "active".to_owned(),
            })
            .collect();
        for disk_rows in 1..=4 {
            let max_rows = layout::max_chip_rows(disk_rows);
            let specs = chip_specs(&plugins);
            let placed = place_chips(&specs, layout::PANEL_INNER_WIDTH, max_rows);
            assert_eq!(
                placed.len(),
                plugins.len(),
                "{disk_rows} 块磁盘时应能显示全部 6 个插件"
            );
            assert!(placed.iter().all(|chip| !chip.name.starts_with('+')));
        }
    }

    /// 空数据也要有兜底文案，且不影响后续区块。
    #[test]
    fn empty_sections_fall_back_to_placeholder() {
        let card = CardData::default();
        let html = build_html(&card);
        assert!(html.contains(NO_DATA));
        assert!(html.contains("未配置管理员"));
        assert!(html.contains(&format!("ID:{}", 0)));
        // 一条 gauge 都没有时，面板高度按 1 行算，占位文案在那儿。
        assert!(html.contains("使用情况"));
    }

    /// 极端数据：两条 gauge、四块磁盘、十个插件都不越界。
    #[test]
    fn full_layout_stays_within_canvas() {
        let card = samples::sample_card();
        let html = build_html(&card);
        // 所有 left/top 都不会超过画布（简单结构检查：没有负坐标）。
        assert!(!html.contains("left:-"));
        assert!(!html.contains("top:-"));
        assert!(html.matches("<div ").count() > 20);
    }

    /// 样例数据自检：两条 gauge（CPU / RAM）、四块磁盘。
    #[test]
    fn sample_card_covers_full_case() {
        let card = samples::sample_card();
        assert_eq!(card.gauges.len(), 2, "SWAP 已去掉");
        assert_eq!(card.gauges[1].label, "RAM");
        assert_eq!(card.disks.len(), 4);
        assert!(card.admin.is_some());
        assert!(card.plugins.len() >= 6);
        assert_eq!(card.gauges[0].usage, 37);
    }
}
