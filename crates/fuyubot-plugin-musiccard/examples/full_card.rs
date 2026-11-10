//! 端到端预览：真实网易云接口 → 选歌词 → 650x1220 卡片 → `card-full.jpg`。
//!
//! 这是 `/musiccard` 命令流程的数据侧复刻（不含宿主消息收发）：取歌 → 取封面 →
//! 选歌词 → 截断字段 → 渲染 JPEG。用来在不连 Telegram 的情况下验证整条链路。
//!
//! 用法（在 `plugins/musiccard` 目录下）：
//!
//! ```text
//! cargo run --example full_card                         # 默认《晴天》(186016)
//! cargo run --example full_card -- 536622304            # 歌曲 ID
//! cargo run --example full_card -- "周杰伦 晴天"         # 关键词
//! cargo run --example full_card -- https://music.163.com/song?id=347230
//! cargo run --example full_card -- --out D:\tmp\card.jpg
//! ```
//!
//! 说明：`src/{model,util,netease,render}` 在 `lib.rs` 里是私有模块，示例用
//! `#[path]` 复用同一份源码，保证与插件内部走的是完全同一套实现。

#[path = "../src/model.rs"]
mod model;
#[path = "../src/netease/mod.rs"]
mod netease;
#[path = "../src/render/mod.rs"]
mod render;
#[path = "../src/util/mod.rs"]
mod util;

use std::path::PathBuf;
use std::process::ExitCode;

/// 默认歌曲：《晴天》网易云 id 186016。
const DEFAULT_QUERY: &str = "186016";

/// 解析命令行：`--out <path>` 之外的第一个参数当查询串。
fn parse_args() -> (String, PathBuf) {
    let mut query = DEFAULT_QUERY.to_owned();
    let mut output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("card-full.jpg");
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
                    "用法：cargo run --example full_card -- [查询串] [--out <路径>]\n\
                     查询串可以是歌曲 ID、网易云链接或搜索关键词。"
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

    let cover = netease::fetch_cover(&song.cover_url).map_err(|error| error.to_string())?;
    println!("封面：{} 字节（{}）", cover.len(), song.cover_url);

    let lyrics = netease::select_lyrics(&song.lyric, &song.translated_lyric);
    println!("歌词：{} 行", lyrics.len());
    for (index, line) in lyrics.iter().enumerate() {
        match line.translation.as_deref() {
            Some(translation) => println!("  {}. {}\n     {}", index + 1, line.text, translation),
            None => println!("  {}. {}", index + 1, line.text),
        }
    }

    // 与 `cmd::musiccard` 完全一致的字段截断 / 日期口径。
    let title = util::truncate(&song.name, 22);
    let artists = util::truncate(&song.artist_line(), 28);
    let album = util::truncate(&song.album, 24);
    let today = chrono::Local::now();
    let month = today.format("%b").to_string();
    let day = today.format("%d").to_string();
    let qr_content = song.share_url();

    let card = render::Card {
        title: &title,
        artists: &artists,
        album: &album,
        cover: &cover,
        qr_content: &qr_content,
        month: &month,
        day: &day,
        lyrics: &lyrics,
    };
    let jpeg = render::render_card(&card).map_err(|error| error.to_string())?;

    std::fs::write(output, &jpeg)
        .map_err(|error| format!("写入 {} 失败：{error}", output.display()))?;
    println!(
        "已生成：{}（650x1220 JPEG，{} 字节）",
        output.display(),
        jpeg.len()
    );
    Ok(())
}
