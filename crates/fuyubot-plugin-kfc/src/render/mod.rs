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

/// 卡片模板要的数据：正文 + 卡片宽度（宽度由模板拼进 `style`）。
pub fn view_data(content: &str, width: u32) -> Value {
    json!({ "content": content, "width": width })
}
