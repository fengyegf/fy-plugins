//! 真网络探针：把 `get_song` / `fetch_cover` / `select_lyrics` 走一遍。
//!
//! 用法（在 `plugins/musiccard` 下）：
//!
//! ```powershell
//! cargo run --example netease_probe                  # 默认搜"晴天"
//! cargo run --example netease_probe -- 347230        # 直接给 ID
//! cargo run --example netease_probe -- 七里香         # 关键词
//! cargo run --example netease_probe -- "https://music.163.com/song?id=347230"
//! ```
//!
//! 只读探针：不写文件、不改任何状态，失败时进程退出码为 1。
//!
//! 注意：`src/lib.rs` 里的模块目前是私有的（便于宿主内部使用），
//! 所以示例用 `#[path]` 把需要的模块直接编进自己的 crate root，
//! 这样示例不需要等 lib 把模块改成 `pub`。
//!
//! 本机实测（2026-11-04，网络可用）：
//! - 关键词 `晴天` → "晴天 (原唱 周杰伦)" / RyaVocal（ID 2668397359），
//!   JPEG 封面 210851 字节，选出 5 行歌词（cloudsearch 第一条是翻唱，与旧版一致）；
//! - `347230` 与 `https://music.163.com/song?id=347230` → "海阔天空" / Beyond，
//!   JPEG 封面 110314 字节，选出 5 行歌词；
//! - `536622304` → "Lemon" / 米津玄師，日文原文 + 中文译文按时间对齐，5 行都有译文。

#[path = "../src/model.rs"]
mod model;
#[path = "../src/netease/mod.rs"]
mod netease;
#[path = "../src/util/mod.rs"]
mod util;

use std::process::ExitCode;

fn main() -> ExitCode {
    let input = std::env::args().nth(1).unwrap_or_else(|| "晴天".to_owned());

    println!("== 输入 ==\n{input}");
    match netease::extract_song_id(&input) {
        Some(id) => println!("从输入里解析出歌曲 ID：{id}"),
        None => println!("输入里没有 ID，按关键词搜索"),
    }

    let song = match netease::get_song(&input) {
        Ok(song) => song,
        Err(error) => {
            eprintln!("\n取歌失败：{error}");
            return ExitCode::FAILURE;
        }
    };

    // 歌曲概要：直接打成 JSON，方便贴进报告 / 比对字段。
    let summary = serde_json::json!({
        "id": song.id,
        "name": song.name,
        "artists": song.artists,
        "artist_line": song.artist_line(),
        "album": song.album,
        "cover_url": song.cover_url,
        "share_url": song.share_url(),
        "lyric_bytes": song.lyric.len(),
        "translated_lyric_bytes": song.translated_lyric.len(),
    });
    println!("\n== 歌曲 JSON 概要 ==");
    match serde_json::to_string_pretty(&summary) {
        Ok(text) => println!("{text}"),
        Err(error) => {
            eprintln!("概要序列化失败：{error}");
            return ExitCode::FAILURE;
        }
    }

    let cover = match netease::fetch_cover(&song.cover_url) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("\n下载封面失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    println!("\n== 封面 ==");
    println!("字节数：{}", cover.len());
    println!("前 16 字节：{}", hex_prefix(&cover, 16));
    println!("判定格式：{}", sniff_format(&cover));

    let raw_lines = song
        .lyric
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .count();
    println!("\n== 歌词 ==");
    println!(
        "原文 {} 字节 / {} 个非空行；译文 {} 字节",
        song.lyric.len(),
        raw_lines,
        song.translated_lyric.len()
    );
    let picked = netease::select_lyrics(&song.lyric, &song.translated_lyric);
    println!("select_lyrics 选出 {} 行：", picked.len());
    for (index, line) in picked.iter().enumerate() {
        match &line.translation {
            Some(translation) => println!("  {}. {}  /  {}", index + 1, line.text, translation),
            None => println!("  {}. {}", index + 1, line.text),
        }
    }

    println!("\n探针跑通。");
    ExitCode::SUCCESS
}

fn hex_prefix(bytes: &[u8], take: usize) -> String {
    bytes
        .iter()
        .take(take)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 光看魔数就知道封面是不是真图片。
fn sniff_format(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "JPEG"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "PNG"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..]) {
        "WebP"
    } else {
        "未知（可能不是图片）"
    }
}
