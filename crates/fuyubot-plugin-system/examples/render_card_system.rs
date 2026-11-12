//! 生成卡片样张的 html2img 请求 JSON：调版面 / 做验收用。
//!
//! 渲染本身由宿主上的 `fuyubot.html2img` 服务完成，这里把 `{html, vue, data, …}`
//! 落盘到 `target/qa/json/`；同一份 `data` 另存到 `target/qa/data/`，可以直接喂给
//! 本地模板校验器（编译 + 渲染，不需要 html2img）：
//!
//! ```text
//! cargo run --example render_card_system
//! cd target/vuecheck
//! cargo run --quiet -- ../../crates/fuyubot-plugin-system/assets/card.html ../qa/data/card-sample.json ../qa/html/card-sample.html
//! ```
//!
//! | 文件 | 内容 |
//! | --- | --- |
//! | `card-sample` | 完整样张（有 admin、CPU/RAM、4 磁盘、10 插件、头像 + 背景 URL） |
//! | `card-sample-plain` | 同样的数据，但没有任何素材（纯渐变背景 + 头像占位） |
//! | `card-sample-no-admin` | 无 admin、2 gauge、1 磁盘、6 插件 |
//! | `card-sample-long` | 超长名字 / 超长插件名（模板用省略号兜底） |
//! | `card-sample-empty` | 全空数据（三处「暂无数据」兜底） |

use serde_json::{Value, json};
use system::core::model::{Assets, CardData};
use system::render::{FORMAT, HEIGHT, SCALE, TEMPLATE, WIDTH, samples, view_data};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let json_dir = std::path::Path::new("target/qa/json");
    let data_dir = std::path::Path::new("target/qa/data");
    std::fs::create_dir_all(json_dir)?;
    std::fs::create_dir_all(data_dir)?;

    let full = samples::sample_assets();
    let cases: Vec<(&str, CardData, Assets, Option<&str>)> = vec![
        (
            "card-sample",
            samples::sample_card(),
            full.clone(),
            Some(samples::SAMPLE_BACKGROUND_URL),
        ),
        (
            "card-sample-plain",
            samples::sample_card(),
            Assets::default(),
            None,
        ),
        (
            "card-sample-no-admin",
            samples::sample_card_no_admin(),
            full.clone(),
            Some(samples::SAMPLE_BACKGROUND_URL),
        ),
        (
            "card-sample-long",
            samples::sample_card_long_names(),
            full.clone(),
            Some(samples::SAMPLE_BACKGROUND_URL),
        ),
        (
            "card-sample-empty",
            samples::sample_card_empty(),
            Assets::default(),
            None,
        ),
    ];

    for (name, card, assets, background) in cases {
        let data = view_data(&card, &assets, background);
        let request = payload(&data);
        let request_path = json_dir.join(format!("{name}.json"));
        let data_path = data_dir.join(format!("{name}.json"));
        std::fs::write(&request_path, serde_json::to_vec_pretty(&request)?)?;
        std::fs::write(&data_path, serde_json::to_vec_pretty(&data)?)?;
        println!(
            "{}（模板 {} 字节，data {} 字节）",
            request_path.display(),
            TEMPLATE.len(),
            data_path.metadata()?.len()
        );
    }
    Ok(())
}

/// 拼一份完整的 html2img 请求（`vue: true` + `data`）。
fn payload(data: &Value) -> Value {
    json!({
        "html": TEMPLATE,
        "vue": true,
        "data": data,
        "width": WIDTH,
        "height": HEIGHT,
        "scale": SCALE,
        "format": FORMAT,
        "auto_height": false,
        "transparent": false,
        "dark": false,
    })
}
