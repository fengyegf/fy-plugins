//! 核对完成后的分享数据与成品消息文案。
//!
//! 文案直接拼成带样式的片段（[`Piece`]），**不写 Markdown**：
//! 标题加粗、入正地址与预览图是链接、介绍与其他信息是可折叠引用块。
//! 这样既不用转义用户文本里的 `*_|#`，也不吃 TDLib 两套 Markdown 的版本差异。

use crate::host::messenger::Piece;

/// 信息核对完成后的分享数据。
#[derive(Debug, Default, Clone)]
pub struct Info {
    pub title: String,
    pub chinese_name: String,
    pub release_date: String,
    pub genre: Vec<String>,
    pub buy: Vec<Link>,
    pub developer: Vec<String>,
    pub publisher: Vec<String>,
    /// 预览图所在的 Telegraph 地址。
    pub telegraph: String,
    pub description: String,
    /// 原画 / 剧本 / 声优 等自由补充。
    pub extra: String,
    /// `["android"]` / `["pc"]` / 两者都有。
    pub platform: Vec<String>,
    /// 压缩包 SHA256，用于后期校验。
    pub zip_sha256: String,
    /// 封面是否加剧透遮罩。
    pub blur_cover: bool,
    /// 用户是否显式回答过「剧透遮罩」这一步（回复 OK 跳过时为 `false`）。
    ///
    /// 入库据此决定写不写 `blurCover`：跳过时不覆盖库里已有的值，
    /// 未填写时保持 `None`，序列化阶段不会写入空键。
    pub blur_cover_answered: bool,
}

/// 一条入正地址。
#[derive(Debug, Clone)]
pub struct Link {
    /// 平台名（steam / fanza / dlsite / other）。
    pub name: String,
    pub link: String,
}

/// 成品消息：标题 → 基本字段 → 入正地址 → 介绍（折叠）→ 其他信息（折叠）→ 平台。
pub fn share_text(info: &Info) -> Vec<Piece> {
    let chinese = if info.chinese_name.is_empty() {
        &info.title
    } else {
        &info.chinese_name
    };
    let mut pieces = vec![
        Piece::bold(format!("#游戏分享 #Galgame {}", info.title)),
        Piece::plain(format!(
            "\n\n中文名: {chinese}\n发售时间: {}\n\n类型: {}\n入正地址: ",
            info.release_date,
            tags(&info.genre)
        )),
    ];

    for (index, link) in info.buy.iter().enumerate() {
        if index > 0 {
            pieces.push(Piece::plain(" | "));
        }
        pieces.push(Piece::link(buy_name(&link.name), link.link.clone()));
    }

    pieces.push(Piece::plain(format!(
        "\n\n开发商: {}\n发行商: {}\n",
        tags(&info.developer),
        tags(&info.publisher)
    )));
    pieces.push(Piece::link("预览图", info.telegraph.clone()));
    pieces.push(Piece::plain("\n介绍:"));
    if !info.description.trim().is_empty() {
        pieces.push(Piece::plain("\n"));
        pieces.push(Piece::quote(info.description.clone()));
    }
    if !info.extra.trim().is_empty() {
        pieces.push(Piece::plain("\n\n其他信息:\n"));
        pieces.push(Piece::quote(info.extra.clone()));
    }
    if !info.platform.is_empty() {
        pieces.push(Piece::plain(format!("\n\n{}", platforms(&info.platform))));
    }
    pieces
}

/// 发售平台显示名：只有 STEAM / FANZA 换名字，其余原样。
fn buy_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "steam" => "STEAM(官方国际中文版)".to_owned(),
        "fanza" => "FANZA(日文原版)".to_owned(),
        _ => name.to_owned(),
    }
}

/// `#标签` 串。
fn tags(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("#{}", tag_text(item)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 支持平台以标签展示，未提供时不输出。
fn platforms(items: &[String]) -> String {
    items
        .iter()
        .map(|item| match item.as_str() {
            "pc" => "#PC".to_owned(),
            "android" => "#安卓".to_owned(),
            other => format!("#{other}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 标签文本：去掉 OVA / THE ANIMATION 这类噪音，只留文字与数字。
fn tag_text(raw: &str) -> String {
    let stripped = strip_ignore_case(&strip_ignore_case(raw, "OVA"), "THE ANIMATION");
    stripped.chars().filter(|ch| ch.is_alphanumeric()).collect()
}

/// 删掉子串（大小写不敏感）；`needle` 必须全是 ASCII（`to_ascii_lowercase` 不改字节长度）。
fn strip_ignore_case(text: &str, needle: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut kept = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find(&needle) {
        kept.push_str(&text[cursor..cursor + found]);
        cursor += found + needle.len();
    }
    kept.push_str(&text[cursor..]);
    kept
}

/// 按分隔符拆分并去掉空项。
fn split(text: &str, separators: &[char]) -> Vec<String> {
    text.split(separators)
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect()
}

/// 游戏类型：`ADV、SLG` / `ADV/SLG` / `ADV,SLG` 都认。
pub fn split_genres(text: &str) -> Vec<String> {
    split(text, &['/', '、', ',', '，'])
}

/// 开发商 / 发行商：只按英文逗号拆。
pub fn split_commas(text: &str) -> Vec<String> {
    split(text, &[','])
}

/// 按链接推断发售平台名。
pub fn platform_name(link: &str) -> &'static str {
    let link = link.to_ascii_lowercase();
    if link.contains("steam") {
        "steam"
    } else if link.contains("dmm") || link.contains("fanza") {
        "fanza"
    } else if link.contains("dlsite") {
        "dlsite"
    } else {
        "other"
    }
}
