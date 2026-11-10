//! 卡片用到的图形素材：内联 SVG 图标。
//!
//! hyper-render 0.2.2 **不加载 `<img>`**（外部与 data URI 都不认），图标只能走
//! 内联 `<svg>`。动手前用一次性探针验证过：Blitz + usvg 对 `fill` / `stroke`、
//! `stroke-width`、`linearGradient` 都能正确光栅化，所以这里全部用 Lucide 风格的
//! 卡片使用的内联线框图标。
//!
//! 网站图标走的是另一条路：内联 `<svg>` 里的 `<image href="data:image/png;base64,...">`。
//! 这条也验证过（`anyrender_svg` 的 `image_format_png` 是默认特性，usvg 的默认
//! data URI 解析器认 PNG）：`<img>` 不行，但 SVG 里的 `<image>` 可以。
//!
//! 颜色直接写死在下方的 `stroke` 里（`currentColor` 在 Blitz 里不可靠）：
//! 头部地球为白色，分组图标按区域使用蓝、紫、深灰描边。

use base64::Engine as _;

use crate::core::favicon::Icon;

/// 头部 64×64 渐变方块里的白色地球图标。
pub const GLOBE_HEADER: &str = r##"<svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="flex-shrink:0;"><circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><line x1="2" y1="12" x2="22" y2="12"/></svg>"##;

/// 网站图标的内联 SVG：`size × size` 的 `<image>`，等比缩放居中。
///
/// 编码失败返回 `None`，调用方回退到 [`GLOBE_HEADER`] + 渐变底色。
pub fn favicon(icon: &Icon, size: u32) -> Option<String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(encode_png(icon)?);
    Some(format!(
        r##"<svg width="{size}" height="{size}" viewBox="0 0 {size} {size}" xmlns="http://www.w3.org/2000/svg"><image href="data:image/png;base64,{encoded}" x="0" y="0" width="{size}" height="{size}" preserveAspectRatio="xMidYMid meet"/></svg>"##
    ))
}

/// 把 RGBA8 像素编码成 PNG。
fn encode_png(icon: &Icon) -> Option<Vec<u8>> {
    use image::ImageEncoder as _;

    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            &icon.rgba,
            icon.width,
            icon.height,
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(bytes)
}

/// 「域名信息」标题图标：蓝色地球（20×20）。
pub const GLOBE_BLUE: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#3b82f6" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="flex-shrink:0;"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>"##;

/// 「服务器信息」标题图标：蓝色服务器（20×20）。
pub const SERVER_BLUE: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#3b82f6" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="flex-shrink:0;"><rect width="20" height="8" x="2" y="2" rx="2" ry="2"/><rect width="20" height="8" x="2" y="14" rx="2" ry="2"/><line x1="6" x2="6.01" y1="6" y2="6"/><line x1="6" x2="6.01" y1="18" y2="18"/></svg>"##;

/// 「其它信息」标题图标：深灰信息圈（20×20）。
pub const INFO_DARK: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#111827" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="flex-shrink:0;"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>"##;

/// 「注册人信息」标题图标：紫色用户（20×20）。
pub const USER_PURPLE: &str = r##"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#a855f7" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="flex-shrink:0;"><path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>"##;

/// 头部圆角方块的渐变底色。
///
/// 用一次性探针验证过：Blitz 能直接画 CSS 渐变，不需要退回「分层纯色」，
/// 也就不用内联 SVG 的 `linearGradient` 方案。`135deg` 与 `to bottom right` 在方形上等价。
pub const HEADER_GRADIENT: &str = "linear-gradient(135deg,#3b82f6,#9333ea)";
