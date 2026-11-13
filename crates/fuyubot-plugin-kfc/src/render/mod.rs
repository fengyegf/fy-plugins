//! 图像生成：Vue 卡片模板 + 数据 → `fuyubot.html2img` 服务 → 本机图片文件。
//!
//! **界面不在 Rust 里拼**：整张卡片的 HTML / CSS / Vue 指令都在
//! [`assets/card.html`](../../assets/card.html) 里（`<style>` + Vue 模板），
//! 本模块只做三件事：
//!
//! 1. 把业务文案和宽度凑成 Vue 模板的 `data`（[JSON](serde_json::Value)）；
//! 2. 调宿主上的 html2img 服务（`vue: true`），拿本机图片路径；
//! 3. 把错误翻译成插件层能展示的中文。
//!
//! 渲染器只有宿主那一份（hyper-render 的本地 fork：Blitz + Stylo + Taffy + Vello CPU），
//! 插件不再链接 hyper-render，也不再自己拼 HTML 字符串。
//!
//! html2img 是**硬依赖**（`Plugin::inject`）：没有它出不了图，插件停在 Pending
//! 比启动后每条命令都失败更容易排查。

pub mod html2img;

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

/// 卡片模板：`<style>` + Vue 模板，编译期嵌入。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 卡片宽度允许的范围。
///
/// 下限 460 不是拍脑袋：再窄内容列就放不下「头像 + KFC + 蓝标 + @句柄」，
/// 句柄会被迫在「刚/刚」这种地方断行，用户名行进入换行兜底后卡片底部
/// 还会多出一条空白带（实测 400–456 有 96–139px 空白）。上限 1600：
/// 再宽就不像推特卡片了。
const MIN_WIDTH: u32 = 460;
const MAX_WIDTH: u32 = 1600;

/// 交给 html2img 的视口高度下限：`auto_height` 时真实高度由内容撑开，
/// 这里给一个比卡片最小高度还小的值，避免短文案被留白撑高。
const VIEWPORT_HEIGHT: u32 = 100;

/// 单次 HTML 上限，与 html2img 默认的 `max_html_bytes`（128 KiB）对齐。
pub const MAX_HTML_BYTES: usize = 128 * 1024;

/// 渲染失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 渲染服务调用失败（服务不在 / 模板编译失败 / 渲染报错 / 应答不合法）。
    #[error("渲染服务失败: {0}")]
    Service(#[from] html2img::Error),
    /// 模板本身超过服务上限（编译期就该发现，留作兜底）。
    #[error("卡片模板 {0} 字节，超过 128 KiB 上限")]
    TemplateTooLarge(usize),
}

/// 把文案渲染成卡片图，返回 html2img 产出的本机文件路径；`width` 是卡片宽度（px）。
///
/// 调用方负责在用完之后删掉 `path`（html2img 自己会在 24 小时后清理）。
pub fn render_card(host: &HostApi, content: &str, width: u32) -> Result<PathBuf, Error> {
    if TEMPLATE.len() > MAX_HTML_BYTES {
        return Err(Error::TemplateTooLarge(TEMPLATE.len()));
    }
    let width = clamp_width(width);
    let data = view_data(content, width);
    let request = html2img::Request {
        html: TEMPLATE,
        // 卡片是 Vue 模板：循环 / 条件 / 插值都在模板里，Rust 只给数据。
        vue: true,
        data: Some(&data),
        width,
        // 短文案不被默认的 768px 视口留白撑高；长文案由 auto_height 撑开。
        height: Some(VIEWPORT_HEIGHT),
        // 与老版本地渲染一致：800 CSS px 宽、1:1 输出。
        scale: 1.0,
        // 白底卡片，JPEG 体积小、Telegram 照片上传够用。
        format: "jpg",
        auto_height: true,
        transparent: false,
        dark: false,
    };
    Ok(html2img::render(host, &request)?)
}

/// 把宽度夹到 `[MIN_WIDTH, MAX_WIDTH]`。
pub fn clamp_width(width: u32) -> u32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// 卡片内边距（模板里的 `padding:20px`）。
pub const CARD_PADDING: u32 = 20;
/// 卡片边框宽度（模板里的 `border:1px`）。
pub const CARD_BORDER: u32 = 1;
/// 头像边长（模板里的 `width/height:100px`）。
pub const AVATAR_SIZE: u32 = 100;
/// 头像右侧间距（模板里的 `margin-right:15px`）。
pub const AVATAR_GAP: u32 = 15;
/// 内容列右侧留白（模板里的 `margin-right:30px`）。
pub const CONTENT_MARGIN: u32 = 30;
/// 正文宽度占内容列的比例：模板原来是 `width:95%`。
const TEXT_WIDTH_NUM: u32 = 95;
const TEXT_WIDTH_DEN: u32 = 100;

/// 内容列（头像右边那一列）的宽度，单位 CSS 像素。
///
/// **必须算成确定值交给模板**：html2img 的渲染器不会按 `flex-grow` 把内容列
/// 收窄进卡片，而是取 max-content 宽度——正文和图标行会直接顶出卡片右侧
/// （线上表现：正文第一行被卡片右边缘切掉、五个互动图标只剩第一个可见）。
/// 这里把宽度算好，模板只读字段，不再依赖 flex 收缩。
pub fn content_width(width: u32) -> u32 {
    width.saturating_sub(
        2 * (CARD_BORDER + CARD_PADDING) + AVATAR_SIZE + AVATAR_GAP + CONTENT_MARGIN,
    )
}

/// 正文宽度：内容列的 95%，也就是 `white-space:pre-wrap` 的换行宽度。
pub fn text_width(width: u32) -> u32 {
    content_width(width) * TEXT_WIDTH_NUM / TEXT_WIDTH_DEN
}

/// 卡片模板要的数据：正文 + 卡片宽度 + 内容列 / 正文的确定宽度（模板拼进 `style`）。
pub fn view_data(content: &str, width: u32) -> Value {
    json!({
        "content": content,
        "width": width,
        "contentWidth": content_width(width),
        "textWidth": text_width(width),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_column_leaves_room_for_avatar_and_padding() {
        // 800 宽：800 - 2*(1+20) - 100 - 15 - 30 = 613，与 TS 版实测的内容列一致。
        assert_eq!(content_width(800), 613);
        assert_eq!(text_width(800), 582);
    }

    #[test]
    fn narrow_cards_do_not_underflow() {
        // 最窄 460：460 - 42 - 145 = 273，正文 259。
        assert_eq!(content_width(MIN_WIDTH), 273);
        assert_eq!(text_width(MIN_WIDTH), 259);
        // 比装饰宽度还窄时退化成 0，而不是下溢成天文数字。
        assert_eq!(content_width(0), 0);
        assert_eq!(text_width(0), 0);
    }

    #[test]
    fn view_data_exposes_definite_widths() {
        let data = view_data("文案", 800);
        assert_eq!(data["content"], "文案");
        assert_eq!(data["width"], 800);
        assert_eq!(data["contentWidth"], 613);
        assert_eq!(data["textWidth"], 582);
    }
}
