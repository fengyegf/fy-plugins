//! HTML 模板：把 [`Card`] 拼成 650x1220 的绝对定位 HTML，交给 hyper-render 渲染。
//!
//! 坐标都是**画布坐标**（不是卡片内坐标），所以所有元素都是 `body` 的直接子元素、
//! 各自 `position:absolute`，避免嵌套定位基准带来的偏移。
//!
//! 两个已知的 Blitz 限制：
//! - `<img>` / 背景图（含 data URI）不会渲染，所以封面槽、二维码槽只留空；
//! - `border-style:dashed` 的虚线不可靠，改用一排 2px 小方块拼出虚线
//!   （[`push_dashed_line`]）。

use crate::render::{Card, Error, HEIGHT, WIDTH};

/// 内层卡片的几何。
const CARD_LEFT: u32 = 26;
const CARD_TOP: u32 = 26;
const CARD_WIDTH: u32 = 598;
const CARD_HEIGHT: u32 = 1168;
const CARD_RADIUS: u32 = 26;

/// 文字列左边距（52 = 卡片 26 + 内边距 26）。
const TEXT_LEFT: u32 = 52;
/// 文字列宽度（598 - 26*2 - 二维码槽 76 - 间距 14 = 454）。
const TEXT_WIDTH: u32 = 454;

/// 虚线参数：546 宽、2px 高、7px 实线段 + 5px 空隙。
const DASH_LEFT: u32 = 52;
const DASH_WIDTH: u32 = 546;
const DASH_LEN: u32 = 7;
const DASH_GAP: u32 = 5;
const DASH_THICKNESS: u32 = 2;

/// 单行文本的显示宽度估算（px）：ASCII 约 0.55em，CJK / emoji 约 1em。
///
/// 只用于「放不下时等比缩字号」，比真实排版略保守即可——宁可小一点点，
/// 也不要溢出到封面或二维码上。
fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    text.chars()
        .map(|ch| if ch.is_ascii() { 0.55 } else { 1.0 })
        .sum::<f32>()
        * font_size
}

/// 单行文本的适配字号：不超过 `base`，不小于 `min`，尽量塞进 `max_width`。
///
/// 标题固定在 `top 620`、歌手行固定在 `top 660`，一行只有 40px 高。
/// 22 个汉字的标题按 30px 排需要 660px，必然换行并压到歌手行；这里在放不下时
/// 等比缩字号，保证标题永远是**一行**（常见长度的标题仍然正好 30px）。
fn fit_font_size(text: &str, max_width: f32, base: f32, min: f32) -> f32 {
    let estimated = estimate_text_width(text, base);
    if estimated <= max_width {
        return base;
    }
    (base * max_width / estimated).clamp(min, base)
}

