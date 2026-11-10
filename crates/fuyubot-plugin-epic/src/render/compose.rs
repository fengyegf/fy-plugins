//! 封面合成：占位槽位检测 + 图片裁切 + 圆角遮罩叠加。
//!
//! hyper-render 只能画出「洋红占位块」，真正的封面得我们自己贴：
//!
//! 1. [`find_slots`]：在渲染结果里找洋红像素，用「行内游程 + 相邻行合并」把像素
//!    归并成矩形（比 flood fill 简单，且天然按行从上到下扫描）；
//! 2. 每张封面 `load_from_memory` → 中心裁切到槽位宽高比 → `Lanczos3` 缩放到槽位
//!    尺寸 → 按顶部两角圆角遮罩与底图 alpha 混合；
//! 3. 没有封面（或字节解不开）的槽位填成深灰 [`NO_COVER_COLOR`]，不留洋红。
//!
//! 所有坐标都是**设备像素**（= CSS 像素 × scale），圆角半径也由调用方按 scale 放大。

use image::imageops::FilterType;
use image::{ImageEncoder, Rgba, RgbaImage};

use crate::render::RenderError;
use crate::render::template::NO_COVER_COLOR;

/// 洋红判定容差：抗锯齿/色彩管理会让边缘像素漂移，纯 `== 0xFF00FF` 太脆。
const MAGENTA_MIN_R: u8 = 240;
const MAGENTA_MAX_G: u8 = 60;
const MAGENTA_MIN_B: u8 = 240;

/// 误判成槽位的最小尺寸（设备像素）：太小的色块是噪声，不是槽位。
const MIN_SLOT_SIDE: u32 = 8;

/// 检测到的封面槽位（设备像素，左上角 + 尺寸）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 找所有洋红槽位，按 (y, x) 排序（= 先按行、再按列，与卡片顺序一致）。
pub fn find_slots(image: &RgbaImage) -> Vec<Slot> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let mut active: Vec<Rect> = Vec::new();
    let mut finished: Vec<Rect> = Vec::new();

    for y in 0..height {
        let runs = row_runs(image, y);
        let mut next: Vec<Rect> = Vec::with_capacity(runs.len());

        for (x0, x1) in runs {
            // 槽位之间水平不重叠，任意重叠都说明是同一个槽位的下一行。
            match active.iter().position(|rect| rect.overlaps(x0, x1)) {
                Some(index) => {
                    let mut rect = active.remove(index);
                    rect.x0 = rect.x0.min(x0);
                    rect.x1 = rect.x1.max(x1);
                    rect.y1 = y + 1;
                    next.push(rect);
                }
                None => next.push(Rect {
                    x0,
                    x1,
                    y0: y,
                    y1: y + 1,
                }),
            }
        }

        // 这一行没被延续的活动矩形到此为止。
        finished.append(&mut active);
        active = next;
    }
    finished.append(&mut active);

    let mut slots: Vec<Slot> = finished
        .into_iter()
        .filter_map(Rect::into_slot)
        .filter(|slot| slot.width >= MIN_SLOT_SIDE && slot.height >= MIN_SLOT_SIDE)
        .collect();
    slots.sort_by_key(|slot| (slot.y, slot.x));
    slots
}

/// 扫描一行，返回该行所有洋红游程 `[x0, x1)`。
fn row_runs(image: &RgbaImage, y: u32) -> Vec<(u32, u32)> {
    let width = image.width();
    let mut runs = Vec::new();
    let mut x = 0;
    while x < width {
        if is_magenta(*image.get_pixel(x, y)) {
            let start = x;
            while x < width && is_magenta(*image.get_pixel(x, y)) {
                x += 1;
            }
            runs.push((start, x));
        } else {
            x += 1;
        }
    }
    runs
}

/// 洋红像素判定（带容差）。
fn is_magenta(pixel: Rgba<u8>) -> bool {
    pixel[0] >= MAGENTA_MIN_R && pixel[1] <= MAGENTA_MAX_G && pixel[2] >= MAGENTA_MIN_B
}

/// 合并过程中的临时矩形（半开区间）。
struct Rect {
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
}

impl Rect {
    /// 与游程 `[x0, x1)` 是否有水平重叠。
    fn overlaps(&self, x0: u32, x1: u32) -> bool {
        self.x0 < x1 && x0 < self.x1
    }

    fn into_slot(self) -> Option<Slot> {
        Some(Slot {
            x: self.x0,
            y: self.y0,
            width: self.x1.checked_sub(self.x0)?,
            height: self.y1.checked_sub(self.y0)?,
        })
    }
}

/// 把封面合成进底图。
///
/// `targets` 是「卡片下标 → 槽位」的清单，只包含**需要贴图**的卡片：没有封面的
/// 卡片在模板里已经画成深灰，这里不必再动（也避免兜底坐标偏了反而盖住徽章）。
/// `covers[卡片下标]` 是封面的原始编码字节；字节解不开时按「无封面」处理，
/// 不会让整张卡片图失败。
///
/// 返回新图（底图不变，便于调用方复用）。
pub fn compose(
    base: &RgbaImage,
    targets: &[(usize, Slot)],
    covers: &[Option<Vec<u8>>],
    radius: f32,
) -> Result<RgbaImage, RenderError> {
    let no_cover = parse_hex(NO_COVER_COLOR)
        .ok_or_else(|| RenderError::Layout(format!("内置颜色非法: {NO_COVER_COLOR}")))?;

    let mut out = base.clone();
    for &(card_index, slot) in targets {
        if slot.width == 0 || slot.height == 0 {
            return Err(RenderError::Cover(format!(
                "第 {} 张卡片的槽位尺寸为 0",
                card_index + 1
            )));
        }

        let cover = match covers.get(card_index).and_then(Option::as_deref) {
            Some(bytes) => image::load_from_memory(bytes).ok(),
            None => None,
        };

        match cover.and_then(|decoded| fit_cover(&decoded, slot)) {
            Some(pixels) => paste_cover(&mut out, slot, &pixels, radius),
            None => fill_slot(&mut out, slot, no_cover, radius),
        }
    }
    Ok(out)
}

