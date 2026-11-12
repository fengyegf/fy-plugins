//! 图像生成：卡片数据 → Vue 模板的 `data` → `fuyubot.html2img` 服务 → 本机图片文件。
//!
//! **界面不在 Rust 里拼**：整张卡片的 HTML / CSS / Vue 指令都在
//! [`assets/card.html`](../../assets/card.html) 里（`<style>` + 原 `musiccard.vue` 的
//! 模板正文），本模块只做三件事：
//!
//! 1. 把业务数据凑成 Vue 模板的 `data`（[JSON](serde_json::Value)）；
//! 2. 调宿主上的 html2img 服务（`vue: true`），拿本机图片路径；
//! 3. 把错误翻译成插件层能展示的中文。
//!
//! 渲染器只有宿主那一份（hyper-render 的本地 fork：Blitz + Stylo + Taffy + Vello CPU），
//! 插件不再链接渲染引擎，也不再自己拼 HTML 字符串。
//!
//! 图片分工：封面是网易云 CDN 的**公网 URL**（不下载、不转 base64），二维码由 [`qr`]
//! 生成 PNG 后内联成 `data:image/png;base64,…`（约 15 KiB）。
//!
//! html2img 是**硬依赖**（`Plugin::inject`）：没有它出不了图，插件停在 Pending
//! 比启动后每条命令都失败更容易排查。

pub mod html2img;
mod qr;

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use crate::model::LyricLine;

/// 卡片模板：`<style>` + Vue 模板正文，编译期嵌入。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 画布尺寸（与 TS 版 `generateImage` 的 width/height 一致），固定不随内容变化。
pub const WIDTH: u32 = 650;
pub const HEIGHT: u32 = 1220;

/// 输出倍率：CSS 像素 1:1，产物就是 650x1220。
const SCALE: f32 = 1.0;
/// 输出格式：深色不透明底，JPEG 体积更小、Telegram 照片上传够用。
const FORMAT: &str = "jpg";

/// 交给服务的模板上限（html2img 默认 128 KiB）。
///
/// 模板本身只有几 KiB（封面是 URL，二维码在 `data` 里），这里只是兜住
/// 「哪天有人往模板里塞大图」的意外：提前给出明确错误，而不是让服务报错。
pub const MAX_HTML_BYTES: usize = 128 * 1024;

/// 歌词最多显示几行（模板里的歌词列按 5 行排版）。
const MAX_LYRIC_LINES: usize = 5;

/// 文字列宽度：卡片 598 - 左右内边距 26×2 - 二维码 76 - 间距 14 = 454。
const TEXT_WIDTH: f32 = 454.0;

/// 标题字号：原 `.vue` 的 30px，放不下时等比缩小，最低 20px。
const TITLE_FONT_SIZE: f32 = 30.0;
const TITLE_MIN_FONT_SIZE: f32 = 20.0;

/// 歌手 / 专辑行字号：原 `.vue` 的 17px，放不下时等比缩小，最低 13px。
const ARTISTS_FONT_SIZE: f32 = 17.0;
const ARTISTS_MIN_FONT_SIZE: f32 = 13.0;

/// 渲染一张卡片所需的全部数据（文本均已截断）。
pub struct Card<'a> {
    pub title: &'a str,
    pub artists: &'a str,
    pub album: &'a str,
    /// 封面**公网 URL**（网易云 CDN，`https://…`）。
    /// 空串 / 非 http(s) 时模板降级为纯渐变底，不整张失败。
    pub cover_url: &'a str,
    /// 二维码内容（歌曲链接）。
    pub qr_content: &'a str,
    /// 形如 "Nov"。
    pub month: &'a str,
    /// 形如 "04"。
    pub day: &'a str,
    pub lyrics: &'a [LyricLine],
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 渲染服务调用失败（服务不在 / 模板编译失败 / 渲染报错 / 应答不合法）。
    #[error("渲染服务失败：{0}")]
    Service(#[from] html2img::Error),
    /// 二维码生成失败（内容无法编码 / PNG 编码失败）。
    #[error("二维码生成失败：{0}")]
    Qr(String),
    /// 模板本身超过服务上限（编译期就该发现，留作兜底）。
    #[error("卡片模板 {0} 字节，超过 128 KiB 上限")]
    TemplateTooLarge(usize),
}

