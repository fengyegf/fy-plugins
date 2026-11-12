//! 生成一张 Epic 卡片要用的 html2img 请求 JSON，用来调样式 / 做验收。
//!
//! 界面在 `assets/card.html`（Vue 模板 + 内联 CSS）；渲染由宿主的
//! `fuyubot.html2img` 服务完成。这里把 {html, vue, data, …} 落盘，
//! 装好 html2img 的宿主 / 调试环境可以直接拿它复现；同时会写一份只含 data
//! 的兄弟文件，可以直接喂给 target/vuecheck 校验模板。
//!
//! ```bash
//! # 默认输出 render-sample.json（以及 render-sample.data.json）
//! cargo run --example render_card_epic
//! # 指定输出
//! cargo run --example render_card_epic -- epic-sample.json
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use epic::core::epic::{FreeGame, FreeGames};
use epic::render::{RenderOptions, payload};

fn main() -> ExitCode {
    let output = std::env::args()
        .skip(1)
        .find(|arg| arg.ends_with(".json") || arg.ends_with(".JSON"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("render-sample.json"));

    let games = sample();
    let request = match payload(&games, &RenderOptions::default()) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("生成失败: {error}");
            return ExitCode::FAILURE;
        }
    };

    let data_path = output.with_extension("data.json");
    let data = &request["data"];
    if let Err(error) = write_json(&output, &request).and_then(|()| write_json(&data_path, data)) {
        eprintln!("写样张失败: {error}");
        return ExitCode::FAILURE;
    }

    println!(
        "已输出 {}（{} 张卡，模板 {} 字节）",
        output.display(),
        games.iter().count(),
        epic::render::TEMPLATE.len()
    );
    println!("模板 data: {}", data_path.display());
    ExitCode::SUCCESS
}

/// 写一份格式化 JSON。
fn write_json(path: &std::path::Path, value: &serde_json::Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, bytes)
}

/// 与真实接口同形状的样张数据（覆盖当前免费 / 即将免费 / 有封面 / 无封面）。
fn sample() -> FreeGames {
    FreeGames {
        current: (1..=4)
            .map(|index| FreeGame {
                title: format!("示例免费游戏 {index}（The Long Title of Demo Game）"),
                start_date: "2026-11-01".to_owned(),
                end_date: "2026-11-08".to_owned(),
                offer_image: Some(format!(
                    "https://cdn2.unrealengine.com/egs/demo-{index}/cover.jpg?w=1200&h=675"
                )),
            })
            .collect(),
        upcoming: vec![FreeGame {
            title: "即将免费的神秘游戏".to_owned(),
            start_date: "2026-11-08".to_owned(),
            end_date: "2026-11-15".to_owned(),
            offer_image: None,
        }],
    }
}
