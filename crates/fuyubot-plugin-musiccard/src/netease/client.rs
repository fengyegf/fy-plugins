//! 网易云 HTTP 取数：搜索 / 歌曲详情 / 歌词 / 封面下载。
//!
//! HTTP 走 `ureq` 阻塞式调用；命令本来就在独立业务线程里跑，不需要 async runtime。

use std::io::Read;
use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

use super::Error;

/// 请求网易云接口时使用的浏览器 UA。
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ",
    "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
);
/// 网易云要求带 Referer，否则部分接口返回空。
const REFERER: &str = "https://music.163.com/";
/// PC 端接口使用的固定 Cookie。
const COOKIE: &str = "os=pc; appver=3.1.0.203271";

/// 关键词搜索接口。
pub const SEARCH_URL: &str = "https://music.163.com/api/cloudsearch/pc";
/// 歌曲详情接口。
pub const DETAIL_URL: &str = "https://interface3.music.163.com/api/v3/song/detail";
/// 歌词与译文接口。
pub const LYRIC_URL: &str = "https://interface3.music.163.com/api/song/lyric";

/// JSON 接口超时。
const API_TIMEOUT: Duration = Duration::from_secs(15);
/// 封面下载超时。
const COVER_TIMEOUT: Duration = Duration::from_secs(20);

/// 一首歌的展示字段（不含歌词）。
#[derive(Debug, Clone)]
pub struct SongDetail {
    pub id: i64,
    pub name: String,
    pub artists: Vec<String>,
    pub album: String,
    pub cover_url: String,
}

/// 歌词原文 + 译文（都是裸 LRC，交给 [`super::select_lyrics`] 挑行）。
#[derive(Debug, Clone, Default)]
pub struct Lyrics {
    pub lyric: String,
    pub translated: String,
}

/// 构造一个带超时的 agent；UA 统一定在 agent 上。
fn agent(timeout: Duration) -> Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()
}

/// 表单体百分号编码。
///
/// 空格编成 `%20` 而不是 `+`：两者在 `application/x-www-form-urlencoded`
/// 里都是空格，网易云两边都认，但 `%20` 在 querystring 语义下更保守。
pub(crate) fn encode_form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// 把 `ureq::Error` 翻成给用户看的中文原因。
fn describe(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, _) => format!("HTTP 状态码 {code}"),
        ureq::Error::Transport(transport) => transport.to_string(),
    }
}

/// POST 一张表单，返回原始响应体。
fn post_form(url: &str, pairs: &[(&str, &str)]) -> Result<String, Error> {
    let body = encode_form(pairs);
    let response = agent(API_TIMEOUT)
        .post(url)
        .set("Referer", REFERER)
        .set("Cookie", COOKIE)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body)
        .map_err(|err| Error::Http(format!("{url} 请求失败：{}", describe(err))))?;

    response
        .into_string()
        .map_err(|err| Error::Http(format!("{url} 读取响应失败：{err}")))
}

/// 解析响应 JSON；顺带把"不是 JSON"的情况说清楚。
fn parse_json(raw: &str, url: &str) -> Result<Value, Error> {
    serde_json::from_str(raw)
        .map_err(|err| Error::Decode(format!("{url} 返回的不是合法 JSON：{err}")))
}

/// 响应里的 `code`（缺失时给 `None`，由调用方决定怎么报错）。
fn api_code(value: &Value) -> Option<i64> {
    value.get("code").and_then(Value::as_i64)
}

/// 错误文案里的状态码：缺失时写成"未知"。
fn code_label(value: &Value) -> String {
    match value.get("code") {
        Some(code) => code.to_string(),
        None => "未知".to_owned(),
    }
}

/// 关键词搜索，返回第一首歌的 ID；搜不到返回 `None`。
///
/// 接口：`POST https://music.163.com/api/cloudsearch/pc`
/// 表单 `s=<keyword>&type=1&limit=1`。
pub fn search_song(keyword: &str) -> Result<Option<i64>, Error> {
    let raw = post_form(SEARCH_URL, &[("s", keyword), ("type", "1"), ("limit", "1")])?;
    let value = parse_json(&raw, SEARCH_URL)?;

    if api_code(&value) != Some(200) {
        return Err(Error::Api(format!(
            "网易云搜索失败，状态码：{}",
            code_label(&value)
        )));
    }

    Ok(value.pointer("/result/songs/0/id").and_then(Value::as_i64))
}

/// 歌曲详情；`code != 200` 或没有 `songs[0]` 时报"未找到歌曲"。
///
/// 接口：`POST https://interface3.music.163.com/api/v3/song/detail`
/// 表单 `c=[{"id":<id>,"v":0}]`。
pub fn song_detail(song_id: i64) -> Result<SongDetail, Error> {
    // 详情接口要求把歌曲参数编码成 JSON 数组。
    let payload = serde_json::json!([{ "id": song_id, "v": 0 }]).to_string();

    let raw = post_form(DETAIL_URL, &[("c", payload.as_str())])?;
    let value = parse_json(&raw, DETAIL_URL)?;

    let song = match value.pointer("/songs/0") {
        Some(song) if api_code(&value) == Some(200) => song,
        _ => return Err(Error::Api(format!("未找到歌曲（ID：{song_id}）"))),
    };

    let album = song
        .get("al")
        .ok_or_else(|| Error::Decode("歌曲详情里缺少专辑字段 al".to_owned()))?;

    let artists = song
        .get("ar")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|artist| artist.get("name").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    // 封面统一加大图参数。
    let pic_url = album
        .get("picUrl")
        .and_then(Value::as_str)
        .unwrap_or_default();

    Ok(SongDetail {
        id: song.get("id").and_then(Value::as_i64).unwrap_or(song_id),
        name: song
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        artists,
        album: album
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        cover_url: format!("{pic_url}?param=720y720"),
    })
}

/// 歌词 + 译文。
///
/// 接口：`POST https://interface3.music.163.com/api/song/lyric`
/// 表单 `id=<id>&cp=false&tv=0&lv=0&rv=0&kv=0&yv=0&ytv=0&yrv=0`。
/// `code != 200` **不算致命错误**：返回空歌词，卡片走"暂无歌词"那句。
pub fn song_lyric(song_id: i64) -> Result<Lyrics, Error> {
    let id = song_id.to_string();
    let raw = post_form(
        LYRIC_URL,
        &[
            ("id", id.as_str()),
            ("cp", "false"),
            ("tv", "0"),
            ("lv", "0"),
            ("rv", "0"),
            ("kv", "0"),
            ("yv", "0"),
            ("ytv", "0"),
            ("yrv", "0"),
        ],
    )?;
    let value = parse_json(&raw, LYRIC_URL)?;

    if api_code(&value) != Some(200) {
        return Ok(Lyrics::default());
    }

    Ok(Lyrics {
        lyric: value
            .pointer("/lrc/lyric")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        translated: value
            .pointer("/tlyric/lyric")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// 下载任意 URL 的原始字节（封面用）。
///
/// 下载封面时只带 UA 与 Referer，不携带接口 Cookie，超时 20s。
pub fn download(url: &str) -> Result<Vec<u8>, Error> {
    let response = agent(COVER_TIMEOUT)
        .get(url)
        .set("Referer", REFERER)
        .call()
        .map_err(|err| Error::Http(format!("{url} 下载失败：{}", describe(err))))?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|err| Error::Http(format!("{url} 读取响应失败：{err}")))?;
    Ok(bytes)
}
