//! galgame 记录的结构与字段映射。
//!
//! 字段名与既有数据格式兼容，序列化后就是 MongoDB 文档的形状（camelCase）：
//!
//! | Rust | 文档 | 来源 | 可缺省 |
//! | --- | --- | --- | --- |
//! | `title` | `title` | 核对结果 | 否 |
//! | `chinese_name` | `chineseName` | 核对结果 | 是 |
//! | `release_date` | `releaseDate` | 核对结果 | 否 |
//! | `game_genre` | `gameGenre` | 核对结果（已拆成数组） | 否 |
//! | `buy` | `buy[{name,link}]` | 核对结果 | 否 |
//! | `developer` | `developer` | 核对结果 | 是 |
//! | `publisher` | `publisher` | 核对结果 | 是 |
//! | `telegraph_url` | `telegraphUrl` | 解析阶段生成的 Telegraph 页 | 否 |
//! | `description` | `description` | 核对结果（空串也写） | 否 |
//! | `extra_info` | `extraInfo` | 核对结果（空串也写） | 否 |
//! | `download_links` | `downloadLinks` | 兼容字段，恒为 `""` | 否 |
//! | `platform` | `platform` | 核对结果 | 是 |
//! | `zip_sha256` | `zipSha256` | 核对结果 | 是 |
//! | `blur_cover` | `blurCover` | 核对结果（用户跳过遮罩步骤时不写） | 是 |
//! | `cover_image` | `coverImage` | 页面解析 | 否 |
//! | `images` | `images` | 页面解析（不含封面） | 否 |
//! | `brand` | `brand` | 页面解析 | 否 |
//! | `video` | `video` | 页面解析（仅 DMM 同人页） | 是 |
//! | `updated_at` | `updated_at` | 存储层 `$currentDate` 生成 | 是 |
//!
//! 「可缺省」列里的字段用 `Option` 表达：空串 / 空数组在 [`Record::build`] 里
//! 收成 `None`，序列化时整个键都不出现，于是 `$set` 不会拿空值覆盖库里的旧内容。
//! `description` / `extra_info` 是例外：兼容格式始终保留这两个键，这里保持空串也写。
//!
//! `blurCover` 有独立语义：只有用户**显式回答**过「剧透遮罩」这一步才写（`Some`），
//! 跳过（回复 OK）时不写——否则二次 `/p` 跳过会把库里已有的 `true` 覆盖成 `false`。
//! 跳过时记录里不包含这个键，保持兼容语义。
//!
//! `downloadLinks` 在兼容格式中恒为 `""`、从未被填充；为了文档形状逐键对齐，这里仍保留
//! `download_links` 并始终写空串（此前省略会让新文档缺这个键）。
//!
//! `updated_at` 不由本结构生成：写入时交给 MongoDB 的 `$currentDate`，不受本机时钟影响；
//! 读取时用 `Option<Value>` 接住
//! relaxed Extended JSON 的 `{"$date": ...}`。它只用于排障，缺失或形状不认识
//! 都不算错误；反序列化默认忽略未知字段，库里存量的 `_id` 等不影响读取。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一条入正地址，对应既有格式的 `buy: { name, link }`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buy {
    /// 发售平台名（steam / fanza / dlsite / other）。
    #[serde(default)]
    pub name: String,
    /// 发售平台链接。
    #[serde(default)]
    pub link: String,
}

