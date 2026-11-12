//! 图像生成：Vue 卡片模板 + 数据 → fuyubot.html2img 服务 → 本机图片文件。
//!
//! **界面不在 Rust 里拼**：整张卡片的 HTML / CSS / 循环 / 条件都在
//! [assets/card.html](../../assets/card.html) 里（style + Vue 模板），
//! 本模块只做四件事：
//!
//! 1. 把游戏列表整理成 Vue 模板的 data（见 [data]）；
//! 2. 算画布宽度与倍率预算（高度交给服务的 auto_height）；
//! 3. 调宿主上的 html2img 服务（vue: true），拿本机图片路径；
//! 4. 把错误翻译成插件层能展示的中文。
//!
//! 渲染器只有宿主那一份（hyper-render 的本地 fork：Blitz + Stylo + Taffy +
//! Vello CPU），插件不再链接 hyper-render，也不再自己拼 HTML 字符串。
//!
//! html2img 是**硬依赖**（Plugin::inject）：没有它出不了图，插件停在 Pending
//! 比启动后每条命令都失败更容易排查。

pub mod data;
pub mod html2img;

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::Value;

use crate::core::epic::FreeGames;

/// 卡片模板：style + Vue 模板，编译期嵌入。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 输出高度上限，与 html2img 服务配置里的 max_height 默认值（6000）对齐。
pub const MAX_HEIGHT: u32 = 6000;
/// 输出总像素上限，与 html2img 的 max_pixels 默认值（16M）对齐。
pub const MAX_PIXELS: f64 = 16_000_000.0;
/// 输出倍率上限，与 html2img 的 max_scale 默认值（3.0）对齐。
pub const MAX_SCALE: f32 = 3.0;
/// 输出倍率下限：再小就看不清了，宁可不出图。
pub const MIN_SCALE: f32 = 0.5;
/// 单次 HTML 上限，与 html2img 的默认值（128 KiB）对齐。
pub const MAX_HTML_BYTES: usize = 128 * 1024;
/// 交给服务的视口高度：auto_height 时真实高度由内容撑开，这里只给一个比卡片
/// 最小高度还小的值，避免内容不足时被留白撑高。
pub const VIEWPORT_HEIGHT: u32 = 100;

/// 徽章条高度：py-1（上下各 4px）+ text-sm 行高 20px。
pub const BADGE_HEIGHT: u32 = 28;
/// 卡片（封面 + 徽章）与标题的间距（模板里的 margin-top:12px）。
pub const COVER_TITLE_GAP: u32 = 12;
/// 标题区高度：两行 × text-sm 行高 20px。
pub const TITLE_HEIGHT: u32 = 40;
/// 日期行高度（text-sm 行高 20px）。
pub const DATE_HEIGHT: u32 = 20;
/// 页头高度（模板里显式写死 56px）。
pub const HEADER_HEIGHT: u32 = 56;
/// 页头与卡片之间的间距（模板里的 margin-bottom:16px）。
pub const HEADER_GAP: u32 = 16;
/// 空列表提示区高度（模板里的 height:100px）。
pub const EMPTY_BODY_HEIGHT: u32 = 100;

/// 渲染参数（由配置转换而来，也决定模板拿到的版式数据）。
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// 输出缩放倍数（CSS 像素 → 位图）。
    pub scale: f32,
    /// 卡片宽度（CSS 像素）。
    pub card_width: u32,
    /// 封面高度（CSS 像素）。
    pub cover_height: u32,
    /// 卡片间距（CSS 像素）。
    pub gap: u32,
    /// 画布内边距（CSS 像素）。
    pub padding: u32,
    /// 每行卡片数。
    pub max_per_row: u32,
    /// 一张图最多展示多少款游戏。
    pub max_cards: usize,
    /// 画布背景色（RGBA）。
    pub background: [u8; 4],
    /// 页头标题。
    pub title: String,
    /// 没有任何免费游戏时的提示文案。
    pub empty_text: String,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 2.0,
            card_width: 280,
            cover_height: 157,
            gap: 20,
            padding: 24,
            max_per_row: 4,
            max_cards: 8,
            background: [32, 32, 36, 255],
            title: "免费游戏".to_owned(),
            empty_text: "当前没有免费游戏".to_owned(),
        }
    }
}

