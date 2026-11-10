//! 卡片模板：把一段文案变成 HTML。
//!
//! 设计照搬 TS 版 `kfc.vue`：白底圆角卡片，左侧圆形头像，右侧 KFC + 蓝标 + 句柄，
//! 正文 30px，底部一排互动图标（回复 / 转发 / 喜欢 / 书签 / 分享）。
//!
//! **注意**：hyper-render 只认内联样式里的属性，`<style>` 标签里的选择器不保证生效，
//! 所以样式全部写在 `style="…"` 上（和 `kfc.vue` 一样），别改成 class + `<style>`。
//!
//! 和 `kfc.vue` 的一处“必要差异”：Vue 版靠 `flex-grow:1` 让右侧内容列自适应，
//! 但 Blitz 对「flex 子项 + margin-right + 百分比子元素」的组合算出来的宽度偏大，
//! 长正文会顶出卡片右边（实测不折行、直接溢出）。所以这里把内容列与正文的宽度
//! 都按卡片宽度**显式算成像素**，布局才是确定的。

use crate::render::assets;

/// 卡片边框宽度（`kfc.vue` 的 `1px solid #e1e8ed`）。
const BORDER: u32 = 1;
/// 卡片内边距。
const PADDING: u32 = 20;
/// 头像直径。
const AVATAR: u32 = 100;
/// 头像与正文列的间距（`kfc.vue` 的 `margin-right:15px`）。
const AVATAR_GAP: u32 = 15;
/// 正文列与卡片右内边距之间的留白（`kfc.vue` 的 `margin-right:30px`）。
const RIGHT_GAP: u32 = 30;

/// 正文列最小宽度：再窄的卡片也不至于把正文压成一条竖线。
const MIN_COLUMN: u32 = 120;

/// 用户名那一行的高度。satori 用 Noto Sans SC 的 `line-height: normal` 排出来是
/// 30px × 1.433 ≈ 43px；这里写死，避免不同系统字体的行高差异把版式带跑。
const USER_LINE_HEIGHT: u32 = 43;
/// 昵称（KFC）字号。
const NAME_FONT_SIZE: u32 = 30;
/// 句柄（@KFC · 刚刚）字号。
const HANDLE_FONT_SIZE: u32 = 28;
/// 正文字号。
const BODY_FONT_SIZE: u32 = 30;
/// 正文行高（px）。satori 侧是 Noto Sans SC 的 `normal`（30 × 1.433 = 43px）；
/// 这里写死像素值，免得系统字体行高差异把整张卡片带跑。
const BODY_LINE_HEIGHT: u32 = 43;

/// 字体链。TS 版用 Noto Sans SC，Windows 上装不到；`Segoe UI` 负责拉丁文，
/// 缺字时回退到 `Microsoft YaHei` 出中文，观感最接近 satori 的无衬线效果。
/// 不写 font-family 的话 Blitz 默认落到衬线体，英文正文会明显偏「印刷体」。
///
/// `Segoe UI Symbol` 不能省：线上文案偶尔会带 `◁ ▷ ↻ ❚ ♬` 这类符号
/// （接口返回过带媒体控制符的段子），Segoe UI / 雅黑都没有这些字形，缺了就是豆腐块。
///
/// 顺序必须是 **Emoji 在 Symbol 之前**：两只字体都覆盖一部分 emoji 码位，
/// Symbol 里的那部分是单色字形，排前面会把 🍗🍔 这类抢成黑白线稿（实测）。
const FONT_FAMILY: &str = "'Segoe UI','Segoe UI Emoji','Segoe UI Symbol','Microsoft YaHei','Microsoft YaHei UI',sans-serif";

/// 图标行与正文的间距（`kfc.vue` 的 `margin-top:20px`）。
const ACTIONS_MARGIN_TOP: u32 = 20;
/// 图标尺寸上限。
const ICON_SIZE: u32 = 32;
/// 图标之间的最小留白：卡片窄时不能把 5 个图标挤成一条。
const ICON_GAP: u32 = 8;
/// 用户名行还能排成一行（KFC + 蓝标 + 句柄）的最小内容列宽度；
/// 比这窄就允许换行，避免句柄溢出卡片。
const USER_ROW_MIN_WIDTH: u32 = 270;

