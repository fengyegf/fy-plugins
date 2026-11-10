//! 位图合成：背景层（模糊封面 + 渐变）、卡片层 alpha 合成、封面 / 二维码贴图、JPEG 编码。
//!
//! 全部是 CPU 整数运算，不依赖 GPU；`image` 0.25 没有内置模糊，这里用
//! 「可分离盒式模糊 × 3 遍」近似高斯模糊（O(像素数)，实测对 650x1220 够快）。

use image::imageops::{FilterType, crop_imm, resize};
use image::{DynamicImage, Rgb, RgbImage, RgbaImage};

use crate::render::{Error, HEIGHT, WIDTH};

/// 背景底色 `#292044`。
pub const BASE_COLOR: [u8; 3] = [0x29, 0x20, 0x44];

/// 模糊封面叠在底色上的不透明度。
const COVER_ALPHA: f32 = 0.34;

/// 盒式模糊半径与遍数：3 遍半径 16 的盒式模糊 ≈ sigma 16.5 的高斯模糊，
/// 视觉上对应半径约 24–32px 的柔化强度。
const BLUR_RADIUS: u32 = 16;
const BLUR_PASSES: u32 = 3;

/// 封面贴图位置与尺寸。
pub const COVER_X: u32 = 52;
pub const COVER_Y: u32 = 52;
pub const COVER_SIZE: u32 = 546;
/// 封面圆角半径。
pub const COVER_RADIUS: u32 = 20;

/// 二维码贴图位置。
pub const QR_X: u32 = 520;
pub const QR_Y: u32 = 616;

/// 解码封面原始字节（jpeg / png）。
pub fn decode_cover(bytes: &[u8]) -> Result<DynamicImage, Error> {
    if bytes.is_empty() {
        return Err(Error::Cover("封面字节为空".to_string()));
    }
    image::load_from_memory(bytes).map_err(|e| Error::Cover(e.to_string()))
}

/// 纵向渐变采样：返回 `(rgb, alpha)`，`y = 0` 对应顶端、`y = HEIGHT-1` 对应底端。
///
/// 渐变从 `rgba(80,38,41,.56)` 到 `rgba(38,31,84,.82)`。rgb 用 f32 保存中间值，
/// 避免逐像素插值时的舍入偏差。
pub fn gradient_overlay(y: u32) -> ([f32; 3], f32) {
    const TOP_RGB: [f32; 3] = [80.0, 38.0, 41.0];
    const TOP_ALPHA: f32 = 0.56;
    const BOTTOM_RGB: [f32; 3] = [38.0, 31.0, 84.0];
    const BOTTOM_ALPHA: f32 = 0.82;

    let t = if HEIGHT <= 1 {
        0.0
    } else {
        (y as f32 / (HEIGHT - 1) as f32).clamp(0.0, 1.0)
    };
    let rgb = [
        TOP_RGB[0] + (BOTTOM_RGB[0] - TOP_RGB[0]) * t,
        TOP_RGB[1] + (BOTTOM_RGB[1] - TOP_RGB[1]) * t,
        TOP_RGB[2] + (BOTTOM_RGB[2] - TOP_RGB[2]) * t,
    ];
    let alpha = TOP_ALPHA + (BOTTOM_ALPHA - TOP_ALPHA) * t;
    (rgb, alpha)
}

