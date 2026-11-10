//! 独立验收用的集成测试（QA 添加，不依赖 crate 内部单元测试的实现细节）。
//!
//! 只走公开 API：`render::render_card` / `render::template` / `core::text` /
//! `cmd::parser` / `core::Config`。覆盖验收要求里的五类：
//! 合法 PNG、长文本更高、宽度生效、HTML 转义、配置默认值/解析结果。
//!
//! 配置段的**解析**（`Config::load` 需要宿主 `HostApi`）由 `src/core/config.rs` 的
//! 单元测试覆盖；这里验它解析后的默认值是否与 README 一致。

use std::time::{Duration, Instant};

use kfc::cmd::parser;
use kfc::core::{Config, text};
use kfc::render::{render_card, template};

/// 读出 PNG 的 `(宽, 高)`，顺带校验签名 / IHDR / IEND。
fn png_size(png: &[u8]) -> (u32, u32) {
    assert!(png.len() > 33, "PNG 太短: {} bytes", png.len());
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG 签名不对");
    assert_eq!(&png[12..16], b"IHDR", "IHDR 必须在签名之后");
    assert_eq!(
        &png[png.len() - 8..png.len() - 4],
        b"IEND",
        "PNG 必须以 IEND 结束"
    );
    let width = u32::from_be_bytes(png[16..20].try_into().expect("IHDR 宽度"));
    let height = u32::from_be_bytes(png[20..24].try_into().expect("IHDR 高度"));
    (width, height)
}

#[test]
fn renders_a_valid_png() {
    let png = render_card("第一行\n第二行", 800).expect("渲染应当成功");
    let (width, height) = png_size(&png);
    assert_eq!(width, 800, "输出宽度必须等于入参 width");
    assert!(
        height >= 150,
        "两行正文的卡片至少要有 150px 高，实际 {height}"
    );
    assert!(
        png.len() > 5_000,
        "PNG 不像一张真实卡片: {} bytes",
        png.len()
    );
}

#[test]
fn long_text_is_taller() {
    let short = render_card("一行", 800).expect("渲染应当成功");
    let long = render_card(&"疯狂星期四，v我50。\n".repeat(12), 800).expect("渲染应当成功");
    let (short_h, long_h) = (png_size(&short).1, png_size(&long).1);
    assert!(
        long_h >= short_h + 300,
        "12 行长文应当明显更高：{long_h} vs {short_h}"
    );
}

#[test]
fn width_is_honored_across_the_configured_range() {
    // 合法区间 460..=1600；低于下限会被夹到 460（400–456 会出现底部空白带）。
    for width in [460, 520, 800, 1200, 1600] {
        let png = render_card("宽度测试", width).expect("渲染应当成功");
        assert_eq!(png_size(&png).0, width, "width={width} 没有生效");
    }
    let narrow = render_card("宽度测试", 320).expect("渲染应当成功");
    assert_eq!(png_size(&narrow).0, 460, "width<460 应当被夹到 460");
    let too_wide = render_card("宽度测试", 99_999).expect("渲染应当成功");
    assert_eq!(png_size(&too_wide).0, 1600, "width>1600 应当被夹到 1600");
}

#[test]
fn html_specials_are_escaped_not_interpreted() {
    let html = template::build("<script>alert(1)</script> & <b>x</b>", 800);
    assert!(
        !html.contains("<script>"),
        "正文里的 <script> 必须被转义，否则会注入模板"
    );
    assert!(
        html.contains("&lt;script&gt;"),
        "转义后的文本应当出现在 HTML 里"
    );
    assert_eq!(
        template::escape("a<b>&c"),
        "a&lt;b&gt;&amp;c",
        "escape 必须处理 & < >"
    );

    // 真的渲染一遍：转义错了这一张会变成空白或报错。
    let png = render_card("a<b>&c \"quoted\"", 800).expect("转义后的正文应当能渲染");
    assert_eq!(png_size(&png).0, 800);
}