/// 渲染一张卡片，返回 html2img 产出的本机文件路径。
///
/// 调用方负责在用完之后删掉 `path`（html2img 自己也会在 24 小时后清理）。
///
/// 这是**阻塞**调用：渲染在调用方线程上跑完才返回，只能在业务线程里调。
pub fn render_card(host: &HostApi, card: &Card<'_>) -> Result<PathBuf, Error> {
    if TEMPLATE.len() > MAX_HTML_BYTES {
        return Err(Error::TemplateTooLarge(TEMPLATE.len()));
    }
    let data = view_data(card)?;
    let request = request(&data);
    Ok(html2img::render(host, &request)?)
}

/// 拼给 html2img 的入参。抽成纯函数，单测直接验字段。
fn request(data: &Value) -> html2img::Request<'_> {
    html2img::Request {
        html: TEMPLATE,
        // 卡片是 Vue 模板：循环 / 条件 / 插值都在模板里，Rust 只给数据。
        vue: true,
        data: Some(data),
        width: WIDTH,
        // 固定 650x1220：不传 height 会落到服务默认视口，auto_height 也不该开。
        height: Some(HEIGHT),
        scale: SCALE,
        format: FORMAT,
        auto_height: false,
        // JPEG 不支持透明；整页本来就是不透明底。
        transparent: false,
        dark: false,
    }
}

/// 把卡片映射成 Vue 模板要的 `data`（不调服务，单测 / examples 直接用）。
///
/// 字段与 `musiccard.vue` 的 props 对齐：`title` / `artists` / `album` / `cover` /
/// `qrCode` / `month` / `day` / `lyrics`，外加两个在模板里算不出来的排版值
/// （vue-template-html 没有函数调用 / Math）：适配后的 `titleSize` 与 `artistsSize`。
pub fn view_data(card: &Card<'_>) -> Result<Value, Error> {
    let qr_code = qr::data_uri(card.qr_content)?;
    let cover = cover_src(card.cover_url);
    let artists_line = format!("{} · {}", card.artists, card.album);
    let title_size = fit_font_size(card.title, TITLE_FONT_SIZE, TITLE_MIN_FONT_SIZE);
    let artists_size = fit_font_size(&artists_line, ARTISTS_FONT_SIZE, ARTISTS_MIN_FONT_SIZE);
    let lyrics: Vec<Value> = card
        .lyrics
        .iter()
        .take(MAX_LYRIC_LINES)
        .map(|line| {
            json!({
                "text": line.text,
                "translation": line.translation.as_deref().unwrap_or(""),
            })
        })
        .collect();

    Ok(json!({
        "title": card.title,
        "artists": card.artists,
        "album": card.album,
        // 没有可用封面时是 null：模板的 `v-if="cover"` 会跳过背景层，卡片封面留占位。
        "cover": cover,
        "qrCode": qr_code,
        "month": card.month,
        "day": card.day,
        "lyrics": lyrics,
        "titleSize": round1(title_size),
        "artistsSize": round1(artists_size),
    }))
}

/// 封面 URL 是否能用：只接受公网 `http(s)`。
///
/// html2img 的图片加载只支持 `data:` 与公网 `http(s)`，私网 / `file://` 会被拒绝。
/// 真遇到不可用的 URL 时不整张卡片失败，而是**只丢掉封面**（保留渐变底与其余版面），
/// 属于可接受的降级。
fn cover_src(url: &str) -> Option<String> {
    let url = url.trim();
    let lowered = url.to_ascii_lowercase();
    if lowered.starts_with("https://") || lowered.starts_with("http://") {
        Some(url.to_owned())
    } else {
        None
    }
}

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

/// 单行文本的适配字号：不超过 `base`，不小于 `min`，尽量塞进 [`TEXT_WIDTH`]。
///
/// 标题固定在 40px 行高、歌手行固定在 28px 行高，两行都必须保持**一行**：
/// 22 个汉字的标题按 30px 排需要 660px，必然换行并压到歌手行；这里在放不下时
/// 等比缩字号（常见长度的标题仍然正好 30px）。
fn fit_font_size(text: &str, base: f32, min: f32) -> f32 {
    let estimated = estimate_text_width(text, base);
    if estimated <= TEXT_WIDTH {
        return base;
    }
    (base * TEXT_WIDTH / estimated).clamp(min, base)
}

/// 字号保留 1 位小数，避免 `24.3f32` 序列化成 `24.299999…` 后被模板拼进 CSS。
fn round1(value: f32) -> f64 {
    (f64::from(value) * 10.0).round() / 10.0
}
