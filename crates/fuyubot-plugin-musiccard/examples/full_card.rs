//! 端到端预览：真实网易云接口 → 选歌词 → 650x1220 卡片的 html2img 请求 JSON
//! （card-full.json，含 `html` / `vue` / `data`）。
//!
//! 这是 /musiccard 命令流程的数据侧复刻（不含宿主消息收发与渲染服务）：取歌 →
//! 选歌词 → 截断字段 → 生成**交给 html2img 的请求**。用来在不连 Telegram 的情况下
//! 验证整条取数链路；真实出图请在装了 fuyubot.html2img 的宿主上跑命令。
//!
//! ```text
//! cargo run --example full_card                         # 默认《晴天》(186016)
//! cargo run --example full_card -- 536622304            # 歌曲 ID
//! cargo run --example full_card -- "周杰伦 晴天"         # 关键词
//! cargo run --example full_card -- https://music.163.com/song?id=347230
//! cargo run --example full_card -- --out target/qa/musiccard-request.json
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use musiccard::render::{self, Card};
use musiccard::{netease, util};

/// 默认歌曲：《晴天》网易云 id 186016。
const DEFAULT_QUERY: &str = "186016";

/// 解析命令行：--out <path> 之外的第一个参数当查询串。
fn parse_args() -> (String, PathBuf) {
    let mut query = DEFAULT_QUERY.to_owned();
    let mut output = PathBuf::from("card-full.json");
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out" => {
                if let Some(path) = iter.next() {
                    output = PathBuf::from(path);
                }
            }
            "-h" | "--help" => {
                println!(
                    "用法：cargo run --example full_card -- [查询串] [--out <路径>]\n查询串可以是歌曲 ID、网易云链接或搜索关键词。"
                );
                std::process::exit(0);
            }
            other => query = other.to_owned(),
        }
    }
    (query, output)
}

fn main() -> ExitCode {
    let (query, output) = parse_args();
    match run(&query, &output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("失败：{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(query: &str, output: &std::path::Path) -> Result<(), String> {
    println!("查询：{query}");
    let song = netease::get_song(query).map_err(|error| error.to_string())?;
    println!(
        "歌曲：{} / {} / {}（id={}）",
        song.name,
        song.artist_line(),
        song.album,
        song.id
    );
    println!("封面 URL：{}", song.cover_url);

    let lyrics = netease::select_lyrics(&song.lyric, &song.translated_lyric);
    println!("歌词：{} 行", lyrics.len());
    for (index, line) in lyrics.iter().enumerate() {
        match line.translation.as_deref() {
            Some(translation) => println!("  {}. {}\n     {}", index + 1, line.text, translation),
            None => println!("  {}. {}", index + 1, line.text),
        }
    }

    // 与 cmd::musiccard 完全一致的字段截断 / 日期口径。
    let title = util::truncate(&song.name, 22);
    let artists = util::truncate(&song.artist_line(), 28);
    let album = util::truncate(&song.album, 24);
    let today = chrono::Local::now();
    let month = today.format("%b").to_string();
    let day = today.format("%d").to_string();
    let qr_content = song.share_url();

    let card = Card {
        title: &title,
        artists: &artists,
        album: &album,
        cover_url: &song.cover_url,
        qr_content: &qr_content,
        month: &month,
        day: &day,
        lyrics: &lyrics,
    };
    let data = render::view_data(&card).map_err(|error| error.to_string())?;
    let payload = serde_json::json!({
        "html": render::TEMPLATE,
        "vue": true,
        "data": data,
        "width": render::WIDTH,
        "height": render::HEIGHT,
        "scale": 1.0,
        "format": "jpg",
        "auto_height": false,
        "transparent": false,
        "dark": false,
    });
    let bytes =
        serde_json::to_vec_pretty(&payload).map_err(|error| format!("序列化请求失败：{error}"))?;

    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建输出目录 {} 失败：{error}", parent.display()))?;
    }
    std::fs::write(output, &bytes)
        .map_err(|error| format!("写入 {} 失败：{error}", output.display()))?;
    println!(
        "已生成：{}（650x1220 卡片的 html2img 请求，{} 字节）",
        output.display(),
        bytes.len()
    );
    Ok(())
}
