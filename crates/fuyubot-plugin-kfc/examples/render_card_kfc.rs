//! 生成一张卡片要用的 html2img 请求 JSON，用来调样式 / 做验收。
//!
//! 界面在 `assets/card.html`（Vue 模板 + 内联 CSS）；渲染由宿主的
//! `fuyubot.html2img` 服务完成。这里把 `{ html, vue, data, … }` 落盘，
//! 装好 html2img 的宿主 / 调试环境可以直接拿它复现。
//!
//! ```bash
//! # 直接给文案（`\n` 会换成真换行），默认输出 render-sample.json
//! cargo run --example render_card_kfc -- "第一行\n第二行"
//! # 指定输出与宽度（默认 800）
//! cargo run --example render_card_kfc -- "疯狂星期四，谁能 V 我 50" render-sample.json 520
//! # 从文件读（保留换行）
//! cargo run --example render_card_kfc -- @content.txt render-sample.json
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut content: Option<String> = None;
    let mut output = PathBuf::from("render-sample.json");
    let mut width: u32 = 800;

    for arg in std::env::args().skip(1) {
        if arg.ends_with(".json") || arg.ends_with(".JSON") {
            output = PathBuf::from(arg);
        } else if let Ok(parsed) = arg.parse::<u32>() {
            // 纯数字参数当宽度，方便试窄卡片（320 / 520 / 1200 …）。
            width = parsed;
        } else if content.is_none() {
            content = Some(arg);
        }
    }

    let Some(raw) = content else {
        eprintln!("用法：render_card_kfc <文案|@文件> [输出.json] [宽度]");
        return ExitCode::FAILURE;
    };

    let text = if let Some(path) = raw.strip_prefix('@') {
        match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("读文件失败 {path}: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        // 命令行里的 `\n` 换成真换行，方便一次写多行。
        raw.replace("\\n", "\n")
    };

    let width = kfc::render::clamp_width(width);
    let payload = serde_json::json!({
        "html": kfc::render::TEMPLATE,
        "vue": true,
        "data": kfc::render::view_data(&text, width),
        "width": width,
        "height": 100,
        "scale": 1.0,
        "format": "jpg",
        "auto_height": true,
    });
    match serde_json::to_vec_pretty(&payload) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(&output, bytes) {
                eprintln!("写 {} 失败: {error}", output.display());
                return ExitCode::FAILURE;
            }
            println!(
                "已输出 {}（模板 {} 字节，宽度 {}px）",
                output.display(),
                kfc::render::TEMPLATE.len(),
                width
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("序列化失败: {error}");
            ExitCode::FAILURE
        }
    }
}
