//! 网站图标（favicon）：给卡片左上角的图标方块取一张真实站点图标。
//!
//! 顺序（拿到可解码的位图就停，全部失败返回 `None`）：
//!
//! 1. `https://<domain>/favicon.ico`
//! 2. `https://www.<domain>/favicon.ico`（域名本身不以 `www.` 开头时）
//! 3. 抓首页 HTML，找 `<link rel="...icon...">` 的 `href`，再取那个地址
//! 4. `http://<domain>/favicon.ico`（有的小站没有 HTTPS，放最后免得白等）
//!
//! 整个过程受 `favicon_timeout`（缺省 6 秒）约束：站点连不上时不能把 `/whois`
//! 拖到几十秒。任何一步失败都只是「没有图标」，渲染层会回退到内置的渐变地球图标。
//! IP 字面量与 `localhost` 这类主机名直接跳过，避免用户借 `/whois` 探测内网。
//!
//! 解码交给 `image` crate（ICO / PNG / GIF / WebP），拿到的是 RGBA8 像素；
//! 长边超过 [`MAX_SIDE`] 的先缩小——卡片上只画 64px，留着原图只会让 data URI 变大。
//! SVG 图标直接跳过：`image` 解不了，硬塞给渲染层风险更大。

use std::io::{Cursor, Read};
use std::time::{Duration, Instant};

use crate::core::Config;

/// 单次响应体上限：图标通常几 KB，512 KiB 足够，也挡住异常响应。
const MAX_BYTES: u64 = 512 * 1024;
/// 图标长边上限（px）。卡片上只画 64px，128 已经留了余量。
const MAX_SIDE: u32 = 128;
/// 原始图片长边上限：真实图标最大也就 512px，更大的当作解压炸弹丢掉。
const MAX_SOURCE_SIDE: u32 = 1024;
/// 图标边长下限：1×1 的跟踪像素画出来只是个色块，当作没有图标。
const MIN_SIDE: u32 = 8;
/// 连接超时：DNS 卡住时不要等满整个预算。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 剩余预算低于这个值就不再发起新请求。
const MIN_BUDGET: Duration = Duration::from_millis(300);

const USER_AGENT: &str = "fuyubot-whois/0.1 (+rust)";

/// 解码后的网站图标：RGBA8 像素，长边不超过 [`MAX_SIDE`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    /// 长度为 `width * height * 4`，逐行排列。
    pub rgba: Vec<u8>,
}

/// 抓取 `domain` 的网站图标；拿不到返回 `None`（不报错，调用方回退默认图标）。
pub fn fetch(config: &Config, domain: &str) -> Option<Icon> {
    if !config.favicon || !is_public_host(domain) {
        return None;
    }
    let deadline = Instant::now() + config.favicon_timeout;

    if let Some(icon) = fetch_file(deadline, &format!("https://{domain}/favicon.ico")) {
        return Some(icon);
    }
    if !domain.starts_with("www.")
        && let Some(icon) = fetch_file(deadline, &format!("https://www.{domain}/favicon.ico"))
    {
        return Some(icon);
    }
    // 很多站点不把 favicon.ico 放在根目录，而是在首页里声明。
    if let Some(icon) = fetch_declared(deadline, domain) {
        return Some(icon);
    }
    // 最后才试明文 HTTP：有的小站没有证书，放在前面会白白多花时间。
    fetch_file(deadline, &format!("http://{domain}/favicon.ico"))
}

/// 只抓看起来是公网域名的目标：跳过 IP 字面量与单标签主机名（`localhost` 之类），
/// 免得 `/whois` 被用户拿来探测内网。
fn is_public_host(domain: &str) -> bool {
    domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with(".local")
        && domain.parse::<std::net::IpAddr>().is_err()
}

/// 直接抓一个固定地址的图标。
fn fetch_file(deadline: Instant, url: &str) -> Option<Icon> {
    decode(&get(deadline, url, "image/*,*/*;q=0.8")?)
}

/// 抓首页 HTML，按 `<link rel="icon">` 声明的地址再抓一次。
fn fetch_declared(deadline: Instant, domain: &str) -> Option<Icon> {
    let home = format!("https://{domain}/");
    let html = get(deadline, &home, "text/html,application/xhtml+xml,*/*;q=0.8")?;
    let href = icon_href(&String::from_utf8_lossy(&html))?;
    let url = resolve(&home, &href)?;
    decode(&get(deadline, &url, "image/*,*/*;q=0.8")?)
}