/// 一条成品 galgame 记录（核对结果 + 解析出的图片 / 品牌）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub title: String,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub game_genre: Vec<String>,
    #[serde(default)]
    pub buy: Vec<Buy>,
    #[serde(default)]
    pub telegraph_url: String,
    /// 游戏介绍。兼容格式始终保留该键，空串也写进文档。
    #[serde(default)]
    pub description: String,
    /// 其他信息（原画 / 剧本 / 声优等）。空串也写，保持兼容形状。
    #[serde(default)]
    pub extra_info: String,
    /// 兼容字段 `downloadLinks` 恒为 `""`；保留该键以保持文档逐键对齐。
    #[serde(default)]
    pub download_links: String,
    #[serde(default)]
    pub cover_image: String,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub brand: String,

    /// 用户显式回答过「剧透遮罩」时才是 `Some`；跳过时不写该键。
    ///
    /// 这样二次 `/p` 跳过这一步不会把库里已有的 `true` 覆盖回 `false`，
    /// 跳过后不写该键，避免覆盖数据库中的已有值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_cover: Option<bool>,

    // 以下字段可以没有：没有就不写进 `$set`（见模块文档）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chinese_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,

    /// 写入时间，由存储层 `$currentDate` 生成；只读，永不参与序列化。
    ///
    /// 显式写 `rename`：`rename_all = "camelCase"` 会把它变成 `updatedAt`，
    /// 而数据库文档中的键是下划线的 `updated_at`。
    #[serde(rename = "updated_at", default, skip_serializing)]
    pub updated_at: Option<Value>,
}

impl Record {
    /// 组装成品记录：核对结果来自 [`Info`](crate::galgame::Info)，
    /// 封面 / 预览图 / 品牌 / 视频来自页面解析的 [`Work`](crate::galgame::Work)。
    pub fn build(work: &crate::galgame::Work, info: &crate::galgame::Info) -> Self {
        Self {
            title: info.title.clone(),
            release_date: info.release_date.clone(),
            game_genre: info.genre.clone(),
            buy: info
                .buy
                .iter()
                .map(|link| Buy {
                    name: link.name.clone(),
                    link: link.link.clone(),
                })
                .collect(),
            telegraph_url: info.telegraph.clone(),
            // 跳过遮罩步骤时不写该键，避免把库里的旧值覆盖成 false。
            blur_cover: info.blur_cover_answered.then_some(info.blur_cover),
            description: info.description.clone(),
            extra_info: info.extra.clone(),
            // downloadLinks 在兼容格式中始终是空串，这里保持同一文档形状。
            download_links: String::new(),
            cover_image: work.cover.clone(),
            images: work.images.clone(),
            brand: work.brand.clone(),
            chinese_name: filled(&info.chinese_name),
            developer: filled_list(&info.developer),
            publisher: filled_list(&info.publisher),
            platform: filled_list(&info.platform),
            zip_sha256: filled(&info.zip_sha256),
            video: info_video(work.video.as_deref()),
            updated_at: None,
        }
    }

    /// 序列化成 `$set` 用的对象。
    ///
    /// 可缺省字段由 `skip_serializing_if` 自动省略，`updated_at` 由
    /// `skip_serializing` 保证不出现——写时间只能来自 `$currentDate`。
    pub(crate) fn to_set(&self) -> Result<Value, String> {
        let value =
            serde_json::to_value(self).map_err(|err| format!("序列化游戏记录失败：{err}"))?;
        match value {
            Value::Object(_) => Ok(value),
            other => Err(format!("序列化游戏记录失败：期望对象，得到 {other}")),
        }
    }

    /// 从 `find_one` 返回的文档反序列化。
    ///
    /// 未知字段（`_id` / 将来新增的字段）自动忽略；缺字段走 `#[serde(default)]`，
    /// 这样老数据不会让查询整条失败。
    pub(crate) fn from_doc(doc: Value) -> Result<Self, String> {
        serde_json::from_value(doc).map_err(|err| format!("游戏记录字段不符合预期：{err}"))
    }
}

/// 可缺省字符串：去空白后为空就当没填。
fn filled(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// 可缺省字符串数组：空数组当没填。
fn filled_list(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}

/// 可缺省的可选字符串：`None` 与空串都当没填。
fn info_video(video: Option<&str>) -> Option<String> {
    video.and_then(filled)
}

/// 写入结果，只用于日志与后续分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// upsert 插入了新文档。
    Inserted,
    /// 命中了已有文档并更新（`modified` 可能为 0：内容没变化）。
    Updated,
}

impl SaveOutcome {
    /// 写日志用的一句话。
    pub fn label(self) -> &'static str {
        match self {
            Self::Inserted => "新记录",
            Self::Updated => "已有记录",
        }
    }
}

impl std::fmt::Display for SaveOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}