/// 渲染层的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// 渲染服务调用失败（服务不在 / 模板编译失败 / 渲染报错 / 应答不合法）。
    #[error("渲染服务失败: {0}")]
    Service(#[from] html2img::Error),
    /// 估算的画布高度超过服务上限。
    #[error("画布高度约 {0}px，超过上限 6000px")]
    TooTall(u32),
    /// 画布 × 倍率超过服务的像素上限。
    #[error("画布 {width}×{height} 在 {scale:.2}x 下超过 16M 像素上限")]
    TooLarge { width: u32, height: u32, scale: f32 },
    /// 模板本身超过服务上限（编译期就该发现，留作兜底）。
    #[error("卡片模板 {0} 字节，超过 128 KiB 上限")]
    TemplateTooLarge(usize),
}

/// 画布几何（CSS 像素）。
///
/// 宽度是**固定**的（按列数算），高度只是估算：真实高度由 html2img 的
/// auto_height 按内容撑开，估算值用于像素预算与高度上限校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// 每行卡片数（空列表时为 1）。
    pub cols: usize,
    /// 行数（空列表时为 0）。
    pub rows: usize,
    /// 画布宽度（CSS 像素）。
    pub width: u32,
    /// 画布高度估算（CSS 像素）。
    pub height: u32,
    /// 整列高度：卡片 + 标题 + 日期。
    pub column_height: u32,
}

/// 计算画布几何。
///
/// 宽 = padding*2 + cols*card_width + (cols-1)*gap
/// 高约 = padding*2 + 页头 + 页头间距 + rows*列高 + (rows-1)*gap
/// 列高 = cover_height + 28(徽章) + 12 + 40(标题两行) + 20(日期)
pub fn layout(card_count: usize, options: &RenderOptions) -> Layout {
    let padding = options.padding;
    let card_width = options.card_width.max(1);
    let cover_height = options.cover_height.max(1);
    let gap = options.gap;
    let column_height = cover_height
        .saturating_add(BADGE_HEIGHT)
        .saturating_add(COVER_TITLE_GAP)
        .saturating_add(TITLE_HEIGHT)
        .saturating_add(DATE_HEIGHT);

    if card_count == 0 {
        return Layout {
            cols: 1,
            rows: 0,
            width: padding.saturating_mul(2).saturating_add(card_width),
            height: padding
                .saturating_mul(2)
                .saturating_add(HEADER_HEIGHT)
                .saturating_add(HEADER_GAP)
                .saturating_add(EMPTY_BODY_HEIGHT),
            column_height,
        };
    }

    let cols = card_count.min(options.max_per_row.max(1) as usize).max(1);
    let rows = card_count.div_ceil(cols);
    let width = padding
        .saturating_mul(2)
        .saturating_add((cols as u32).saturating_mul(card_width))
        .saturating_add((cols as u32 - 1).saturating_mul(gap));
    let height = padding
        .saturating_mul(2)
        .saturating_add(HEADER_HEIGHT)
        .saturating_add(HEADER_GAP)
        .saturating_add((rows as u32).saturating_mul(column_height))
        .saturating_add((rows as u32 - 1).saturating_mul(gap));

    Layout {
        cols,
        rows,
        width,
        height,
        column_height,
    }
}

/// 一次渲染的完整输入：模板数据 + 画布宽度 + 倍率。
#[derive(Debug)]
pub struct Prepared {
    /// 交给 Vue 模板的数据。
    pub data: Value,
    /// 画布宽度（CSS 像素）。
    pub width: u32,
    /// 实际交给服务的倍率。
    pub scale: f32,
}

