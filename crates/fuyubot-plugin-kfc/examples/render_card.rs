//! 把一段文案渲染成卡片 PNG，用来调样式 / 做验收。
//!
//! ```bash
//! # 直接给文案（`\n` 会换成真换行），默认输出 render-sample.png
//! cargo run --example render_card -- "第一行\n第二行"
//! # 指定输出与宽度（默认 800）
//! cargo run --example render_card -- "疯狂星期四，谁能 V 我 50" render-sample.png 520
//! # 从文件读（保留换行）
//! cargo run --example render_card -- @content.txt render-sample.png
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut content: Option<String> = None;
    let mut output = PathBuf::from("render-sample.png");
    let mut width: u32 = 800;

    for arg in std::env::args().skip(1) {
        if arg.ends_with(".png") || arg.ends_with(".PNG") {
            output = PathBuf::from(arg);
        } else if let Ok(parsed) = arg.parse::<u32>() {
            // 纯数字参数当宽度，方便试窄卡片（320 / 520 / 1200 …）。
            width = parsed;
        } else if content.is_none() {
            content = Some(arg);
        }
    }

    let Some(raw) = content else {
        eprintln!("用法：render_card <文案|@文件> [输出.png] [宽度]");
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

    let started = std::time::Instant::now();
    match kfc::render::render_card(&text, width) {
        Ok(png) => {
            if let Err(error) = std::fs::write(&output, &png) {
                eprintln!("写 {} 失败: {error}", output.display());
                return ExitCode::FAILURE;
            }
            println!(
                "已输出 {}（{} bytes，{} ms）",
                output.display(),
                png.len(),
                started.elapsed().as_millis()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("渲染失败: {error}");
            ExitCode::FAILURE
        }
    }
}
