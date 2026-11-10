//! 生成样张：在 `plugins/system/` 下跑
//!
//! ```text
//! cargo run --offline --release --example render_card
//! ```
//!
//! 会写出几张 1400×800 的 JPEG（不联网、不读外部文件），并打印尺寸与字节数：
//!
//! | 文件 | 内容 |
//! | --- | --- |
//! | `card-sample.jpg` | 完整样张（有 admin、3 gauge、4 磁盘、10 插件、头像 + 背景图） |
//! | `card-sample-plain.jpg` | 同样的数据，但没有任何素材（纯渐变背景 + 头像占位） |
//! | `card-sample-no-admin.jpg` | 无 admin、2 gauge、1 磁盘、6 插件 |
//! | `card-sample-long.jpg` | 超长名字 / 超长插件名（截断与缩字号） |
//! | `card-sample-empty.jpg` | 全空数据（三处「暂无数据」兜底） |
//!
//! 调版面时改 `src/render/layout.rs` 的常量，重跑这条命令再看图。

use system::core::model::Assets;
use system::render::{render_card, samples};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let full_assets = samples::sample_assets();
    let cases: Vec<(&str, system::core::model::CardData, Assets)> = vec![
        (
            "card-sample.jpg",
            samples::sample_card(),
            full_assets.clone(),
        ),
        (
            "card-sample-plain.jpg",
            samples::sample_card(),
            Assets::default(),
        ),
        (
            "card-sample-no-admin.jpg",
            samples::sample_card_no_admin(),
            full_assets.clone(),
        ),
        (
            "card-sample-long.jpg",
            samples::sample_card_long_names(),
            full_assets.clone(),
        ),
        (
            "card-sample-empty.jpg",
            samples::sample_card_empty(),
            Assets::default(),
        ),
    ];

    for (file, card, assets) in cases {
        let jpeg = render_card(&card, &assets)?;
        std::fs::write(file, &jpeg)?;
        let decoded = image::load_from_memory(&jpeg)?;
        println!(
            "{file}: {}×{} px, {} bytes",
            decoded.width(),
            decoded.height(),
            jpeg.len()
        );
    }

    println!("输出目录：{}", std::env::current_dir()?.display());
    Ok(())
}
