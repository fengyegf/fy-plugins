//! 带默认公网背景的样张：输出 html2img 请求 JSON（背景图由 html2img 去拉）。
//!
//! ```text
//! cargo run --example render_real
//! ```
//!
//! 输出 `target/qa/json/card-real.json`（请求）与 `target/qa/data/card-real.json`
//! （data，可喂给 `target/vuecheck`）。背景 URL 来自 `[plugin.system] background_url`
//! 的缺省值（TS 版 `system.vue` 里的 B 站图）；html2img 拉不到时会自动只留渐变，
//! 那正是插件在弱网下的真实行为。

use serde_json::json;
use system::core::Config;
use system::render::{FORMAT, HEIGHT, SCALE, TEMPLATE, WIDTH, samples, view_data};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::default();
    let background = config.background_url.as_deref();
    println!("背景图 URL：{}", background.unwrap_or("（无，纯渐变）"));

    let data = view_data(
        &samples::sample_card(),
        &samples::sample_assets(),
        background,
    );
    let request = json!({
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
    });

    let json_dir = std::path::Path::new("target/qa/json");
    let data_dir = std::path::Path::new("target/qa/data");
    std::fs::create_dir_all(json_dir)?;
    std::fs::create_dir_all(data_dir)?;
    let request_path = json_dir.join("card-real.json");
    let data_path = data_dir.join("card-real.json");
    std::fs::write(&request_path, serde_json::to_vec_pretty(&request)?)?;
    std::fs::write(&data_path, serde_json::to_vec_pretty(&data)?)?;
    println!(
        "{}（模板 {} 字节，data {} 字节）",
        request_path.display(),
        TEMPLATE.len(),
        data_path.metadata()?.len()
    );
    Ok(())
}
