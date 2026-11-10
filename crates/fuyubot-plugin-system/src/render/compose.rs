//! 位图合成：渐变背景 → 背景图 → 白色蒙版 → 叠 HTML 层 → 贴头像 → 编码 JPEG。
//!
//! 全部是 CPU 整数运算，不依赖 GPU，也不加载任何网络资源。三层各自负责一件事：
//!
//! 1. **背景层**（本模块）：`system.vue` 的 `linear-gradient(132.5deg, #ffffff, #ceeddb)`
//!    在 Rust 侧逐像素算出来，再按需 cover-fit 一张背景图，最后叠上 vue 里那层白色蒙版
//!    （水平方向 alpha 从 1.0 线性降到 0.09）；
//! 2. **HTML 层**（[`super::template`]）：hyper-render 渲出的 1400×800 透明 PNG，
//!    这里按 alpha 合成到背景上；
//! 3. **头像层**（本模块）：机器人 / 管理员的照片 cover 裁切成圆形贴进 HTML 画好的
//!    白边圆环里。取不到照片或解码失败时**保留 HTML 的渐变占位**，不影响出图。

use image::imageops::{FilterType, crop_imm, resize};
use image::{DynamicImage, Rgb, RgbImage, RgbaImage};

use crate::core::model::Assets;
use crate::render::layout;
use crate::render::{Error, HEIGHT, WIDTH};

/// 背景图叠在渐变底上的不透明度：整幅铺满但留一点渐变底子，右侧更通透。
const BACKGROUND_ALPHA: f32 = 0.86;
/// 白色蒙版在右侧的最小不透明度（vue 的 `rgba(255,255,255,0.09)`）。
const MASK_MIN_ALPHA: f32 = 0.09;
/// 白色蒙版到位时的 x：1400 × 70% = 980。
const MASK_RAMP_X: f32 = 980.0;

/// 背景层：渐变 → 背景图（可选）→ 白色蒙版，得到一张**不透明** RGB 画布。
pub fn build_background(assets: &Assets) -> RgbImage {
    let mut canvas = gradient_base();

    if let Some(bytes) = assets.background.as_deref()
        && let Ok(decoded) = image::load_from_memory(bytes)
    {
        let fitted = cover_fit(&decoded.to_rgb8(), WIDTH, HEIGHT, FilterType::Triangle);
        blend_over(&mut canvas, &fitted, BACKGROUND_ALPHA);
    }

    apply_white_mask(&mut canvas);
    canvas
}

/// `linear-gradient(132.5deg, #ffffff 0%, #ceeddb 100%)`。
///
/// CSS 的角度以「向上」为 0°、顺时针增加，所以方向向量是 `(sinθ, -cosθ)`
/// （画布 y 轴向下）。渐变线长度 = |W·sinθ| + |H·cosθ|，像素投影到渐变线上取比例。
fn gradient_base() -> RgbImage {
    const TOP: [f32; 3] = [255.0, 255.0, 255.0];
    const BOTTOM: [f32; 3] = [206.0, 237.0, 219.0];
    const ANGLE_DEG: f32 = 132.5;

    let theta = ANGLE_DEG.to_radians();
    let (dx, dy) = (theta.sin(), -theta.cos());
    let length = (WIDTH as f32 * dx.abs() + HEIGHT as f32 * dy.abs()).max(1.0);
    let center_x = WIDTH as f32 / 2.0;
    let center_y = HEIGHT as f32 / 2.0;

    let mut canvas = RgbImage::new(WIDTH, HEIGHT);
    for (x, y, pixel) in canvas.enumerate_pixels_mut() {
        let projection = (x as f32 + 0.5 - center_x) * dx + (y as f32 + 0.5 - center_y) * dy;
        let t = (0.5 + projection / length).clamp(0.0, 1.0);
        for channel in 0..3 {
            pixel.0[channel] = (TOP[channel] + (BOTTOM[channel] - TOP[channel]) * t).round() as u8;
        }
    }
    canvas
}

