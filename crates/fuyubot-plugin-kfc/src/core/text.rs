//! 文本清洗：把接口给的字符串收拾成能直接塞进卡片的纯文本。
//!
//! 规则照抄 TS 版 `index.ts` 的 `normalizeContent`：
//!
//! - 统一换行（`\r\n` / `\r` → `\n`）；
//! - 把字面量 `\n` / `/n` 还原成真换行（接口偶尔转义过头）；
//! - `\u2028` / `\u2029` 一并当换行；
//! - 去掉控制字符与零宽字符；
//! - 去掉行尾空白（`trim_end`）。

/// 清洗一段文案；空内容返回空串（调用方负责兜底提示）。
pub fn normalize(content: &str) -> String {
    let mut text = content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("\\n", "\n")
        .replace("/n", "\n")
        .replace(['\u{2028}', '\u{2029}'], "\n")
        // 制表符换成空格：卡片正文是 `white-space:pre-wrap`，渲染器没有 tab stop，
        // 直接把 \t 画成一个豆腐块（线上文案偶尔带 tab）。
        .replace('\t', " ");
    text.retain(|ch| {
        !matches!(ch, '\u{0000}'..='\u{0008}' | '\u{000B}' | '\u{000C}' | '\u{000E}'..='\u{001F}' | '\u{007F}'..='\u{009F}')
            && !matches!(ch, '\u{200B}' | '\u{200C}' | '\u{FEFF}')
    });
    text.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_line_breaks_and_strips_controls() {
        let raw = "第一行\\n第二行\r\n第三行\u{200B}\u{0007}";
        assert_eq!(normalize(raw), "第一行\n第二行\n第三行");
    }

    #[test]
    fn turns_tabs_into_spaces() {
        // 渲染器没有 tab stop，\t 会变成豆腐块，所以在入口就换掉。
        assert_eq!(normalize("a\tb"), "a b");
    }
}
