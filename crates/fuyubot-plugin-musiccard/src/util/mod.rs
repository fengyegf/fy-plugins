//! 纯工具函数：不碰宿主、不碰网络。

/// 按**字符**截断，超长时用 `…` 收尾。
///
/// 注意不要按字节切：中文、emoji 都会切坏。
pub fn truncate(text: &str, max_length: usize) -> String {
    let count = text.chars().count();
    if count <= max_length {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(max_length.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}