/// 背景层：封面 cover-fit → 高斯模糊 → 34% 叠底 → 纵向渐变。
pub fn build_background(cover: &DynamicImage) -> RgbImage {
    // 封面先铺满整张画布（可能上下/左右被裁掉），再模糊。
    let fitted = cover_fit(&cover.to_rgb8(), WIDTH, HEIGHT, FilterType::Triangle);
    let blurred = gaussian_blur(&fitted, BLUR_RADIUS, BLUR_PASSES);

    let mut canvas = RgbImage::from_pixel(WIDTH, HEIGHT, Rgb(BASE_COLOR));
    for (x, y, dst) in canvas.enumerate_pixels_mut() {
        let src = blurred.get_pixel(x, y);
        // 先按 0.34 把模糊封面叠到底色上。
        for (dst, src) in dst.0.iter_mut().zip(src.0.iter()) {
            *dst = (*dst as f32 * (1.0 - COVER_ALPHA) + *src as f32 * COVER_ALPHA)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        // 再叠纵向渐变（自身带 alpha，按 source-over 混合：src * alpha + dst * (1-alpha)）。
        let (rgb, alpha) = gradient_overlay(y);
        for (dst, rgb) in dst.0.iter_mut().zip(rgb.iter()) {
            *dst = (*dst as f32 * (1.0 - alpha) + *rgb * alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    canvas
}

/// 把图片按 cover 语义铺满 `tw x th`（等比放大后居中裁切）。
fn cover_fit(src: &RgbImage, tw: u32, th: u32, filter: FilterType) -> RgbImage {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 {
        return RgbImage::from_pixel(tw, th, Rgb(BASE_COLOR));
    }
    let scale = (tw as f32 / sw as f32).max(th as f32 / sh as f32);
    let nw = ((sw as f32 * scale).ceil() as u32).max(tw);
    let nh = ((sh as f32 * scale).ceil() as u32).max(th);

    let resized = if nw == sw && nh == sh {
        src.clone()
    } else {
        resize(src, nw, nh, filter)
    };
    let x = (nw - tw) / 2;
    let y = (nh - th) / 2;
    crop_imm(&resized, x, y, tw, th).to_image()
}

/// 高斯模糊的近似实现：连续 3 遍可分离盒式模糊。
pub fn gaussian_blur(src: &RgbImage, radius: u32, passes: u32) -> RgbImage {
    let mut current = src.clone();
    for _ in 0..passes {
        let horizontal = box_blur_pass(&current, radius, true);
        current = box_blur_pass(&horizontal, radius, false);
    }
    current
}

/// 单方向盒式模糊（运行和实现，O(像素数)）；边界按最近像素夹取。
fn box_blur_pass(src: &RgbImage, radius: u32, horizontal: bool) -> RgbImage {
    let (w, h) = src.dimensions();
    let mut out = RgbImage::new(w, h);
    let r = radius as i64;
    let window = 2 * radius + 1;
    let max_x = w.saturating_sub(1) as i64;
    let max_y = h.saturating_sub(1) as i64;

    // 夹取坐标：盒式窗口越界时复用边缘像素（相当于 edge clamp）。
    let clamp_x = |x: i64| x.clamp(0, max_x) as u32;
    let clamp_y = |y: i64| y.clamp(0, max_y) as u32;

    if horizontal {
        for y in 0..h {
            let mut sum = [0u32; 3];
            for offset in -r..=r {
                let px = src.get_pixel(clamp_x(offset), y);
                for (sum, channel) in sum.iter_mut().zip(px.0.iter()) {
                    *sum += *channel as u32;
                }
            }
            for x in 0..w {
                let px = out.get_pixel_mut(x, y);
                for (dst, sum) in px.0.iter_mut().zip(sum.iter()) {
                    *dst = (sum / window) as u8;
                }
                // 窗口右移一格：先移出左端、再移入右端。
                let leaving = src.get_pixel(clamp_x(x as i64 - r), y);
                let entering = src.get_pixel(clamp_x(x as i64 + r + 1), y);
                for ((sum, leaving), entering) in
                    sum.iter_mut().zip(leaving.0.iter()).zip(entering.0.iter())
                {
                    *sum = *sum - *leaving as u32 + *entering as u32;
                }
            }
        }
    } else {
        for x in 0..w {
            let mut sum = [0u32; 3];
            for offset in -r..=r {
                let px = src.get_pixel(x, clamp_y(offset));
                for (sum, channel) in sum.iter_mut().zip(px.0.iter()) {
                    *sum += *channel as u32;
                }
            }
            for y in 0..h {
                let px = out.get_pixel_mut(x, y);
                for (dst, sum) in px.0.iter_mut().zip(sum.iter()) {
                    *dst = (sum / window) as u8;
                }
                let leaving = src.get_pixel(x, clamp_y(y as i64 - r));
                let entering = src.get_pixel(x, clamp_y(y as i64 + r + 1));
                for ((sum, leaving), entering) in
                    sum.iter_mut().zip(leaving.0.iter()).zip(entering.0.iter())
                {
                    *sum = *sum - *leaving as u32 + *entering as u32;
                }
            }
        }
    }
    out
}

/// 判断 hyper-render（vello_cpu）输出的 RGBA 是否为 **premultiplied**。
///
/// 依据：premultiplied 中每个颜色分量必然 `<= alpha`；只要有一个半透明像素
/// 的某个通道大于 alpha，就说明是 straight alpha。cover 槽内还有全透明像素、
/// 卡片底色 0.90 近白（straight 时 249 > 230），足以稳定识别。
pub fn layer_is_premultiplied(layer: &RgbaImage) -> bool {
    let mut checked = 0;
    for px in layer.pixels() {
        let alpha = px.0[3];
        if alpha == 0 || alpha == 255 {
            continue;
        }
        checked += 1;
        for channel in 0..3 {
            if px.0[channel] > alpha {
                return false;
            }
        }
        // 前若干个半透明像素足以定论，避免全图扫描。
        if checked >= 4096 {
            break;
        }
    }
    // 没有半透明像素时两种公式等价，按 premultiplied 处理即可。
    true
}

/// 把 hyper-render 的卡片层 PNG 合成到不透明背景上。
///
/// 合成公式（premultiplied）：`out = src + dst * (1 - alpha)`；
/// 若检测到是 straight alpha，则先把 src 预乘再套同一公式。
pub fn composite_layer(canvas: &mut RgbImage, png: &[u8]) -> Result<(), Error> {
    let layer = image::load_from_memory(png)
        .map_err(|e| Error::Render(format!("卡片层 PNG 解码失败：{e}")))?
        .to_rgba8();
    if layer.dimensions() != (WIDTH, HEIGHT) {
        return Err(Error::Compose(format!(
            "卡片层尺寸应为 {WIDTH}x{HEIGHT}，实际 {:?}",
            layer.dimensions()
        )));
    }

    let premultiplied = layer_is_premultiplied(&layer);
    for (dst, src) in canvas.pixels_mut().zip(layer.pixels()) {
        let alpha = src.0[3] as u32;
        if alpha == 0 {
            continue;
        }
        if alpha == 255 {
            *dst = Rgb([src.0[0], src.0[1], src.0[2]]);
            continue;
        }
        let inverse = 255 - alpha;
        for channel in 0..3 {
            let src_channel = if premultiplied {
                src.0[channel] as u32
            } else {
                (src.0[channel] as u32 * alpha + 127) / 255
            };
            let value = src_channel + (dst.0[channel] as u32 * inverse + 127) / 255;
            dst.0[channel] = value.min(255) as u8;
        }
    }
    Ok(())
}

/// 圆角矩形蒙版，返回 `size * size` 的覆盖度（0..=255）。
///
/// 用「像素中心到内缩矩形最近点的距离」算覆盖度：半径边界处覆盖度 0.5，
/// 天然得到 1px 抗锯齿，不需要超采样。
pub fn rounded_mask(size: u32, radius: u32) -> Vec<u8> {
    let side = size as f32;
    let r = radius as f32;
    let mut mask = vec![0u8; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let cx = x as f32 + 0.5;
            let cy = y as f32 + 0.5;
            // 内缩矩形的最近点：矩形内部时距离为 0。
            let nearest_x = cx.clamp(r, side - r);
            let nearest_y = cy.clamp(r, side - r);
            let distance = ((cx - nearest_x).powi(2) + (cy - nearest_y).powi(2)).sqrt();
            let coverage = (r + 0.5 - distance).clamp(0.0, 1.0);
            mask[(y * size + x) as usize] = (coverage * 255.0).round() as u8;
        }
    }
    mask
}

/// 封面：cover-crop 到 546x546 → 圆角蒙版 → 贴到 (52, 52)。
pub fn paste_cover(canvas: &mut RgbImage, cover: &DynamicImage) {
    let fitted = cover_fit(
        &cover.to_rgb8(),
        COVER_SIZE,
        COVER_SIZE,
        FilterType::Lanczos3,
    );
    let mask = rounded_mask(COVER_SIZE, COVER_RADIUS);

    for (x, y, src) in fitted.enumerate_pixels() {
        let coverage = mask[(y * COVER_SIZE + x) as usize] as u32;
        if coverage == 0 {
            continue;
        }
        let dst = canvas.get_pixel_mut(COVER_X + x, COVER_Y + y);
        if coverage == 255 {
            *dst = *src;
            continue;
        }
        let inverse = 255 - coverage;
        for channel in 0..3 {
            dst.0[channel] =
                ((src.0[channel] as u32 * coverage + dst.0[channel] as u32 * inverse + 127) / 255)
                    .min(255) as u8;
        }
    }
}

/// 把 RGBA 位图按 straight alpha 贴到画布 `(x0, y0)`（二维码走这里）。
pub fn paste_rgba(canvas: &mut RgbImage, image: &RgbaImage, x0: u32, y0: u32) {
    for (x, y, src) in image.enumerate_pixels() {
        let alpha = src.0[3] as u32;
        if alpha == 0 {
            continue;
        }
        let dx = x0 + x;
        let dy = y0 + y;
        if dx >= WIDTH || dy >= HEIGHT {
            continue;
        }
        let dst = canvas.get_pixel_mut(dx, dy);
        if alpha == 255 {
            *dst = Rgb([src.0[0], src.0[1], src.0[2]]);
            continue;
        }
        let inverse = 255 - alpha;
        for channel in 0..3 {
            dst.0[channel] = (((src.0[channel] as u32 * alpha + 127) / 255)
                + (dst.0[channel] as u32 * inverse + 127) / 255)
                .min(255) as u8;
        }
    }
}

/// 编码为 JPEG（RGB，quality 约 90）。
pub fn encode_jpeg(canvas: &RgbImage, quality: u8) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality);
    encoder
        .encode(
            canvas.as_raw(),
            canvas.width(),
            canvas.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| Error::Encode(e.to_string()))?;
    Ok(output)
}
