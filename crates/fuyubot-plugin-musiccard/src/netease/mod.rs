//! 网易云音乐的取数层。
//!
//! 只依赖网络与 `serde_json`，不碰宿主、不碰渲染；命令层拿到的就是结构化数据。
//!
//! 内部拆成：
//! - [`client`]：搜索 / 详情 / 歌词三个接口 + 封面下载；
//! - [`lyric`]：LRC 解析与歌词选行。
//!
//! 对外只暴露 [`get_song`] / [`fetch_cover`] / [`select_lyrics`] /
//! [`extract_song_id`] 与 [`MusicCardSong`]。

mod client;
mod lyric;

use crate::model::LyricLine;

/// 在卡片上展示的一首歌。
#[derive(Debug, Clone)]
pub struct MusicCardSong {
    pub id: i64,
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
    pub cover_url: String,
    pub lyric: String,
    pub translated_lyric: String,
}

impl MusicCardSong {
    /// "A / B / C"
    pub fn artist_line(&self) -> String {
        self.artists.join(" / ")
    }

    /// 二维码内容。
    pub fn share_url(&self) -> String {
        format!("https://music.163.com/song?id={}", self.id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("网络请求失败：{0}")]
    Http(String),
    /// 业务错误：文案直接透出（`没有搜索到…` / `未找到歌曲…` /
    /// `网易云搜索失败，状态码：…`），由命令层统一加上失败前缀。
    #[error("{0}")]
    Api(String),
    #[error("数据解析失败：{0}")]
    Decode(String),
}

/// 输入：纯数字 ID、网易云链接、或搜索关键词。
///
/// 先尝试从输入里提取 ID，提取不到就按关键词搜索；
/// 搜索无结果报 `没有搜索到“<input>”`；随后取详情 + 歌词。
pub fn get_song(input: &str) -> Result<MusicCardSong, Error> {
    let song_id = match extract_song_id(input) {
        Some(song_id) => song_id,
        None => {
            client::search_song(input)?.ok_or_else(|| Error::Api(format!("没有搜索到“{input}”")))?
        }
    };

    let song = client::song_detail(song_id)?;
    let lyrics = client::song_lyric(song_id)?;

    Ok(MusicCardSong {
        id: song.id,
        name: song.name,
        artists: song.artists,
        album: song.album,
        cover_url: song.cover_url,
        lyric: lyrics.lyric,
        translated_lyric: lyrics.translated,
    })
}

/// 下载封面原图字节（调用方负责解码）。
pub fn fetch_cover(url: &str) -> Result<Vec<u8>, Error> {
    client::download(url)
}

/// 从 LRC 里挑选最多 5 行歌词。
pub fn select_lyrics(original: &str, translated: &str) -> Vec<LyricLine> {
    lyric::select_lyrics(original, translated)
}

/// 从 "347230" 或链接里提取歌曲 ID。
///
/// - 纯数字（`^\d+$`）直接当 ID；
/// - 否则找 `?id=` / `&id=` / `#id=`（大小写不敏感）或 `/song/<id>` 后的第一段数字。
pub fn extract_song_id(input: &str) -> Option<i64> {
    if !input.is_empty() && input.bytes().all(|byte| byte.is_ascii_digit()) {
        return input.parse::<i64>().ok();
    }
    find_id_in_url(input)
}

/// 按出现顺序找 `(?:[?&#]id=|/song/)(\d+)` 的第一个匹配。
fn find_id_in_url(input: &str) -> Option<i64> {
    // 只折迭 ASCII 大小写，中文不受影响。
    let lowered = input.to_ascii_lowercase();
    for (index, character) in lowered.char_indices() {
        let rest = &lowered[index..];
        let digits_start = if character == '/' && rest.starts_with("/song/") {
            Some("/song/".len())
        } else if matches!(character, '?' | '&' | '#') && rest[1..].starts_with("id=") {
            Some(1 + "id=".len())
        } else {
            None
        };

        if let Some(offset) = digits_start {
            let digits: String = rest[offset..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                return digits.parse::<i64>().ok();
            }
        }
    }
    None
}