/// 生成整页 HTML。`width` 是卡片宽度（px）。
pub fn build(content: &str, width: u32) -> String {
    let body = escape(content);
    let column = column_width(width);
    // `kfc.vue` 里 `.tweet-text` 是 `width:95%`，保留这个比例。
    let text = text_width(width);
    let body_line_height = format!("{}px", BODY_LINE_HEIGHT);
    // 宽卡片里这一行放得下「KFC + 蓝标 + @KFC · 刚刚」，高度写死 43px 最稳；
    // 窄到放不下时允许换行，宁可多占一行，也别让句柄顶出卡片边框。
    let user_row_style = if column >= USER_ROW_MIN_WIDTH {
        format!("display:flex;flex-direction:row;align-items:center;height:{USER_LINE_HEIGHT}px;")
    } else {
        format!(
            "display:flex;flex-direction:row;align-items:center;flex-wrap:wrap;min-height:{USER_LINE_HEIGHT}px;"
        )
    };

    format!(
        // 整张卡片写成一行：flex 容器里**不能有空白文本节点**。Blitz 会把
        // 「换行 + 缩进」当成一个匿名 flex item，用户名行因此凭空多出一行高
        // （实测 800px 卡片多出 43px），space-between 的 5 个图标也会被它挤偏。
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>KFC</title></head>
<body style="margin:0;padding:0;background:#ffffff;font-family:{font_family};"><div style="display:flex;flex-direction:row;align-items:flex-start;box-sizing:border-box;width:{width}px;padding:{padding}px;background:#ffffff;border:{border}px solid #e1e8ed;border-radius:15px;">{avatar}<div style="display:flex;flex-direction:column;width:{column}px;margin-left:{avatar_gap}px;"><div style="{user_row_style}"><span style="font-size:{name_font}px;font-weight:bold;color:#14171a;line-height:{user_line_height}px;">KFC</span>{verified}<span style="font-size:{handle_font}px;color:#657786;line-height:{user_line_height}px;margin-left:5px;white-space:nowrap;">@KFC · 刚刚</span></div><div style="width:{text}px;font-size:{body_font}px;line-height:{body_line_height};color:#14171a;white-space:pre-wrap;word-break:break-word;overflow-wrap:break-word;">{body}</div><div style="display:flex;flex-direction:row;align-items:center;justify-content:space-between;width:{column}px;height:{icon_size}px;margin-top:{actions_margin_top}px;">{icons}</div></div></div></body></html>"#,
        font_family = FONT_FAMILY,
        width = width,
        padding = PADDING,
        border = BORDER,
        avatar = assets::avatar_svg(),
        column = column,
        avatar_gap = AVATAR_GAP,
        user_row_style = user_row_style,
        user_line_height = USER_LINE_HEIGHT,
        name_font = NAME_FONT_SIZE,
        verified = assets::VERIFIED_BADGE,
        handle_font = HANDLE_FONT_SIZE,
        text = text,
        body_font = BODY_FONT_SIZE,
        body_line_height = body_line_height,
        body = body,
        icon_size = icon_size(column),
        actions_margin_top = ACTIONS_MARGIN_TOP,
        icons = assets::action_icons(icon_size(column)),
    )
}

/// 右侧内容列（用户名 / 正文 / 图标行）的可用宽度。
pub fn column_width(width: u32) -> u32 {
    width
        .saturating_sub(2 * BORDER + 2 * PADDING + AVATAR + AVATAR_GAP + RIGHT_GAP)
        .max(MIN_COLUMN)
}

/// 正文的可用宽度（内容列的 95%，与 `kfc.vue` 的 `width:95%` 对齐）。
pub fn text_width(width: u32) -> u32 {
    (column_width(width) * 95 / 100).max(1)
}

/// 图标行放得下 5 个图标 + 4 道间隙的尺寸：卡片窄时等比缩小，避免图标互相压住。
fn icon_size(column: u32) -> u32 {
    (column.saturating_sub(4 * ICON_GAP) / 5).min(ICON_SIZE)
}

/// 卡片里与正文行数无关的固定高度：边框 + 内边距 + 用户名行 + 图标行。
fn fixed_height() -> u32 {
    2 * BORDER + 2 * PADDING + USER_LINE_HEIGHT + ACTIONS_MARGIN_TOP + ICON_SIZE
}

/// 保守估算卡片高度（px），给 [`crate::render::render_card`] 做「渲之前先拦住」用。
///
/// 这里假设每个字符都占满 `30px` 一个字宽 —— 中文确实如此，英文会高估，
/// 所以结果只可能偏大：宁可提前报「太长」，也不去分配一张几百 MB 的位图。
pub fn estimate_height(content: &str, width: u32) -> u32 {
    // 每行最多放得下几个「满宽」字符。
    let per_line = (text_width(width) / BODY_FONT_SIZE).max(1) as u64;

    let mut lines: u64 = 0;
    for paragraph in content.split('\n') {
        let chars = paragraph.chars().count() as u64;
        // 空行本身也占一行高。
        lines += if chars == 0 {
            1
        } else {
            chars.div_ceil(per_line)
        };
    }
    // 正文为空时，卡片仍然有用户名行和图标行。
    let lines = lines.max(1);
    let height = fixed_height() as u64 + lines * BODY_LINE_HEIGHT as u64;
    u32::try_from(height).unwrap_or(u32::MAX)
}

/// HTML 转义：正文里的 `& < >` 必须转义，否则会被解析成标签。
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_specials() {
        assert_eq!(escape("a<b>&c"), "a&lt;b&gt;&amp;c");
    }

    #[test]
    fn keeps_line_breaks_and_width() {
        let html = build("第一行\n第二行", 800);
        assert!(html.contains("第一行\n第二行"), "换行必须原样保留");
        assert!(html.contains("width:800px"), "宽度必须写进卡片");
    }

    #[test]
    fn estimate_grows_with_lines() {
        let one = estimate_height("一行", 800);
        let many = estimate_height(
            &"行
"
            .repeat(10),
            800,
        );
        assert!(many > one, "行数越多估算越高：{many} vs {one}");
        assert!(
            estimate_height("", 800) >= fixed_height(),
            "空正文也要留出用户名行和图标行"
        );
    }

    #[test]
    fn column_narrows_with_card() {
        assert_eq!(column_width(800), 613);
        assert_eq!(text_width(800), 582);
        assert!(column_width(520) < column_width(800));
        assert_eq!(column_width(100), MIN_COLUMN, "极窄卡片走保底宽度");
    }
}