/// `linear-gradient(90deg, rgba(255,255,255,1) 0%, rgba(255,255,255,.09) 70%)`。
fn apply_white_mask(canvas: &mut RgbImage) {
    let mut alphas = vec![MASK_MIN_ALPHA; WIDTH as usize];
    for (x, alpha) in alphas.iter_mut().enumerate() {
        let t = ((x as f32 + 0.5) / MASK_RAMP_X).clamp(0.0, 1.0);
        *alpha = 1.0 + (MASK_MIN_ALPHA - 1.0) * t;
    }

    for (x, _y, pixel) in canvas.enumerate_pixels_mut() {
        let alpha = alphas[x as usize];
        if alpha <= 0.0 {
            continue;
        }
        let inverse = 1.0 - alpha;
        for channel in 0..3 {
            pixel.0[channel] = (pixel.0[channel] as f32 * inverse + 255.0 * alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
}

/// 把同尺寸的 RGB 图按 `alpha` 叠到画布上（source-over，画布不透明）。
fn blend_over(canvas: &mut RgbImage, layer: &RgbImage, alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    let inverse = 1.0 - alpha;
    for (dst, src) in canvas.pixels_mut().zip(layer.pixels()) {
        for channel in 0..3 {
            dst.0[channel] = (dst.0[channel] as f32 * inverse + src.0[channel] as f32 * alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
}

/// 把图片按 cover 语义铺满 `target_w × target_h`（等比放大后居中裁切）。
fn cover_fit(src: &RgbImage, target_w: u32, target_h: u32, filter: FilterType) -> RgbImage {
    let (source_w, source_h) = src.dimensions();
    if source_w == 0 || source_h == 0 {
        return RgbImage::from_pixel(target_w, target_h, Rgb([255, 255, 255]));
    }
    let scale = (target_w as f32 / source_w as f32).max(target_h as f32 / source_h as f32);
    let width = ((source_w as f32 * scale).ceil() as u32).max(target_w);
    let height = ((source_h as f32 * scale).ceil() as u32).max(target_h);

    let resized = if width == source_w && height == source_h {
        src.clone()
    } else {
        resize(src, width, height, filter)
    };
    let x = (width - target_w) / 2;
    let y = (height - target_h) / 2;
    crop_imm(&resized, x, y, target_w, target_h).to_image()
}

/// 圆形蒙版：返回 `size × size` 的覆盖度（0..=255），边缘按像素中心距离做 1px 抗锯齿。
fn circular_mask(size: u32) -> Vec<u8> {
    circle_mask(size, size / 2)
}

/// 半径为 `radius` 的圆形蒙版（与 [`circular_mask`] 同一套覆盖度算法）。
fn circle_mask(size: u32, radius: u32) -> Vec<u8> {
    let center = size as f32 / 2.0;
    let r = radius as f32;
    let mut mask = vec![0u8; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (r + 0.5 - distance).clamp(0.0, 1.0);
            mask[(y * size + x) as usize] = (coverage * 255.0).round() as u8;
        }
    }
    mask
}

/// 判断 hyper-render（vello_cpu）输出的 RGBA 是否为 **premultiplied**。
///
/// premultiplied 中每个颜色分量必然 `<= alpha`；只要有一个半透明像素的某个通道
/// 大于 alpha，就说明是 straight alpha（做法与 `musiccard` 一致）。
pub fn layer_is_premultiplied(layer: &RgbaImage) -> bool {
    let mut checked = 0;
    for pixel in layer.pixels() {
        let alpha = pixel.0[3];
        if alpha == 0 || alpha == 255 {
            continue;
        }
        checked += 1;
        for channel in 0..3 {
            if pixel.0[channel] > alpha {
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

/// 把 hyper-render 的透明卡片层合成到不透明背景上。
pub fn composite_layer(canvas: &mut RgbImage, png: &[u8]) -> Result<(), Error> {
    let layer = image::load_from_memory(png)
        .map_err(|error| Error::Image(format!("卡片层 PNG 解码失败：{error}")))?
        .to_rgba8();
    if layer.dimensions() != (WIDTH, HEIGHT) {
        return Err(Error::Image(format!(
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

/// 贴两张圆头像：机器人（180 外框 / 170 内圆）与管理员（100 / 90）。
///
/// 照片解码失败时静默跳过——HTML 层已经画好渐变占位，整张卡片不该因此失败。
pub fn paste_avatars(canvas: &mut RgbImage, assets: &Assets) {
    if let Some(bytes) = assets.bot_photo.as_deref() {
        paste_circle(
            canvas,
            bytes,
            layout::BOT_AVATAR_INNER_LEFT,
            layout::BOT_AVATAR_INNER_TOP,
            layout::BOT_AVATAR_INNER_SIZE,
        );
    }
    if let Some(bytes) = assets.admin_photo.as_deref() {
        paste_circle(
            canvas,
            bytes,
            layout::ADMIN_AVATAR_INNER_LEFT,
            layout::ADMIN_AVATAR_INNER_TOP,
            layout::ADMIN_AVATAR_INNER_SIZE,
        );
    }
}

/// 把一张照片 cover 裁成方形 → 圆形蒙版 → 贴到 `(left, top)`。
fn paste_circle(canvas: &mut RgbImage, bytes: &[u8], left: u32, top: u32, size: u32) {
    let Ok(decoded) = image::load_from_memory(bytes) else {
        return;
    };
    let fitted = cover_fit(&decoded.to_rgb8(), size, size, FilterType::Lanczos3);
    let mask = circular_mask(size);

    for (x, y, src) in fitted.enumerate_pixels() {
        let coverage = mask[(y * size + x) as usize] as u32;
        if coverage == 0 {
            continue;
        }
        let dx = left + x;
        let dy = top + y;
        if dx >= WIDTH || dy >= HEIGHT {
            continue;
        }
        let dst = canvas.get_pixel_mut(dx, dy);
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
        .map_err(|error| Error::Image(error.to_string()))?;
    Ok(output)
}

/// 解码一张图像字节（jpeg / png），给测试与调试用。
pub fn decode(bytes: &[u8]) -> Result<DynamicImage, Error> {
    image::load_from_memory(bytes).map_err(|error| Error::Image(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_matches_css_ends() {
        let canvas = gradient_base();
        // 132.5° 的渐变线从左上偏上走到右下，左上角接近白、右下角接近 #ceeddb。
        let top_left = canvas.get_pixel(0, 0);
        let bottom_right = canvas.get_pixel(WIDTH - 1, HEIGHT - 1);
        assert!(top_left.0[1] >= top_left.0[2], "左上应当偏白");
        assert!(
            bottom_right.0[1] < bottom_right.0[0] || bottom_right.0[2] > 200,
            "右下应当偏绿：{bottom_right:?}"
        );
        // 两点之间确实有变化，不是纯色。
        assert_ne!(top_left, bottom_right);
    }

    #[test]
    fn white_mask_is_transparent_on_the_left_and_clear_on_the_right() {
        // 造一张纯黑画布，蒙版后左侧接近白、右侧仍明显偏黑。
        let mut canvas = RgbImage::from_pixel(WIDTH, HEIGHT, Rgb([0, 0, 0]));
        apply_white_mask(&mut canvas);
        let left = canvas.get_pixel(0, 0).0[0];
        let right = canvas.get_pixel(WIDTH - 1, 0).0[0];
        assert!(left > 250, "最左侧应当几乎全白，实际 {left}");
        assert!(right < 60, "最右侧应当透出背景，实际 {right}");
        // 70% 处已经到达最小值（0.09 → 约 23）。
        let at_ramp = canvas.get_pixel((MASK_RAMP_X as u32).min(WIDTH - 1), 0).0[0];
        assert!(at_ramp < 100, "70% 处蒙版应当基本退场，实际 {at_ramp}");
    }

    #[test]
    fn circular_mask_only_keeps_the_inside() {
        let mask = circular_mask(90);
        assert_eq!(mask[(45 * 90 + 45) as usize], 255, "圆心必然完全覆盖");
        assert_eq!(mask[0], 0, "左上角在圆外");
        assert_eq!(mask[(89 * 90 + 89) as usize], 0, "右下角在圆外");
        // 半径方向走一半也在圆内。
        assert_eq!(mask[(45 * 90 + 45 + 40) as usize], 255);
    }

    #[test]
    fn cover_fit_always_fills_the_target() {
        let source = RgbImage::from_pixel(300, 100, Rgb([10, 20, 30]));
        let fitted = cover_fit(&source, 1400, 800, FilterType::Triangle);
        assert_eq!(fitted.dimensions(), (1400, 800));
        assert_eq!(fitted.get_pixel(0, 0).0, [10, 20, 30]);
    }
}