#[test]
fn text_normalization_covers_crlf_and_literal_escapes() {
    // 与 TS 版 normalizeContent 同规则：CRLF / 字面量 \n、/n / 控制字符 / 零宽字符。
    assert_eq!(
        text::normalize("第一行\\n第二行\r\n第三行\u{200B}\u{0007}"),
        "第一行\n第二行\n第三行"
    );
    assert_eq!(text::normalize("\u{2028}a\u{2029}b"), "\na\nb");
    // 渲染器没有 tab stop：\t 必须在入口换成空格，否则卡片上是豆腐块。
    assert_eq!(text::normalize("a\tb"), "a b");
    assert_eq!(text::normalize("  尾部空白  "), "  尾部空白");
}

#[test]
fn config_defaults_match_the_documented_values() {
    let config = Config::default();
    assert_eq!(
        config.api_url, "https://kfc-crazy-thursday.vercel.app/api/index",
        "默认接口地址与 README 不一致"
    );
    assert_eq!(config.width, 800, "默认卡片宽度应为 800（与 TS 版一致）");
    assert_eq!(
        config.timeout,
        Duration::from_secs(15),
        "默认超时与 config.rs 文档不一致"
    );
    assert_eq!(
        config.prefixes,
        ["/", "!", "！", ".", "#"]
            .iter()
            .map(|p| (*p).to_owned())
            .collect::<Vec<_>>(),
        "默认命令前缀与宿主 [config] PREFIXES 缺省不一致"
    );
}

#[test]
fn command_parser_handles_prefixes_and_bot_suffix() {
    let prefixes = ["/", "!", "！", ".", "#"]
        .iter()
        .map(|p| (*p).to_owned())
        .collect::<Vec<_>>();
    let command = parser::parse("！KFC@fuyu_bot 来一份", &prefixes).expect("应当识别为命令");
    assert_eq!(command.name, "KFC");
    assert_eq!(command.args, vec!["来一份".to_owned()]);
    assert!(parser::parse("今天吃什么", &prefixes).is_none());
}

/// 把不同宽度的样张写到 `target/qa/png/`，供人眼验收（不参与断言，target/ 已被忽略）。
#[test]
fn dump_width_samples_for_visual_check() {
    let dir = std::path::Path::new("target/qa/png");
    std::fs::create_dir_all(dir).expect("创建 QA 目录失败");
    let content =
        "临江仙•抒怀\n疯语薄言悲复喜，狂了营营此生。\n星低云阔夜流空。期年终碌碌，四季苦匆匆。";
    for width in [460u32, 520, 1200, 1600] {
        let png = render_card(content, width).expect("渲染应当成功");
        let path = dir.join(format!("test-width-{width}.png"));
        std::fs::write(&path, &png).expect("写样张失败");
        assert_eq!(png_size(&png).0, width);
    }
}

/// QA 手动样张：先走 `text::normalize` 再渲染，模拟 `cmd::kfc` 的真实管线
/// （tab、CRLF、字面量 `\n` 都会在这里被处理）。
#[test]
#[ignore = "QA 手动跑：会往 target/qa/png 写 pipeline-*.png"]
fn dump_pipeline_samples_for_visual_check() {
    let dir = std::path::Path::new("target/qa/png");
    std::fs::create_dir_all(dir).expect("创建 QA 目录失败");
    for (name, path) in [
        ("api_real", "target/qa/inputs/api_real.txt"),
        ("blank", "target/qa/inputs/blank.txt"),
        ("tab", "target/qa/inputs/tab.txt"),
    ] {
        let raw = std::fs::read_to_string(path).expect("读输入失败");
        let content = text::normalize(&raw);
        let png = render_card(&content, 800).expect("渲染应当成功");
        std::fs::write(dir.join(format!("pipeline-{name}-800.png")), &png).expect("写样张失败");
    }
}

/// 渲染耗时：同一进程第二次起应当明显低于首次（字体/引擎初始化只做一次）。
/// 只打印、不设硬阈值，避免在慢机器上误报。
#[test]
fn render_cost_is_measured() {
    let first = Instant::now();
    render_card("首次渲染", 800).expect("渲染应当成功");
    let first = first.elapsed();

    let second = Instant::now();
    render_card("第二次渲染", 800).expect("渲染应当成功");
    let second = second.elapsed();

    println!("渲染耗时：首次 {first:?}，第二次 {second:?}");
    assert!(
        second < Duration::from_secs(5),
        "单张卡片渲染耗时异常：{second:?}"
    );
}
