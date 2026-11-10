//! 真实素材样张：联网下载 TS 版 `system.vue` 里的背景图，把它同时当两张头像
//! （验证真实照片的解码 / cover 裁切 / 圆形蒙版），再出图。
//!
//! ```text
//! cargo run --offline --release --example render_real
//! ```
//!
//! 输出 `card-real.jpg`。下载失败（离线 / 图片站不可达）时退回纯渐变 + 无头像，
//! 同样能出图——这条路径就是插件在弱网下的真实行为。

use system::core::background;
use system::core::config::Config;
use system::core::model::Assets;
use system::render::{render_card, samples};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::default();
    let downloaded = background::load(&config);
    println!(
        "背景图：{}（{} bytes）",
        if downloaded.is_some() {
            "已获取"
        } else {
            "不可用，退化为纯渐变"
        },
        downloaded.as_ref().map_or(0, Vec::len)
    );

    let assets = Assets {
        // 真实照片当头像：验证 cover 裁切与圆形蒙版，不用合成图糊弄过去。
        bot_photo: downloaded.clone(),
        admin_photo: downloaded.clone(),
        background: downloaded,
    };
    let jpeg = render_card(&samples::sample_card(), &assets)?;
    std::fs::write("card-real.jpg", &jpeg)?;
    let decoded = image::load_from_memory(&jpeg)?;
    println!(
        "card-real.jpg: {}×{} px, {} bytes",
        decoded.width(),
        decoded.height(),
        jpeg.len()
    );
    Ok(())
}
