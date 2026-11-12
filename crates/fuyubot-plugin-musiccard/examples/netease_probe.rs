//! 真网络探针：把 get_song / select_lyrics 走一遍。
//!
//! 用法（在 crates/fuyubot-plugin-musiccard 下）：
//!
//! ```powershell
//! cargo run --example netease_probe                  # 默认搜"晴天"
//! cargo run --example netease_probe -- 347230        # 直接给 ID
//! cargo run --example netease_probe -- 七里香         # 关键词
//! cargo run --example netease_probe -- "https://music.163.com/song?id=347230"
//! ```
//!
//! 只读探针：不下载封面、不写文件、不改任何状态，失败时进程退出码为 1。
//! 封面只打印公网 URL——真正拉图由 html2img 服务完成。

use std::process::ExitCode;

use musiccard::netease;

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