/// 中心裁切到槽位宽高比，再 Lanczos3 缩放到槽位尺寸（设备像素）。
fn fit_cover(decoded: &image::DynamicImage, slot: Slot) -> Option<RgbaImage> {
    let (src_width, src_height) = (decoded.width(), decoded.height());
    if src_width == 0 || src_height == 0 || slot.width == 0 || slot.height == 0 {
        return None;
    }

    let target_aspect = slot.width as f32 / slot.height as f32;
    let src_aspect = src_width as f32 / src_height as f32;

    // 中心裁切：按目标比例截出最大的一块，多的部分两边/上下均分。
    let (crop_width, crop_height) = if src_aspect > target_aspect {
        let width = (src_height as f32 * target_aspect).round() as u32;
        (width.clamp(1, src_width), src_height)
    } else {
        let height = (src_width as f32 / target_aspect).round() as u32;
        (src_width, height.clamp(1, src_height))
    };
    let crop_x = (src_width - crop_width) / 2;
    let crop_y = (src_height - crop_height) / 2;

    Some(
        decoded
            .crop_imm(crop_x, crop_y, crop_width, crop_height)
            .resize_exact(slot.width, slot.height, FilterType::Lanczos3)
            .to_rgba8(),
    )
}

/// 把缩放好的封面按圆角遮罩贴到槽位（只遮罩顶部两角）。
fn paste_cover(base: &mut RgbaImage, slot: Slot, cover: &RgbaImage, radius: f32) {
    for y in 0..slot.height {
        for x in 0..slot.width {
            let mask = corner_alpha(x, y, slot.width, slot.height, radius);
            if mask <= 0.0 {
                continue;
            }
            let src = cover.get_pixel(x, y);
            let alpha = mask * (src[3] as f32 / 255.0);
            blend_pixel(base, slot.x + x, slot.y + y, src[0], src[1], src[2], alpha);
        }
    }
}

/// 无封面：把槽位整块填成深灰（同样只剩顶部两角圆角）。
fn fill_slot(base: &mut RgbaImage, slot: Slot, color: Rgba<u8>, radius: f32) {
    for y in 0..slot.height {
        for x in 0..slot.width {
            let mask = corner_alpha(x, y, slot.width, slot.height, radius);
            if mask <= 0.0 {
                continue;
            }
            blend_pixel(
                base,
                slot.x + x,
                slot.y + y,
                color[0],
                color[1],
                color[2],
                mask,
            );
        }
    }
}

/// 顶部两角的圆角覆盖率（0.0–1.0，边缘 1px 抗锯齿）。
///
/// 坐标系原点是槽位左上角；只有左上、右上两个角会被遮罩，底部保持直角
/// （和卡片本身的圆角 + `overflow:hidden` 衔接）。
fn corner_alpha(x: u32, y: u32, width: u32, height: u32, radius: f32) -> f32 {
    let radius = radius
        .max(0.0)
        .min(width as f32 / 2.0)
        .min(height as f32 / 2.0);
    if radius <= 0.0 {
        return 1.0;
    }

    // 像素中心
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let max_x = width as f32;

    let mut alpha = 1.0f32;
    if py < radius {
        if px < radius {
            alpha = alpha.min(corner_coverage(px - radius, py - radius, radius));
        } else if px > max_x - radius {
            alpha = alpha.min(corner_coverage(px - (max_x - radius), py - radius, radius));
        }
    }
    alpha
}

/// 距圆心 `(dx, dy)` 的像素被圆覆盖的比例（1px 线性抗锯齿）。
fn corner_coverage(dx: f32, dy: f32, radius: f32) -> f32 {
    let distance = (dx * dx + dy * dy).sqrt();
    (radius - distance + 0.5).clamp(0.0, 1.0)
}

/// 源色以 `alpha` 覆盖到底图某个像素（底图保持不透明）。
fn blend_pixel(base: &mut RgbaImage, x: u32, y: u32, r: u8, g: u8, b: u8, alpha: f32) {
    if x >= base.width() || y >= base.height() {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    let dst = base.get_pixel_mut(x, y);
    let mix = |src: u8, dst: u8| -> u8 {
        (src as f32 * alpha + dst as f32 * (1.0 - alpha))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    dst[0] = mix(r, dst[0]);
    dst[1] = mix(g, dst[1]);
    dst[2] = mix(b, dst[2]);
    dst[3] = 255;
}

/// 把 RGBA 图编码成 PNG 字节。
pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, RenderError> {
    let mut output = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut output,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    encoder
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| RenderError::Png(error.to_string()))?;
    Ok(output)
}

/// 解析内置的 `#rrggbb` 常量。
fn parse_hex(hex: &str) -> Option<Rgba<u8>> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    Some(Rgba([
        ((value >> 16) & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        (value & 0xFF) as u8,
        255,
    ]))
}
