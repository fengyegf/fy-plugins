//! 图像生成：把一段文案渲染成一张推特风格卡片（PNG）。
//!
//! 方案借鉴 <https://github.com/thomasmost/hyper-render>：不装浏览器、不用 Node，
//! 直接「HTML + CSS → Blitz 布局 → Vello CPU 光栅化」，纯 Rust 一条链路。
//! 与 TS 版 `satori + resvg` 管线的对应关系：
//!
//! | TS 版（satori + resvg） | 这里（hyper-render） |
//! | --- | --- |
//! | `kfc.vue` 模板 → `satoriVue` | [`template::build`] 生成 HTML |
//! | satori 的 flex 布局子集 | Blitz（Stylo + Taffy）完整 flex 布局 |
//! | resvg 光栅化 SVG | Vello CPU 光栅化 |
//! | `width: 800, height: "auto"` | `Config::width(800).auto_height(true)` |
//!
//! 两个必须知道的差异（都是 hyper-render 的现状，不是 bug）：
//!
//! - `<img>` 的外部 / data URI 图片不加载，所以头像不是 `<img>`，而是内联 `<svg>`
//!   里的 `<image href="data:image/jpeg;base64,…">`（见 [`assets::avatar_svg`]）；
//! - 只认系统字体（`@font-face` 未支持），中文走系统 CJK 字体回退，见
//!   [`template`] 里的字体链说明。

pub mod assets;
pub mod template;

#[cfg(test)]
mod samples;

use std::sync::{Mutex, OnceLock};

/// 渲染串行锁。
///
/// 渲染引擎内部有全局字体集合与缓存，插件又会在多条业务线程里并发出图；
/// 串行化是这里最简单也最稳妥的保证。实测 release：首张 ~60 ms（字体集合初始化），
/// 之后每张 ~15 ms，串行不构成瓶颈。
fn render_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 输出高度上限：极端长文本也不至于渲出一张几十米高、几百 MB 的图。
const MAX_HEIGHT: u32 = 20000;

/// 卡片宽度允许的范围。
///
/// 下限 460 不是拍脑袋：再窄内容列就放不下「头像 + KFC + 蓝标 + @句柄」，
/// 句柄会被迫在「刚/刚」这种地方断行，用户名行进入 wrap 兜底后卡片底部
/// 还会多出一条空白带（实测 400–456 有 96–139px 空白）。上限 1600：
/// 再宽就不像推特卡片了。
const MIN_WIDTH: u32 = 460;
const MAX_WIDTH: u32 = 1600;

/// 引擎要求视口宽高至少 16px（`Config::validate`），这里取它允许的最小值。
const MIN_VIEWPORT_HEIGHT: u32 = 16;

/// 渲染失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 布局 / 光栅化失败。
    #[error("渲染引擎报错: {0}")]
    Engine(String),
    /// 文本太长，渲染结果超高度上限。
    #[error("卡片高度超过 {0}px")]
    TooTall(u32),
}

/// 把一段文案渲染成卡片 PNG；`width` 是卡片宽度（px）。
pub fn render_card(content: &str, width: u32) -> Result<Vec<u8>, Error> {
    let width = width.clamp(MIN_WIDTH, MAX_WIDTH);

    // 先估算再渲染：光栅化的位图大小是「宽 × 高 × 4 字节」，等渲完再从 PNG 头里
    // 读高度就已经太晚了（那张位图可能几百 MB）。估算偏保守，只会误伤超长文本。
    if template::estimate_height(content, width) > MAX_HEIGHT {
        return Err(Error::TooTall(MAX_HEIGHT));
    }

    let html = template::build(content, width);
    render_html(&html, width)
}

/// 渲染任意 HTML（测试与调样式用）。
pub fn render_html(html: &str, width: u32) -> Result<Vec<u8>, Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言是合法 PNG 并返回尺寸。
    fn size(png: &[u8]) -> (u32, u32) {
        assert_eq!(
            &png[..8],
            b"\x89PNG\r\n\x1a\n",
            "PNG 签名不对：{}",
            png.len()
        );
        assert_eq!(&png[12..16], b"IHDR", "IHDR 必须在签名之后");
        assert_eq!(
            &png[png.len() - 8..png.len() - 4],
            b"IEND",
            "PNG 必须以 IEND 结尾"
        );
        png_size(png).expect("PNG 头里应当有尺寸")
    }

    #[test]
    fn renders_valid_png() {
        let png = render_card("第一行\n第二行", 800).expect("渲染应当成功");
        let (width, height) = size(&png);
        assert_eq!(width, 800);
        assert!(height > 150, "卡片高度应当大于 150px，实际 {height}");
    }

    #[test]
    fn long_text_is_taller() {
        let short = render_card("一行", 800).expect("渲染应当成功");
        let long = render_card(&"疯狂星期四，v我50。\n".repeat(10), 800).expect("渲染应当成功");
        let (short_height, long_height) = (size(&short).1, size(&long).1);
        assert!(
            short_height + 100 < long_height,
            "10 行长文应当明显更高：{long_height} vs {short_height}"
        );
    }

    #[test]
    fn width_is_honored() {
        for width in [460, 520, 800, 1200] {
            let png = render_card("测试宽度", width).expect("渲染应当成功");
            assert_eq!(size(&png).0, width, "输出宽度必须等于入参 width");
        }
    }

    /// 各种极端输入都要出图（或者明确报错），但绝不能 panic / 越界。
    #[test]
    fn extreme_inputs_do_not_panic() {
        let cases = [
            // 500+ 字的长文
            "这是一段很长的文案，用来测试自动折行和高度自适应。".repeat(20),
            // 没有任何空格的超长串（URL / base64 一类）
            "https://example.com/".to_owned() + &"a".repeat(300),
            // 纯英文 + 超长单词
            "The quick brown fox jumps over the lazy dog. Supercalifragilisticexpialidocious!"
                .to_owned(),
            // 只有 emoji
            "🍗🍔🍟🥤🍦🎉🔥✨😋👍".to_owned(),
            // 只有空白和空行
            "  \n\n \t \n".to_owned(),
            // 空字符串
            String::new(),
            // 需要转义的字符
            "a<b>&c \"quoted\" &amp; <script>alert(1)</script>".to_owned(),
        ];
        for case in cases {
            let png = render_card(&case, 800).unwrap_or_else(|error| panic!("渲染失败: {error}"));
            let (width, height) = size(&png);
            assert_eq!(width, 800);
            assert!(height >= 100, "卡片至少要有头像行 + 图标行，实际 {height}");
        }
    }

    #[test]
    fn oversized_content_is_rejected_without_panicking() {
        let huge = "字".repeat(60_000);
        match render_card(&huge, 800) {
            Err(Error::TooTall(limit)) => assert_eq!(limit, MAX_HEIGHT),
            other => panic!("超长文本应当返回 TooTall，实际 {other:?}"),
        }
    }

    #[test]
    fn width_is_clamped() {
        // 极窄 / 极宽的入参不能把布局搞崩，会先被夹到合法区间。
        let narrow = render_card("窄", 10).expect("渲染应当成功");
        assert_eq!(size(&narrow).0, MIN_WIDTH);
        let wide = render_card("宽", 99_999).expect("渲染应当成功");
        assert_eq!(size(&wide).0, MAX_WIDTH);
    }
}