/// 发一次 GET，返回响应体；超预算、非 2xx、读不出来都返回 `None`。
fn get(deadline: Instant, url: &str, accept: &str) -> Option<Vec<u8>> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining < MIN_BUDGET {
        return None;
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT.min(remaining))
        .timeout(remaining)
        .build();

    let response = agent
        .get(url)
        .set("Accept", accept)
        .set("User-Agent", USER_AGENT)
        .call()
        .ok()?;
    if !(200..300).contains(&response.status()) {
        return None;
    }

    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES)
        .read_to_end(&mut body)
        .ok()?;
    (!body.is_empty()).then_some(body)
}

/// 把图片字节解码成 RGBA8，并把长边缩到 [`MAX_SIDE`] 以内。
fn decode(bytes: &[u8]) -> Option<Icon> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    // 解压炸弹防线：几十 KB 的 PNG 可以解出上亿像素，先按尺寸和内存上限拦一道。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);

    let image = reader.decode().ok()?;

    let (width, height) = (image.width(), image.height());
    if width < MIN_SIDE || height < MIN_SIDE {
        return None;
    }

    let rgba = image.into_rgba8();
    let (width, height) = fit(width, height);
    let rgba = if (width, height) != rgba.dimensions() {
        image::imageops::resize(&rgba, width, height, image::imageops::FilterType::Lanczos3)
    } else {
        rgba
    };
    Some(Icon {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

/// 等比缩放到长边不超过 [`MAX_SIDE`]；本来就不大时原样返回。
fn fit(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= MAX_SIDE {
        return (width, height);
    }
    let scale = MAX_SIDE as f64 / longest as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// 从首页 HTML 里找图标地址。
///
/// 优先 `rel` 里带 `icon` 单词的（`icon` / `shortcut icon`），其次才是
/// `apple-touch-icon` 之类。`mask-icon`（单色 SVG）和 `type="image/svg+xml"`
/// 一律跳过：前者画出来是个黑色剪影，后者 `image` 解不了。
fn icon_href(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut index = 0;
    let mut fallback: Option<String> = None;

    while let Some(offset) = lower[index..].find("<link") {
        let start = index + offset;
        let Some(length) = lower[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + length];
        index = start + length + 1;

        let attrs = attributes(tag);
        let Some((_, rel)) = attrs.iter().find(|(name, _)| name == "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        if !rel.contains("icon") || rel.contains("mask") {
            continue;
        }
        let svg = attrs
            .iter()
            .any(|(name, value)| name == "type" && value.to_ascii_lowercase().contains("svg"));
        if svg {
            continue;
        }
        let Some((_, href)) = attrs.iter().find(|(name, _)| name == "href") else {
            continue;
        };
        let href = href.trim();
        let lower_href = href.to_ascii_lowercase();
        if href.is_empty()
            || lower_href.starts_with("data:")
            || lower_href.ends_with(".svg")
            || lower_href.contains(".svg?")
        {
            continue;
        }

        let is_plain_icon = rel.split_whitespace().any(|token| token == "icon");
        if is_plain_icon {
            return Some(href.to_owned());
        }
        fallback.get_or_insert_with(|| href.to_owned());
    }

    fallback
}

/// 解析一个标签里的属性：`<link rel="icon" href='/a.png'>` → `[("rel","icon"), ...]`。
///
/// 手写而不是上 HTML 解析器：只需要 `<link>` 这几个属性，引号有无、大小写、
/// 属性顺序都要容忍。
fn attributes(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let mut index = 0;

    // 跳过标签名（`link`）。
    while index < bytes.len()
        && !bytes[index].is_ascii_whitespace()
        && !matches!(bytes[index], b'>' | b'/')
    {
        index += 1;
    }

    let mut attrs = Vec::new();
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        let start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || matches!(bytes[index], b'-' | b'_' | b':' | b'.'))
        {
            index += 1;
        }
        if start == index {
            index += 1;
            continue;
        }
        let name = tag[start..index].to_ascii_lowercase();

        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            attrs.push((name, String::new()));
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }

        let value = if index < bytes.len() && (bytes[index] == b'"' || bytes[index] == b'\'') {
            let quote = bytes[index];
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index] != quote {
                index += 1;
            }
            let value = tag[start..index].to_owned();
            index += 1;
            value
        } else {
            let start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>'
            {
                index += 1;
            }
            tag[start..index].to_owned()
        };
        attrs.push((name, value));
    }
    attrs
}