/// 转义 HTML 文本节点里的特殊字符，防止歌曲名里的 `<&"'` 破坏版面。
///
/// 逐字符处理（而不是连续 `replace`），天然避免 `&amp;lt;` 这类二次转义。
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // 换行 / 制表符会被 Blitz 当成空白折叠，这里统一成空格，保持单行。
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// 卡片层样式。用固定坐标 + 绝对定位，保证版面稳定。
fn style() -> String {
    format!(
        r#"
        html, body {{ margin:0; padding:0; width:{width}px; height:{height}px; background:transparent; }}
        body {{ position:relative; color:#302d36;
                font-family:'Noto Sans SC','Microsoft YaHei','PingFang SC',sans-serif; }}
        .card {{ position:absolute; left:{cl}px; top:{ct}px; width:{cw}px; height:{ch}px;
                 border-radius:{cr}px; background-color:rgba(249,247,246,0.90); }}
        .title {{ position:absolute; left:{text_left}px; top:620px; width:{text_width}px; height:40px;
                  font-size:30px; line-height:40px; white-space:nowrap; overflow:hidden; }}
        .artists {{ position:absolute; left:{text_left}px; top:660px; width:{text_width}px; height:28px;
                    font-size:17px; line-height:28px; color:#756f79;
                    white-space:nowrap; overflow:hidden; }}
        .month {{ position:absolute; left:64px; top:738px; font-size:24px; line-height:30px; color:#716b78; }}
        .day {{ position:absolute; left:64px; top:768px; font-size:58px; line-height:66px; font-weight:300; }}
        .lyrics {{ position:absolute; left:190px; top:738px; width:392px; height:367px; overflow:hidden; }}
        .lyric {{ margin-bottom:13px; font-size:21px; line-height:29px; }}
        .translation {{ font-size:15px; line-height:22px; color:#77717b; }}
        .seg {{ position:absolute; height:{dash_thickness}px; }}
        .footer {{ position:absolute; left:{text_left}px; top:1105px; width:{dash_width}px; height:87px;
                   font-size:20px; line-height:87px; color:#645e68; text-align:center; }}
        .watermark {{ position:absolute; left:0; top:1198px; width:619px; height:15px;
                      font-size:12px; line-height:15px; text-align:right;
                      color:rgba(255,255,255,0.82); }}
        "#,
        width = WIDTH,
        height = HEIGHT,
        cl = CARD_LEFT,
        ct = CARD_TOP,
        cw = CARD_WIDTH,
        ch = CARD_HEIGHT,
        cr = CARD_RADIUS,
        text_left = TEXT_LEFT,
        text_width = TEXT_WIDTH,
        dash_thickness = DASH_THICKNESS,
        dash_width = DASH_WIDTH,
    )
}

/// 一排小方块拼出的虚线（代替 Blitz 不可靠的 `border-style:dashed`）。
fn push_dashed_line(body: &mut String, top: u32, color: &str) {
    let mut x = 0;
    while x < DASH_WIDTH {
        let w = DASH_LEN.min(DASH_WIDTH - x);
        body.push_str(&format!(
            "<div class=\"seg\" style=\"left:{}px;top:{}px;width:{}px;background-color:{};\"></div>",
            DASH_LEFT + x,
            top,
            w,
            color
        ));
        x += DASH_LEN + DASH_GAP;
    }
}

/// 歌词列：最多 5 行，每行「原文 + 可选译文」。
fn push_lyrics(body: &mut String, lyrics: &[crate::model::LyricLine]) {
    body.push_str("<div class=\"lyrics\">");
    for line in lyrics.iter().take(5) {
        body.push_str("<div class=\"lyric\">");
        body.push_str("<div>");
        body.push_str(&escape_html(&line.text));
        body.push_str("</div>");
        if let Some(translation) = line.translation.as_deref()
            && !translation.is_empty()
        {
            body.push_str("<div class=\"translation\">");
            body.push_str(&escape_html(translation));
            body.push_str("</div>");
        }
        body.push_str("</div>");
    }
    body.push_str("</div>");
}

/// 生成完整的 HTML 文档。
pub fn build_html(card: &Card<'_>) -> String {
    let title = escape_html(card.title);
    let artists = escape_html(card.artists);
    let album = escape_html(card.album);
    let title_size = fit_font_size(card.title, TEXT_WIDTH as f32, 30.0, 20.0);
    let artists_text = format!("{artists} · {album}");
    let artists_size = fit_font_size(&artists_text, TEXT_WIDTH as f32, 17.0, 13.0);

    let mut body = String::with_capacity(4096);
    body.push_str("<div class=\"card\"></div>");
    body.push_str(&format!(
        "<div class=\"title\" style=\"font-size:{title_size:.1}px;\">{title}</div>"
    ));
    body.push_str(&format!(
        "<div class=\"artists\" style=\"font-size:{artists_size:.1}px;\">{artists_text}</div>"
    ));
    body.push_str(&format!(
        "<div class=\"month\">{}</div><div class=\"day\">{}</div>",
        escape_html(card.month),
        escape_html(card.day)
    ));
    push_lyrics(&mut body, card.lyrics);

    // 头部（封面区与文字区之间）虚线。
    push_dashed_line(&mut body, 710, "rgba(57,48,70,0.16)");
    // 歌词区与页脚之间的虚线。
    push_dashed_line(&mut body, 1105, "rgba(57,48,70,0.18)");

    body.push_str("<div class=\"footer\">网易云音乐｜每日音乐卡片</div>");
    body.push_str("<div class=\"watermark\">Generated by FuyuBot</div>");

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{}</style></head><body>{body}</body></html>",
        style()
    )
}

/// 用 hyper-render 渲染透明卡片层，返回 PNG 字节。
pub fn render_layer(card: &Card<'_>) -> Result<Vec<u8>, Error> {
    let html = build_html(card);
    let config = hyper_render::Config::new()
        .width(WIDTH)
        .height(HEIGHT)
        .scale(1.0)
        .format(hyper_render::OutputFormat::Png)
        .transparent();
    hyper_render::render_to_png(&html, config).map_err(|e| Error::Render(e.to_string()))
}
