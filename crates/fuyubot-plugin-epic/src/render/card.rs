//! 渲染编排：模板 → hyper-render → 槽位检测 → 封面合成 → PNG。
//!
//! 这是 [`crate::render::render`] 的实现体，也是唯一碰渲染引擎的地方：
//!
//! 1. 按 `max_cards` 截断游戏列表，给每张卡片标记「当前免费 / 即将推出 / 有无封面」；
//! 2. [`template`] 出 HTML，hyper-render 渲染成 PNG（封面位置是洋红占位块）；
//! 3. [`compose::find_slots`] 在 PNG 上把这些洋红块找回来；数量对不上时退回
//!    [`Layout::slot_rect`] 的解析式坐标（宁可偏几个像素，也不能 panic 或不出图）；
//! 4. 逐张把封面中心裁切、缩放、圆角遮罩后贴上去，重新编码 PNG。
//!
//! 全部无封面时模板已经直接画深灰占位，可直接返回 hyper-render 的 PNG
//! （省一次解码 + 编码）。

use std::sync::{Mutex, OnceLock};

use hyper_render::{Config, OutputFormat};

use crate::core::epic::FreeGames;
use crate::render::template::{self, Card, Layout};
use crate::render::{RenderError, RenderOptions, compose};

/// 渲染串行锁。
///
/// 引擎内部有全局字体集合与缓存，插件又会在多条业务线程里并发出图；串行化是
/// 这里最简单也最稳妥的保证（单张图 10–50 ms，串行不构成瓶颈）。
fn render_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 渲染一张卡片图（PNG）。
pub fn render(
    games: &FreeGames,
    covers: &[Option<Vec<u8>>],
    options: &RenderOptions,
) -> Result<Vec<u8>, RenderError> {
    let _guard = render_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let cards = plan(games, covers, options);
    let layout = template::layout(cards.len(), options);
    let html = template::document(&cards, &layout, options);
    let scale = normalized_scale(options.scale);

    let png = hyper_render::render_to_png(
        &html,
        Config::new()
            .width(layout.width)
            .height(layout.height)
            .scale(scale)
            .format(OutputFormat::Png)
            .background(options.background),
    )
    .map_err(|error| RenderError::Engine(error.to_string()))?;

    // 全都没有封面：模板里的槽位本来就是深灰，直接交付引擎输出。
    if !cards.iter().any(|card| card.has_cover) {
        return Ok(png);
    }

    let base = image::load_from_memory(&png)
        .map_err(|error| RenderError::Png(error.to_string()))?
        .to_rgba8();
    if base.width() == 0 || base.height() == 0 {
        return Err(RenderError::Layout("渲染结果是 0×0 的位图".to_owned()));
    }

    // 只有「有封面」的卡片才画了洋红占位，所以检测到的槽位要按卡片下标对齐，
    // 不能直接按下标复用 covers（否则缺封面的卡片会让后面的封面整体错位）。
    let markable: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(_, card)| card.has_cover)
        .map(|(index, _)| index)
        .collect();
    let detected = compose::find_slots(&base);
    let targets: Vec<(usize, compose::Slot)> = if detected.len() == markable.len()
        && plausible_slots(&detected, &layout, options, scale)
    {
        markable.iter().copied().zip(detected).collect()
    } else {
        // 数量对不上、或检测到的块尺寸明显不是封面槽位（例如背景色恰好是洋红，
        // 整张画布都被当成一块）：退回解析式坐标，绝不 panic。
        markable
            .iter()
            .map(|&index| {
                (
                    index,
                    analytic_slot(&layout, options, (base.width(), base.height()), index),
                )
            })
            .collect()
    };

    let radius = template::CARD_RADIUS as f32 * scale;
    let composed = compose::compose(&base, &targets, covers, radius)?;
    compose::encode_png(&composed)
}

/// 把游戏列表整理成卡片描述（截断到 `max_cards`，与 `covers` 下标对齐）。
fn plan<'a>(
    games: &'a FreeGames,
    covers: &[Option<Vec<u8>>],
    options: &RenderOptions,
) -> Vec<Card<'a>> {
    let limit = options.max_cards.max(1);
    games
        .iter()
        .take(limit)
        .enumerate()
        .map(|(index, game)| Card {
            game,
            upcoming: index >= games.current.len(),
            has_cover: covers.get(index).and_then(Option::as_ref).is_some(),
        })
        .collect()
}

/// 检测到的槽位是否可信：每一块都得接近配置的封面尺寸（±25%）。
///
/// 主要防一种边角情况——背景色本身是洋红时，整张画布会被当成一块「槽位」，
/// 只按数量校验是拦不住的。
fn plausible_slots(
    slots: &[compose::Slot],
    layout: &Layout,
    options: &RenderOptions,
    scale: f32,
) -> bool {
    if slots.is_empty() || layout.cols == 0 {
        return false;
    }
    let expected_width = options.card_width as f32 * scale;
    let expected_height = options.cover_height as f32 * scale;
    if expected_width <= 0.0 || expected_height <= 0.0 {
        return false;
    }

    slots.iter().all(|slot| {
        let width_error = (slot.width as f32 - expected_width).abs() / expected_width;
        let height_error = (slot.height as f32 - expected_height).abs() / expected_height;
        width_error <= 0.25 && height_error <= 0.25
    })
}

/// 槽位检测失败时的兜底坐标（设备像素），按卡片下标算。
///
/// 用模板的解析式布局算，再用实际位图 / CSS 尺寸的比例修正——引擎取整或边框
/// 带来的整体偏移都能被这部分吸收。
fn analytic_slot(
    layout: &Layout,
    options: &RenderOptions,
    dims: (u32, u32),
    index: usize,
) -> compose::Slot {
    let (image_width, image_height) = dims;
    let ratio_x = if layout.width == 0 {
        1.0
    } else {
        image_width as f32 / layout.width as f32
    };
    let ratio_y = if layout.height == 0 {
        1.0
    } else {
        image_height as f32 / layout.height as f32
    };

    let rect = layout.slot_rect(index, options);
    compose::Slot {
        x: (rect.x * ratio_x).round().max(0.0) as u32,
        y: (rect.y * ratio_y).round().max(0.0) as u32,
        width: ((rect.width * ratio_x).round().max(0.0) as u32).min(image_width),
        height: ((rect.height * ratio_y).round().max(0.0) as u32).min(image_height),
    }
}

/// 缩放的合法区间：引擎拒绝 0 / 负数 / NaN，配置层也给不出极端值。
fn normalized_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.5, 4.0)
    } else {
        1.0
    }
}