/// 规划 + 校验 + 生成模板数据（不调服务），供单测 / 验收 / 样张使用。
///
/// 与 [render_card] 走同一套规划与校验，所以这里能过就说明真实渲染时入参
/// 不会因为超限被服务拒绝。
pub fn prepare(games: &FreeGames, options: &RenderOptions) -> Result<Prepared, RenderError> {
    if TEMPLATE.len() > MAX_HTML_BYTES {
        return Err(RenderError::TemplateTooLarge(TEMPLATE.len()));
    }
    let plan = data::plan(games, options);
    let layout = layout(plan.len(), options);

    // 先按服务上限校验再发请求：失败原因写清楚，命令层才好降级成文本清单。
    if layout.height > MAX_HEIGHT {
        return Err(RenderError::TooTall(layout.height));
    }
    let scale = effective_scale(layout.width, layout.height, options.scale)?;

    Ok(Prepared {
        data: data::view_data(&plan, &layout, options),
        width: layout.width,
        scale,
    })
}

/// 拼给 html2img 的完整入参（html / vue / data / width / …）。
///
/// 抽成公开函数方便验收用例 / 样张直接复用，不必真的调用服务。
pub fn payload(games: &FreeGames, options: &RenderOptions) -> Result<Value, RenderError> {
    let prepared = prepare(games, options)?;
    Ok(request(TEMPLATE, &prepared.data, prepared.width, prepared.scale).payload())
}

/// 拼一次渲染请求：Vue 模板 + 数据、固定宽度、JPEG、高度自适应。
pub fn request<'a>(
    html: &'a str,
    data: &'a Value,
    width: u32,
    scale: f32,
) -> html2img::Request<'a> {
    html2img::Request {
        html,
        // 卡片是 Vue 模板：循环 / 条件 / 插值都在模板里，Rust 只给数据。
        vue: true,
        data: Some(data),
        width,
        // 短内容不被默认视口留白撑高；真实高度由 auto_height 撑开。
        height: Some(VIEWPORT_HEIGHT),
        scale,
        // 卡片是深色版式，JPEG 体积小、Telegram 照片上传够用。
        format: "jpg",
        auto_height: true,
        transparent: false,
        dark: true,
    }
}

/// 渲染一张卡片图（JPEG），返回 html2img 产出的本机文件路径。
///
/// 调用方负责在用完之后删掉 path（html2img 自己会在 24 小时后清理）。
pub fn render_card(
    host: &HostApi,
    games: &FreeGames,
    options: &RenderOptions,
) -> Result<PathBuf, RenderError> {
    let prepared = prepare(games, options)?;
    let request = request(TEMPLATE, &prepared.data, prepared.width, prepared.scale);
    Ok(html2img::render(host, &request)?)
}

/// 实际交给服务的倍率：先夹到 [MIN_SCALE, MAX_SCALE]，再按像素上限下调。
///
/// 单张图默认配置（4 列 2 行）约 1228×654 CSS px，2 倍时 3.2M 像素，远低于
/// 16M；但把 max_per_row / max_cards 调到上限后会超，这里自动降倍率而不是
/// 直接失败——宁可图小一点，也别让用户什么都收不到。
pub fn effective_scale(width: u32, height: u32, requested: f32) -> Result<f32, RenderError> {
    let requested = if requested.is_finite() {
        requested.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        1.0
    };
    let pixels = f64::from(width.max(1)) * f64::from(height.max(1));
    let budget = (MAX_PIXELS / pixels).sqrt();
    if budget < f64::from(MIN_SCALE) {
        return Err(RenderError::TooLarge {
            width,
            height,
            scale: requested,
        });
    }
    let scale = requested.min(budget as f32);
    // 保留两位小数：别把 1.9999999 这类浮点噪声写进请求。
    Ok((scale * 100.0).floor() / 100.0)
}
