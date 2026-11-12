//! 头像处理：TDLib 给的照片字节 → ≤192px 的 JPEG → `data:` URI。
//!
//! html2img 只加载公网 `http(s)` 与 `data:` 图片，单次 HTML 默认上限 128 KiB。
//! Telegram 的大头像动辄几 MB，原图 base64 会直接把 HTML 顶爆；所以这里统一先缩到
//! 最长边 ≤ `MAX_EDGE` 像素、再按 `JPEG_QUALITY` 重编码，一张头像通常只有
//! 5–10 KB（base64 后十几 KB），整页 HTML 稳定在 128 KiB 以内。
//!
//! 失败（不是图片 / 解码出错 / 空字节）返回 `None`，调用方保留模板里的渐变占位，
//! 不让一张坏头像把整条 `/system` 拖失败。

use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};

/// 头像最长边上限（px）。卡片上最大的头像外框 180px、内圆 170px，192 足够 2x 屏。
pub const MAX_EDGE: u32 = 192;
/// JPEG 质量。80 对头像已看不出差别，体积比 90 小一半。
pub const JPEG_QUALITY: u8 = 80;

/// 把图片字节转成 `data:image/jpeg;base64,…`；解不了就返回 `None`。
pub fn data_uri(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(bytes).ok()?;
    encode(&decoded)
}

/// 把已解码的图片缩到 ≤192px、按 JPEG q80 重编码，返回 data URI。
fn encode(image: &DynamicImage) -> Option<String> {
    let (width, height) = fitted_size(image.width(), image.height());
    let resized = if (width, height) == (image.width(), image.height()) {
        image.clone()
    } else {
        image.resize_exact(width, height, FilterType::Lanczos3)
    };
    let rgb = flatten_on_white(&resized);
    let jpeg = encode_jpeg(&rgb)?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    ))
}

/// 等比缩放到最长边 ≤ `MAX_EDGE`（已经够小就不放大）。
fn fitted_size(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= MAX_EDGE || width == 0 || height == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = MAX_EDGE as f64 / longest as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// 透明头像（PNG）合成到白底：JPEG 没有 alpha，直接丢 alpha 会在边缘留黑边。
fn flatten_on_white(image: &DynamicImage) -> RgbImage {
    let rgba = image.to_rgba8();
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
        let alpha = src.0[3] as u32;
        if alpha == 255 {
            *dst = Rgb([src.0[0], src.0[1], src.0[2]]);
            continue;
        }
        let inverse = 255 - alpha;
        *dst = Rgb([
            ((src.0[0] as u32 * alpha + 255 * inverse + 127) / 255) as u8,
            ((src.0[1] as u32 * alpha + 255 * inverse + 127) / 255) as u8,
            ((src.0[2] as u32 * alpha + 255 * inverse + 127) / 255) as u8,
        ]);
    }
    rgb
}

/// 编码成 JPEG 字节；编码器失败时返回 `None`（不 panic）。
fn encode_jpeg(image: &RgbImage) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, JPEG_QUALITY);
    encoder
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(output)
}
