//! 卡片渲染 / 命令流程共用的数据模型。

/// 一行歌词 + 可选翻译。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    /// 原文（可能已被截断）。
    pub text: String,
    /// 译文（可选，可能已被截断）。
    pub translation: Option<String>,
}
