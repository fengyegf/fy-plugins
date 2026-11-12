//! 网站图标 → PNG data URI。
//!
//! html2img 的渲染器支持 `<img src="data:…">`，所以 favicon 不用再拼内联 SVG 的
//! `<image href>`：`core::favicon` 已经把图标解成 RGBA8 并缩到 ≤64px，这里只做
//! 「PNG 编码 + base64」，产出的字符串直接进模板的 `icon` 字段。
//!
//! 图标很小（64px 见方、几 KB），base64 之后仍在 html2img 的 128 KiB 单页上限内；
//! 编码失败返回 `None`，模板会回退到内置的渐变地球图标。

use base64::Engine as _;

use crate::core::favicon::Icon;

/// 把图标编码成 `data:image/png;base64,…`；编码失败返回 `None`。
pub fn data_uri(icon: &Icon) -> Option<String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(encode_png(icon)?);
    Some(format!("data:image/png;base64,{encoded}"))
}

/// 把 RGBA8 像素编码成 PNG。
fn encode_png(icon: &Icon) -> Option<Vec<u8>> {
    use image::ImageEncoder as _;

    // 像素缓冲区与尺寸不一致时 image 会 panic；这里先拦住，调用方回退到内置图标。
    let expected = (icon.width as usize)
        .checked_mul(icon.height as usize)?
        .checked_mul(4)?;
    if icon.rgba.len() != expected {
        return None;
    }

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
