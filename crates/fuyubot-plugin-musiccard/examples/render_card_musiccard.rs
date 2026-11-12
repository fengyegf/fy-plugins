//! 生成一张音乐卡片要用的 html2img 请求 JSON，用来调样式 / 做验收。
//!
//! 界面在 `assets/card.html`（Vue 模板 + 内联 CSS）；渲染由宿主的
//! `fuyubot.html2img` 服务完成。这里把 `{ html, vue, data, … }` 落盘，
//! 装好 html2img 的宿主 / 调试环境可以直接拿它复现，也可以用
//! `target/vuecheck` 把其中的 `data` 单独渲染成 HTML 核对版式。
//!
//! ```bash
//! cargo run --example render_card_musiccard                          # 默认样张 -> render-sample.json
//! cargo run --example render_card_musiccard -- --out card.json       # 指定输出文件
//! cargo run --example render_card_musiccard -- --data data.json      # 只导出模板 data
//! cargo run --example render_card_musiccard -- --cover https://p2.music.126.net/xxx.jpg
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use musiccard::model::LyricLine;
use musiccard::render::{self, Card, HEIGHT, WIDTH};

/// 默认封面：网易云《晴天》（id 186016）的专辑封面，公网 https。
const DEFAULT_COVER: &str =
    "https://p2.music.126.net/ZGffiDQZrGj5s_hnR1CNbg==/109951165566379710.jpg?param=720y720";

/// 示例歌词（《晴天》35% 附近，含两条译文以覆盖「原文 + 译文」版式）。
fn sample_lyrics() -> Vec<LyricLine> {
    vec![
        LyricLine {
            text: "故事的小黄花".to_owned(),
            translation: Some("The little yellow flower of the story".to_owned()),
        },
        LyricLine {
            text: "从出生那年就飘着".to_owned(),
            translation: None,
        },
        LyricLine {
            text: "童年的荡秋千".to_owned(),
            translation: Some("The swing of childhood".to_owned()),
        },
        LyricLine {
            text: "随记忆一直晃到现在".to_owned(),
            translation: None,
        },
        LyricLine {
            text: "Re So So Si Do Si La".to_owned(),
            translation: None,
        },
    ]
}

/// 命令行参数。
struct Args {
    cover: String,
    output: PathBuf,
    /// 只导出模板 data 的路径（不写整份请求）。
    data_output: Option<PathBuf>,
}

fn usage() -> String {
    format!(
        "生成一张 650x1220 音乐卡片的 html2img 请求 JSON\n\n用法：cargo run --example render_card_musiccard -- [选项]\n\n选项：\n  --cover <url>   封面公网 URL（默认《晴天》专辑封面）\n  --out <path>    请求 JSON 输出路径，默认 {}\n  --data <path>   额外只导出模板 data（给 target/vuecheck 用）\n  -h, --help      显示本帮助\n",
        default_output().display()
    )
}

fn default_output() -> PathBuf {
    PathBuf::from("render-sample.json")
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        cover: DEFAULT_COVER.to_owned(),
        output: default_output(),
        data_output: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(usage()),
            "--cover" => args.cover = iter.next().ok_or("--cover 后面缺少 URL")?,
            "--out" => args.output = PathBuf::from(iter.next().ok_or("--out 后面缺少路径")?),
            "--data" => {
                args.data_output = Some(PathBuf::from(iter.next().ok_or("--data 后面缺少路径")?));
            }
            other => return Err(format!("未知参数：{other}\n\n{}", usage())),
        }
    }
    Ok(args)
}

/// 写文件（自动建目录）。
fn write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建输出目录 {} 失败：{error}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|error| format!("写入 {} 失败：{error}", path.display()))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            // --help 也走这里：帮助不是错误。
            println!("{message}");
            return ExitCode::SUCCESS;
        }
    };

    let lyrics = sample_lyrics();
    let card = Card {
        title: "晴天",
        artists: "周杰伦",
        album: "叶惠美",
        cover_url: &args.cover,
        qr_content: "https://music.163.com/song?id=186016",
        month: "Nov",
        day: "04",
        lyrics: &lyrics,
    };

    let data = match render::view_data(&card) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("生成模板 data 失败：{error}");
            return ExitCode::FAILURE;
        }
    };

    // 与 render::render_card 交给服务的入参完全一致。
    let payload = serde_json::json!({
        "html": render::TEMPLATE,
        "vue": true,
        "data": data,
        "width": WIDTH,
        "height": HEIGHT,
        "scale": 1.0,
        "format": "jpg",
        "auto_height": false,
        "transparent": false,
        "dark": false,
    });
    let bytes = match serde_json::to_vec_pretty(&payload) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("序列化请求失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = write(&args.output, &bytes) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    println!(
        "已输出 {}（模板 {} 字节，data 字段：title / artists / album / cover / qrCode / month / day / lyrics / titleSize / artistsSize）",
        args.output.display(),
        render::TEMPLATE.len()
    );

    if let Some(path) = &args.data_output {
        let data_bytes = match serde_json::to_vec_pretty(&data) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("序列化 data 失败：{error}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(error) = write(path, &data_bytes) {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
        println!(
            "已输出 {}（只含模板 data，可交给 target/vuecheck）",
            path.display()
        );
    }

    ExitCode::SUCCESS
}