/// 把首页里的 `href` 补成绝对地址；认不出来就返回 `None`。
fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_owned());
    }
    let origin = base.trim_end_matches('/');
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if let Some(rest) = href.strip_prefix('/') {
        return Some(format!("{origin}/{rest}"));
    }
    Some(format!("{origin}/{href}"))
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder as _;

    use super::*;

    #[test]
    fn attributes_handle_quotes_and_case() {
        let attrs = attributes(r##"<LINK REL="icon" HREF='/a.png' sizes=32x32>"##);
        assert_eq!(
            attrs,
            vec![
                ("rel".to_owned(), "icon".to_owned()),
                ("href".to_owned(), "/a.png".to_owned()),
                ("sizes".to_owned(), "32x32".to_owned()),
            ]
        );
    }

    #[test]
    fn icon_href_prefers_plain_icon() {
        let html = r##"<head><link rel="apple-touch-icon" href="/apple.png"><link rel="shortcut icon" href="/fav.ico"></head>"##;
        assert_eq!(icon_href(html).as_deref(), Some("/fav.ico"));
    }

    #[test]
    fn icon_href_falls_back_to_apple_touch() {
        let html = r##"<link rel="apple-touch-icon" href="/apple.png">"##;
        assert_eq!(icon_href(html).as_deref(), Some("/apple.png"));
    }

    #[test]
    fn icon_href_skips_svg_and_mask_icons() {
        let html = r##"<link rel="mask-icon" href="/mask.svg"><link rel="icon" type="image/svg+xml" href="/icon.svg"><link rel="icon" href="/real.png">"##;
        assert_eq!(icon_href(html).as_deref(), Some("/real.png"));
    }

    #[test]
    fn icon_href_none_without_icon_link() {
        assert_eq!(icon_href("<link rel=\"stylesheet\" href=\"/a.css\">"), None);
    }

    #[test]
    fn resolve_relative_urls() {
        assert_eq!(
            resolve("https://example.com/", "favicon.png").as_deref(),
            Some("https://example.com/favicon.png")
        );
        assert_eq!(
            resolve("https://example.com/", "/img/a.png").as_deref(),
            Some("https://example.com/img/a.png")
        );
        assert_eq!(
            resolve("https://example.com/", "//cdn.example.com/a.png").as_deref(),
            Some("https://cdn.example.com/a.png")
        );
        assert_eq!(
            resolve("https://example.com/", "http://a.com/b.png").as_deref(),
            Some("http://a.com/b.png")
        );
    }

    #[test]
    fn is_public_host_skips_local_targets() {
        assert!(is_public_host("example.com"));
        assert!(is_public_host("例子.中国"));
        assert!(!is_public_host("localhost"));
        assert!(!is_public_host("127.0.0.1"));
        assert!(!is_public_host("192.168.1.10"));
        assert!(!is_public_host("printer.local"));
        assert!(!is_public_host(""));
    }

    #[test]
    fn decode_scales_down_large_icons() {
        let source = image::RgbaImage::from_pixel(256, 256, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(source.as_raw(), 256, 256, image::ExtendedColorType::Rgba8)
            .expect("encode");
        let icon = decode(&bytes).expect("decode");
        assert_eq!((icon.width, icon.height), (MAX_SIDE, MAX_SIDE));
        assert_eq!(icon.rgba.len(), (MAX_SIDE * MAX_SIDE * 4) as usize);
    }

    #[test]
    fn decode_rejects_tiny_icons() {
        let source = image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(source.as_raw(), 1, 1, image::ExtendedColorType::Rgba8)
            .expect("encode");
        assert!(decode(&bytes).is_none());
    }

    #[test]
    fn decode_rejects_oversized_images() {
        let source = image::RgbaImage::from_pixel(2048, 2048, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(source.as_raw(), 2048, 2048, image::ExtendedColorType::Rgba8)
            .expect("encode");
        assert!(decode(&bytes).is_none());
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode(b"not an image").is_none());
    }
}
