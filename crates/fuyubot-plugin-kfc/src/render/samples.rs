//! 调样式用的样张生成器（`cargo test render_samples -- --ignored --nocapture`）。
//!
//! 只做一件事：把一组有代表性的文案渲染成 `target/samples/render-*.png`，
//! 人眼对照原 TS 管线的样张（satori + resvg + Noto Sans SC 的输出）用。
//! 不参与插件运行，也不读写仓库外的任何目录。

use super::render_card;

/// 原 TS 管线样张的正文，用来逐行对照版式（断行位置、行高、行距）。
pub const GOLDEN: &str = "临江仙•抒怀
疯语薄言悲复喜，狂了营营此生。
星低云阔夜流空。期年终碌碌，四季苦匆匆。
请君莫笑当年事，微名何羡留声。
我自孤行尘寰中。五陵芳华尽，十里散秋风。";

/// 样张输出目录（相对 crate 根目录；`target/` 本来就在 .gitignore 里）。
const OUTPUT_DIR: &str = "target/samples";

/// (文件名后缀, 宽度, 正文)
fn cases() -> Vec<(&'static str, u32, String)> {
    let long = "今天星期四，肯德基疯狂星期四，v我50。".repeat(24);
    let no_space = format!("https://example.com/{}", "a".repeat(160));
    let mixed = "Hello 世界！这是一段 mixed English 和中文混排的文本，用来检查断行是否正确。\
Supercalifragilisticexpialidocious 这种超长英文单词也应当被强制断开而不会溢出卡片边界。"
        .to_owned();
    let emoji = "🍗🍔🍟🥤🍦🎉🔥✨😋👍".to_owned();
    let blanks = "\n\n  首行前有空格，中间空行，末尾也留白。  \n\n\n尾巴\n".to_owned();
    let url = "看这里：https://www.kfc.com.cn/menu/2024/04/11/fried-chicken-bucket-coupon-50-off?utm_source=bot&utm_medium=png&utm_campaign=crzay-thursday"
        .to_owned();
    let escaped = "a<b>&c \"quoted\" 'single' 1 < 2 && 3 > 2".to_owned();
    let empty = String::new();
    vec![
        ("golden", 800, GOLDEN.to_owned()),
        ("long", 800, long),
        ("nospace", 800, no_space),
        ("mixed", 800, mixed),
        ("emoji", 800, emoji),
        ("blanks", 800, blanks),
        ("url", 800, url),
        ("escaped", 800, escaped),
        ("empty", 800, empty),
        ("narrow", 520, GOLDEN.to_owned()),
        // 宽度的合法下限（r`render::render_card` 会把更窄的值夹到这里）。
        ("min", 460, GOLDEN.to_owned()),
        ("wide", 1200, GOLDEN.to_owned()),
    ]
}

#[test]
#[ignore = "手动调样式时才跑：会往 target/samples/ 写 PNG"]
fn render_samples() {
    let dir = std::path::Path::new(OUTPUT_DIR);
    std::fs::create_dir_all(dir).expect("创建样张目录失败");
    for (name, width, content) in cases() {
        let png = render_card(&content, width).expect("样张渲染失败");
        let path = dir.join(format!("render-{name}-{width}.png"));
        std::fs::write(&path, &png).expect("写样张失败");
        println!("{} → {} bytes", path.display(), png.len());
    }
}
