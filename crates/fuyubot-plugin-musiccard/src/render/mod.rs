//! 卡片渲染层：HTML 版面（hyper-render）+ 位图合成（image）。
//!
//! HTML 渲染思路参考 <https://github.com/thomasmost/hyper-render>。
//!
//! 一张卡片分三步合成（从底到顶）：
//!
//! 1. [`compose`]：封面 cover-fit 到 650x1220 → 高斯模糊 → 以 0.34 不透明度叠在
//!    `#292044` 上 → 再叠纵向渐变，得到**不透明**背景层；
//! 2. [`template`] + hyper-render：渲染 650x1220 的**透明**卡片层（内层卡片、
//!    文字、虚线、页脚、水印；封面槽与二维码槽留空）；
//! 3. [`compose`]：卡片层按 alpha 合成 → 封面圆角贴图 → 二维码贴图 → JPEG。
//!
//! hyper-render 0.2.2 不加载 `<img>` / 背景图（含 data URI），所以封面与二维码
//! 一律在 Rust 侧合成。

mod compose;
mod qr;
mod template;

use crate::model::LyricLine;

/// 画布尺寸。
pub const WIDTH: u32 = 650;
pub const HEIGHT: u32 = 1220;

/// JPEG 质量。
const JPEG_QUALITY: u8 = 90;

/// 渲染一张卡片所需的全部数据（文本均已截断）。
pub struct Card<'a> {
    pub title: &'a str,
    pub artists: &'a str,
    pub album: &'a str,
    /// 封面图原始字节（jpeg / png）。
    pub cover: &'a [u8],
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
    #[error("HTML 渲染失败：{0}")]
    Render(String),
    #[error("封面解码失败：{0}")]
    Cover(String),
    #[error("二维码生成失败：{0}")]
    Qr(String),
    #[error("图片合成失败：{0}")]
    Compose(String),
    #[error("图片编码失败：{0}")]
    Encode(String),
}

/// 输出 650x1220 JPEG。
pub fn render_card(card: &Card<'_>) -> Result<Vec<u8>, Error> {
    // 封面只解码一次：背景层和封面贴图共用同一份像素。
    let cover = compose::decode_cover(card.cover)?;

    // 第 1 层：模糊封面 + 紫色渐变，直接得到不透明 RGB 画布。
    let mut canvas = compose::build_background(&cover);

    // 第 2 层：hyper-render 输出 650x1220 透明 PNG（卡片 / 文字 / 虚线 / 水印）。
    let layer = template::render_layer(card)?;

    // 合成第 2 层：按 alpha 叠到背景上（卡片圆角外保持背景）。
    compose::composite_layer(&mut canvas, &layer)?;

    // 第 3 层：封面（cover-crop 546x546 + 20px 圆角蒙版）贴到 (52, 52)。
    compose::paste_cover(&mut canvas, &cover);

    // 第 4 层：二维码（quiet zone 3，228x228 绘制后缩到 76x76）贴到 (520, 616)。
    let qr = qr::render_qr(card.qr_content)?;
    compose::paste_rgba(&mut canvas, &qr, compose::QR_X, compose::QR_Y);

    // 输出 JPEG：Telegram photo 上传用，RGB 即可。
    compose::encode_jpeg(&canvas, JPEG_QUALITY)
}
