//! 二维码：`qrcode` 生成矩阵 → 228x228 位图 → 缩到 76x76 → PNG → base64 data URI。
//!
//! 这是插件里**唯一**还自己画出来的图：一张 76x76 的二维码 PNG 约 11 KiB（抗锯齿的
//! 四色灰阶），base64 之后约 15 KiB，远低于 html2img 的 128 KiB 上限，而渲染器支持
//! `<img src="data:…">`。封面 / 背景这类大图一律写公网 URL，不经过本模块。

use image::codecs::png::PngEncoder;
use image::imageops::{FilterType, resize};
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use qrcode::types::Color as ModuleColor;
use qrcode::{EcLevel, QrCode};

use crate::render::Error;

/// 版面里的显示边长。
pub const QR_EDGE: u32 = 76;

/// 绘制边长：先按 3 倍画，再缩到 76（228 / 76 = 3）。
const RENDER_EDGE: u32 = 228;

/// 静区宽度（模块数）。
const QUIET_ZONE: usize = 3;

/// 浅色 / 深色。
pub const LIGHT: [u8; 3] = [0xf9, 0xf7, 0xf6];
pub const DARK: [u8; 3] = [0x51, 0x4b, 0x58];

/// 生成二维码的 `data:image/png;base64,…` URI，直接写进模板的 `<img src>`。
pub fn data_uri(content: &str) -> Result<String, Error> {
    let png = render_png(content)?;
    Ok(format!("data:image/png;base64,{}", encode_base64(&png)))
}

/// 二维码 PNG 字节（76x76，不透明底色）。
pub fn render_png(content: &str) -> Result<Vec<u8>, Error> {
    let bitmap = render_bitmap(content)?;
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(bitmap.as_raw(), QR_EDGE, QR_EDGE, ExtendedColorType::Rgba8)
        .map_err(|error| Error::Qr(format!("二维码 PNG 编码失败：{error}")))?;
    Ok(png)
}

/// 生成二维码位图（76x76，不透明底色）。
fn render_bitmap(content: &str) -> Result<RgbaImage, Error> {
    let code = QrCode::with_error_correction_level(content.as_bytes(), EcLevel::M)
        .map_err(|error| Error::Qr(format!("二维码内容无法编码：{error}")))?;
    let modules = code.width();
    // 含静区的总模块数。
    let total = modules + QUIET_ZONE * 2;

    // 第一遍：228x228，按「像素中心落在哪个模块」取色。
    // 模块边界用整数运算，保证放大时不抖动。
    let mut large = RgbaImage::new(RENDER_EDGE, RENDER_EDGE);
    for (x, y, px) in large.enumerate_pixels_mut() {
        let module_x = x as usize * total / RENDER_EDGE as usize;
        let module_y = y as usize * total / RENDER_EDGE as usize;
        let inside = module_x >= QUIET_ZONE
            && module_y >= QUIET_ZONE
            && module_x < QUIET_ZONE + modules
            && module_y < QUIET_ZONE + modules;
        let dark =
            inside && code[(module_x - QUIET_ZONE, module_y - QUIET_ZONE)] == ModuleColor::Dark;
        let rgb = if dark { DARK } else { LIGHT };
        *px = Rgba([rgb[0], rgb[1], rgb[2], 255]);
    }

    // 第二遍：Lanczos3 缩到 76x76（228/76 = 3，正好 3 倍采样）。
    Ok(resize(&large, QR_EDGE, QR_EDGE, FilterType::Lanczos3))
}

/// 标准 base64（带 `=` 填充）。
///
/// 自己写十行而不是引 `base64`：整个 crate 只有这里需要编码，加依赖会改仓库根的
/// `Cargo.lock`（本次迁移不允许改它）。
fn encode_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}
