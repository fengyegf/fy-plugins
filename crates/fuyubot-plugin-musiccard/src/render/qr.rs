//! 二维码：`qrcode` 生成矩阵 → 228x228 位图（quiet zone 3 模块）→ 高质量缩到 76x76。
//!
//! 先生成 228x228 位图，再用 Lanczos3 缩到 76x76 以保持边缘干净。
//! hyper-render 不渲染 `<img>`，所以二维码在 Rust 侧生成。

use image::imageops::{FilterType, resize};
use image::{Rgba, RgbaImage};
use qrcode::types::Color as ModuleColor;
use qrcode::{EcLevel, QrCode};

use crate::render::Error;

/// 贴到卡片上的二维码边长。
pub const QR_EDGE: u32 = 76;

/// 绘制边长：先按 228x228 画，再缩 3 倍到 76。
pub const RENDER_EDGE: u32 = 228;

/// 静区宽度（模块数）。
pub const QUIET_ZONE: usize = 3;

/// 浅色 / 深色。
pub const LIGHT: [u8; 3] = [0xf9, 0xf7, 0xf6];
pub const DARK: [u8; 3] = [0x51, 0x4b, 0x58];

/// 生成二维码位图（76x76，不透明底色）。
pub fn render_qr(content: &str) -> Result<RgbaImage, Error> {
    let code = QrCode::with_error_correction_level(content.as_bytes(), EcLevel::M)
        .map_err(|e| Error::Qr(e.to_string()))?;
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
