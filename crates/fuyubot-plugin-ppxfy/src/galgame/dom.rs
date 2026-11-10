//! HTML 取值小工具：`dmm` / `dlsite` 两个解析器共用。
//!
//! 只做「选择器 → 元素 → 字符串」，不含任何站点知识：站点差异留在各自文件里。

use scraper::{ElementRef, Html, Selector};

/// 一行「标签 / 内容」。
pub struct Row {
    pub label: String,
    pub value: String,
}

/// 选择器命中的全部元素。
pub fn elements<'a>(doc: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    match Selector::parse(selector) {
        Ok(selector) => doc.select(&selector).collect(),
        // 选择器写错不该炸整个流程：当成「没命中」。
        Err(_) => Vec::new(),
    }
}

/// 选择器命中的第一个元素的属性。
pub fn attr(doc: &Html, selector: &str, name: &str) -> Option<String> {
    elements(doc, selector)
        .first()
        .and_then(|element| element.attr(name))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// 选择器命中的第一个元素的文本。
pub fn text(doc: &Html, selector: &str) -> String {
    elements(doc, selector)
        .first()
        .map(|element| clean(&element.text().collect::<String>()))
        .unwrap_or_default()
}

/// 选择器命中的每个元素的图片地址（`src` / `data-src`），补全协议后返回。
pub fn images(doc: &Html, selector: &str, base: &str) -> Vec<String> {
    elements(doc, selector)
        .iter()
        .filter_map(|element| element.attr("src").or_else(|| element.attr("data-src")))
        .map(|raw| image_url(base, raw))
        .filter(|url| !url.is_empty())
        .collect()
}

/// 第一个非空串。
pub fn first(candidates: &[String]) -> String {
    candidates
        .iter()
        .find(|candidate| !candidate.is_empty())
        .cloned()
        .unwrap_or_default()
}

/// 折叠空白。
pub fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 补全图片地址：`//x` → `https://x`，`/x` → 站点地址 + `/x`。
pub fn image_url(base: &str, raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else if raw.starts_with('/') {
        format!("{base}{raw}")
    } else {
        raw.to_owned()
    }
}

/// 按标签候选取内容，取不到时按行号兜底（1 起，0 表示不兜底）。
pub fn pick<'a>(rows: &'a [Row], labels: &[&str], fallback: usize) -> &'a str {
    let hit = labels.iter().find_map(|label| {
        let target = label_key(label);
        rows.iter()
            .find(|row| label_key(&row.label).contains(&target))
            .map(|row| row.value.as_str())
    });
    hit.or_else(|| {
        let index = fallback.checked_sub(1)?;
        rows.get(index).map(|row| row.value.as_str())
    })
    .unwrap_or_default()
}

/// 归一化标签：去掉空白与冒号，统一小写，便于跨语言匹配。
fn label_key(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, ':' | '：' | '·'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// 日期统一成 `YYYY-MM-DD`；找不到年月日时返回空串。
pub fn normalize_date(raw: &str) -> String {
    let numbers: Vec<&str> = raw
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .collect();

    let found = numbers
        .windows(3)
        .find(|parts| parts[0].len() == 4 && parts[1].len() <= 2 && parts[2].len() <= 2);
    let Some(parts) = found else {
        return String::new();
    };
    let (Ok(month), Ok(day)) = (parts[1].parse::<u32>(), parts[2].parse::<u32>()) else {
        return String::new();
    };
    format!("{}-{month:02}-{day:02}", parts[0])
}
