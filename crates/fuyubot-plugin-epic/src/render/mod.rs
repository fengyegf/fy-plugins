//! 卡片渲染。
//!
//! 管线分两段：
//! 1. [`template`] 生成 HTML/CSS，交给 hyper-render 渲染出布局与文字（占位色块标出封面槽位）；
//! 2. [`compose`] 找到槽位矩形，把下载好的封面裁切、贴合并按圆角遮罩合成。
//!
//! 之所以要第二段：hyper-render 0.2 不加载任何图片（`data:` URI 与 http 都不行），
//! 布局里只能留占位，封面必须由我们自己解码合成。

pub mod card;
pub mod compose;
pub mod template;

use crate::core::epic::FreeGames;

/// 渲染参数（由配置转换而来）。
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
    #[error("渲染引擎失败: {0}")]
    Engine(String),
    #[error("布局解析失败: {0}")]
    Layout(String),
    #[error("封面处理失败: {0}")]
    Cover(String),
    #[error("PNG 编解码失败: {0}")]
    Png(String),
}

/// 渲染一张卡片图（PNG）。
///
/// `covers` 与 `games.iter()` 顺序一一对应：`Some(bytes)` 是封面的原始编码字节
/// （jpg / png / webp），`None` 表示这张不贴图（保留占位色块）。
pub fn render(
    games: &FreeGames,
    covers: &[Option<Vec<u8>>],
    options: &RenderOptions,
) -> Result<Vec<u8>, RenderError> {
    card::render(games, covers, options)
}
