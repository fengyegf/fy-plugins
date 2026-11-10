//! 图像生成：把 [`CardData`] 渲染成一张 Whois 信息卡片（PNG）。
//!
//! 渲染链路为「HTML + CSS → Blitz 布局 → Vello CPU 光栅化」，不依赖浏览器或 Node。
//!
//! 入口是 [`render_card`]：入参是 [`CardData`]、宽度与可选图标，出参是 PNG 字节。
//!
//! 实现上需要遵守三件事：
//!
//! 1. 渲染**串行化**——引擎内部有全局字体集合与缓存，插件又会在多条业务线程里
//!    并发出图，串行锁最简单也最稳；
//! 2. 渲之前先按 [`template::estimate_height`] 估算高度，超过 [`MAX_HEIGHT`] 直接
//!    报错——光栅化位图是「宽 × 高 × 4 字节」，等渲完再拦就太晚了；
//! 3. 只要 PNG 字节，不落盘（临时文件由命令层负责）。

mod assets;
mod template;

use std::sync::{Mutex, OnceLock};

use crate::core::favicon::Icon;
use crate::core::model::CardData;

/// 渲染串行锁。
///
/// 引擎内部有全局字体集合与缓存，插件会在多条业务线程里并发出图。串行化可以
/// 避免共享渲染状态竞争，相比 Whois 接口耗时，串行不构成瓶颈。
fn render_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 输出高度上限：极端数据（比如接口返回上百条域名状态）也不至于渲出一张
/// 几十米高、几百 MB 的位图。
pub const MAX_HEIGHT: u32 = 20000;

/// 卡片宽度允许的范围，与 `core::config` 的夹取范围一致。
///
/// 下限 900 不是拍脑袋：两列信息框每列只剩 418px，值列约 266px，再窄 `status`
/// 文本就会一两行地折；上限 1600：Telegram 再大也没必要。
pub const MIN_WIDTH: u32 = 900;
pub const MAX_WIDTH: u32 = 1600;

/// 引擎要求视口宽高至少 16px（`Config::validate`），这里取它允许的最小值。
const MIN_VIEWPORT_HEIGHT: u32 = 16;

/// 渲染失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 布局 / 光栅化失败。
    #[error("渲染引擎报错: {0}")]
    Engine(String),
    /// 内容太高，渲染结果超过 [`MAX_HEIGHT`]。
    #[error("卡片高度超过 {0}px")]
    TooTall(u32),
}

/// 把宽度夹到 `[MIN_WIDTH, MAX_WIDTH]`。
pub fn clamp_width(width: u32) -> u32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// 把一张卡片渲染成 PNG 字节；`width` 是卡片宽度（px，会夹到 900–1600）。
///
/// `icon` 是抓到的网站图标：`None` 时头部回退到内置的渐变地球图标。
pub fn render_card(card: &CardData, width: u32, icon: Option<&Icon>) -> Result<Vec<u8>, Error> {
    let width = clamp_width(width);

    // 先估算再渲染：位图大小是「宽 × 高 × 4 字节」，等渲完再从 PNG 头里读高度，
    // 那张位图可能已经占了几百 MB。估算偏保守，只会误伤极端数据。
    if template::estimate_height(card, width) > MAX_HEIGHT {
        return Err(Error::TooTall(MAX_HEIGHT));
    }

    let html = template::build(card, width, icon);
    render_html(&html, width)
}

/// 渲染任意 HTML。`width` 是视口宽度（px）。
fn render_html(html: &str, width: u32) -> Result<Vec<u8>, Error> {
    let width = width.clamp(MIN_VIEWPORT_HEIGHT, MAX_WIDTH);
    let _guard = render_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // 初始视口高度只影响百分比高度，这里给引擎允许的最小值；
    // 真正的输出高度由 `auto_height` 从 html 根节点的布局里取。
    let config = hyper_render::Config::new()
        .width(width)
        .height(MIN_VIEWPORT_HEIGHT)
        .auto_height(true)
        .background([255, 255, 255, 255]);

    let png = hyper_render::render_to_png(html, config)
        .map_err(|error| Error::Engine(error.to_string()))?;

    // 兜底：估算有误差时，实际高度仍可能越过上限。
    if let Some((_, height)) = png_size(&png)
        && height > MAX_HEIGHT
    {
        return Err(Error::TooTall(MAX_HEIGHT));
    }
    Ok(png)
}

/// 从 PNG 字节里读 `(宽, 高)`：IHDR 紧跟在 8 字节签名 + 4 字节长度 + 4 字节类型之后，
/// 宽高是偏移 16 起的两个大端 u32。读不到就返回 `None`。
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const WIDTH_OFFSET: usize = 16;
    let width = u32::from_be_bytes(png.get(WIDTH_OFFSET..WIDTH_OFFSET + 4)?.try_into().ok()?);
    let height = u32::from_be_bytes(
        png.get(WIDTH_OFFSET + 4..WIDTH_OFFSET + 8)?
            .try_into()
            .ok()?,
    );
    Some((width, height))
}
