//! 把 GitHub Trending 页面的 HTML 解析成榜单条目。
//!
//! 每条仓库是一个 article.Box-row 块，块内依次是：
//! 仓库链接 h2 > a[href=/owner/name]、简介 p.col-9、语言色块 span.repo-language-color、
//! 语言名 span[itemprop=programmingLanguage]、star 数 a[href=.../stargazers]、
//! fork 数 a[href=.../forks]、增量 span.float-sm-right。
//!
//! 不引 HTML 解析库（插件只有 ureq / serde_json / thiserror）：按块 + 定位串提取，
//! 对缺字段的条目宽容（缺语言 / 缺简介都保留），整块连仓库名都拼不出来才跳过。
//!
//! 定位串都只依赖 GitHub 稳定下来的结构（class 名、href 后缀、itemprop），
//! 同一标签里新增 data-* 属性（页面就常常带 data-view-component）也不影响。

use crate::core::TrendingItem;
use crate::core::model::squeeze;

/// 榜单块开始标签的名字部分。
const ARTICLE_OPEN: &str = "<article";
/// 只有带这个 class 的 article 才是榜单块。
const ARTICLE_CLASS: &str = "Box-row";
/// 榜单块结束标签。
const ARTICLE_CLOSE: &str = "</article>";

/// 解析整页 HTML，返回榜单条目（顺序与页面一致）。
pub fn parse_items(html: &str) -> Vec<TrendingItem> {
    let mut items = Vec::new();
    let mut cursor = 0;
    while let Some(content_start) = next_article_open(html, cursor) {
        let Some(close) = find_from(html, ARTICLE_CLOSE, content_start) else {
            break;
        };
        if let Some(item) = parse_article(&html[content_start..close]) {
            items.push(item);
        }
        cursor = close + ARTICLE_CLOSE.len();
    }
    items
}

/// 找下一个 article.Box-row 块的开始位置，返回开始标签之后的内容起点。
///
/// 从 from 起逐个试 article 开始标签；不是 Box-row 的（页面别处也有 article）
/// 跳过。标签没闭合就当作页面结束。
fn next_article_open(html: &str, from: usize) -> Option<usize> {
    let mut cursor = from;
    while let Some(start) = find_from(html, ARTICLE_OPEN, cursor) {
        let after_name = start + ARTICLE_OPEN.len();
        // 标签名后面必须是空白或 >，否则是 <articles> 这类长名字。
        let boundary = html[after_name..].chars().next()?;
        if !(boundary.is_whitespace() || boundary == '>') {
            cursor = after_name;
            continue;
        }
        let tag_end = find_from(html, ">", after_name)?;
        if html[start..tag_end].contains(ARTICLE_CLASS) {
            return Some(tag_end + 1);
        }
        cursor = tag_end + 1;
    }
    None
}

/// 解析一个榜单块；连 owner/name 都拿不到就返回 None（整块跳过）。
fn parse_article(block: &str) -> Option<TrendingItem> {
    let (owner, name) = repo_identity(block)?;
    Some(TrendingItem {
        owner,
        name,
        description: description(block).unwrap_or_default(),
        language: language(block).unwrap_or_default(),
        language_color: language_color(block).unwrap_or_default(),
        stars: linked_count(block, "/stargazers").unwrap_or(0),
        forks: linked_count(block, "/forks").unwrap_or(0),
        gained: gained(block).unwrap_or(0),
        // 头像要联网抓，解析层不碰网络，交给 core::avatar 回填。
        avatar: String::new(),
    })
}

/// 从 h2 里的仓库链接取 owner 与 name。
fn repo_identity(block: &str) -> Option<(String, String)> {
    let heading = find_from(block, "<h2", 0)?;
    let href_start = find_from(block, "href=\"", heading)? + "href=\"".len();
    let href_end = find_from(block, "\"", href_start)?;
    split_repo_path(&block[href_start..href_end])
}

/// 把 href 切成 owner / name：只认前两段，查询串与锚点忽略。
fn split_repo_path(path: &str) -> Option<(String, String)> {
    let path = path.split(['?', '#']).next()?.trim().trim_matches('/');
    let mut segments = path.split('/');
    let owner = segments.next()?.trim();
    let name = segments.next()?.trim();
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((unescape(owner), unescape(name)))
}

/// 简介：p.col-9 的纯文本（剥标签 + 反转义 + 压空白）；没有就返回 None。
fn description(block: &str) -> Option<String> {
    let marker = find_from(block, "<p class=\"col-9", 0)?;
    let start = find_from(block, ">", marker)? + 1;
    let end = find_from(block, "</p>", start)?;
    text_of(&block[start..end])
}

/// 语言名：span[itemprop=programmingLanguage] 的文本。
fn language(block: &str) -> Option<String> {
    let marker = find_from(block, "itemprop=\"programmingLanguage\"", 0)?;
    let start = find_from(block, ">", marker)? + 1;
    let end = find_from(block, "<", start)?;
    let text = unescape(block[start..end].trim());
    if text.is_empty() { None } else { Some(text) }
}

/// 语言色值：repo-language-color 那个 span 标签里 background-color 后面的 #RRGGBB。
fn language_color(block: &str) -> Option<String> {
    let marker = find_from(block, "repo-language-color", 0)?;
    let tag_start = block[..marker].rfind("<span")?;
    let tag_end = find_from(block, ">", marker)?;
    let tag = &block[tag_start..tag_end];
    let background = find_from(tag, "background-color", 0)?;
    let hash = find_from(tag, "#", background)?;
    let color: String = tag[hash..]
        .chars()
        .take_while(|ch| *ch == '#' || ch.is_ascii_hexdigit())
        .collect();
    if color.len() > 1 { Some(color) } else { None }
}

/// star / fork 数：href 以 suffix 结尾的链接，剥掉 svg 之后链接文本里的数字。
fn linked_count(block: &str, suffix: &str) -> Option<u64> {
    let needle = format!("{suffix}\"");
    let marker = find_from(block, &needle, 0)?;
    let href_end = marker + needle.len();
    let tag_end = find_from(block, ">", href_end)?;
    let content_start = tag_end + 1;
    let content_end = find_from(block, "</a>", content_start)?;
    let text = strip_tags(&block[content_start..content_end]);
    numbers(&text).last().copied()
}

/// 本周期新增 star：span.float-sm-right 里的 1,234 stars today/this week/this month。
///
/// 这个 span 里先嵌一个 star 图标的 svg，所以必须剥标签，再取 stars 前面的数字。
fn gained(block: &str) -> Option<u64> {
    let marker = find_from(block, "float-sm-right", 0)?;
    let tag_start = block[..marker].rfind("<span")?;
    let tag_end = find_from(block, ">", marker)?;
    // float-sm-right 必须就在这个 span 的开始标签里，别把别处的同名 class 当增量。
    if !block[tag_start..tag_end].starts_with("<span") {
        return None;
    }
    let content_start = tag_end + 1;
    let content_end = find_from(block, "</span>", content_start)?;
    let text = strip_tags(&block[content_start..content_end]);
    // "790 stars today"：数字在 stars 前面；没有 stars 字样的块不算增量。
    let stars = find_from(&text, "star", 0)?;
    numbers(&text[..stars]).last().copied()
}

/// 剥掉 HTML 标签，标签位置补一个空格（避免相邻文本粘在一起）。
///
/// 引号里的 >（属性值里可能出现）不当标签结束，按简单的状态机扫。
fn strip_tags(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    let mut quote: Option<char> = None;
    for ch in fragment.chars() {
        if in_tag {
            match ch {
                '"' | '\'' => {
                    if quote == Some(ch) {
                        quote = None;
                    } else if quote.is_none() {
                        quote = Some(ch);
                    }
                }
                '>' if quote.is_none() => {
                    in_tag = false;
                    out.push(' ');
                }
                _ => {}
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        out.push(ch);
    }
    out
}

/// 常用 HTML 实体反转义：命名实体 + 十进制 / 十六进制数字实体。
///
/// 不认识或没闭合的实体原样保留（宁可显示 &bogus; 也不吞字符）。
fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find('&') {
        let amp = cursor + offset;
        out.push_str(&text[cursor..amp]);

        // 实体名最长按 10 个字符找分号，避免把后面的正常文本当实体。
        let mut entity = String::new();
        let mut closed = None;
        for (index, ch) in text[amp + 1..].char_indices().take(10) {
            if ch == ';' {
                closed = Some(amp + 1 + index);
                break;
            }
            if ch.is_ascii_alphanumeric() || ch == '#' {
                entity.push(ch);
            } else {
                break;
            }
        }
        match closed.and_then(|end| decode_entity(&entity).map(|decoded| (decoded, end))) {
            Some((decoded, end)) => {
                out.push_str(&decoded);
                cursor = end + 1;
            }
            None => {
                out.push('&');
                cursor = amp + 1;
            }
        }
    }
    out.push_str(&text[cursor..]);
    out
}

/// 单个实体名解码；未知返回 None。
fn decode_entity(entity: &str) -> Option<String> {
    if let Some(number) = entity.strip_prefix('#') {
        let (radix, digits) = match number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            Some(hex) => (16, hex),
            None => (10, number),
        };
        if digits.is_empty() {
            return None;
        }
        let value = u32::from_str_radix(digits, radix).ok()?;
        return char::from_u32(value).map(|ch| ch.to_string());
    }
    let decoded = match entity {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        "mdash" => "\u{2014}",
        "ndash" => "\u{2013}",
        "hellip" => "\u{2026}",
        "middot" => "\u{00b7}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "copy" => "\u{00a9}",
        "reg" => "\u{00ae}",
        "trade" => "\u{2122}",
        "times" => "\u{00d7}",
        _ => return None,
    };
    Some(decoded.to_owned())
}

/// 一段 HTML 片段 → 干净文本（剥标签、反转义、压空白）；空文本返回 None。
fn text_of(fragment: &str) -> Option<String> {
    let text = squeeze(&unescape(&strip_tags(fragment)));
    if text.is_empty() { None } else { Some(text) }
}

/// 取文本里的所有数字：千分位逗号被忽略，其他非数字字符是分隔符。
fn numbers(text: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if ch == ',' && !digits.is_empty() {
            // 千分位：继续累积。
        } else {
            flush_number(&mut digits, &mut out);
        }
    }
    flush_number(&mut digits, &mut out);
    out
}

/// 把当前累积的数字串落到结果里；溢出（异常超长数字）就丢弃。
fn flush_number(digits: &mut String, out: &mut Vec<u64>) {
    if !digits.is_empty() {
        if let Ok(value) = digits.parse::<u64>() {
            out.push(value);
        }
        digits.clear();
    }
}

/// 从 from 开始找子串，返回绝对位置。
fn find_from(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..].find(needle).map(|offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO_HEAD: &str =
        "<h2 class=\"h3 lh-condensed\"><a href=\"/rust-lang/rust\">rust</a></h2>";

    fn article(inner: &str) -> String {
        format!("<article class=\"Box-row\">{inner}</article>")
    }

    fn stargazers_link(count: &str) -> String {
        format!(
            "<a href=\"/rust-lang/rust/stargazers\" class=\"Link\"><svg><path d=\"M8 .25\"></path></svg>\n  {count}</a>"
        )
    }

    fn forks_link(count: &str) -> String {
        format!(
            "<a href=\"/rust-lang/rust/forks\" class=\"Link\"><svg><path d=\"M5 5.372\"></path></svg>\n  {count}</a>"
        )
    }

    #[test]
    fn empty_or_unrelated_html_has_no_items() {
        assert!(parse_items("").is_empty());
        assert!(parse_items("<html><body>nothing here</body></html>").is_empty());
        assert!(
            parse_items("<articles><article class=\"Box-row\"></article></articles>").is_empty()
        );
    }

    #[test]
    fn minimal_article_keeps_repository_and_zeroes_missing_fields() {
        let html = article(&format!(
            "{REPO_HEAD}{}{}",
            stargazers_link("7"),
            forks_link("3")
        ));
        let items = parse_items(&html);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.full_name(), "rust-lang/rust");
        assert_eq!(item.stars, 7);
        assert_eq!(item.forks, 3);
        assert_eq!(item.gained, 0);
        assert_eq!(item.description, "");
        assert_eq!(item.language, "");
        assert_eq!(item.language_color, "");
    }

    #[test]
    fn full_article_parses_every_field() {
        let html = article(concat!(
            "<h2 class=\"h3 lh-condensed\"><a data-view-component=\"true\" href=\"/ag-ui-protocol/ag-ui\">ag-ui</a></h2>\n",
            "<p class=\"col-9 color-fg-muted my-1 tmp-pr-4\">\n  AG-UI &amp; friends &lt;beta&gt; ",
            "<a href=\"https://x.test\">x.test</a>\n</p>\n",
            "<span class=\"tmp-mr-3 d-inline-block\"><span class=\"repo-language-color\" ",
            "style=\"background-color: #3178c6\"></span>",
            "<span itemprop=\"programmingLanguage\">TypeScript</span></span>\n",
            "<a href=\"/ag-ui-protocol/ag-ui/stargazers\" class=\"Link\">",
            "<svg><path d=\"M8 .25\"></path></svg>\n  18,364</a>\n",
            "<a href=\"/ag-ui-protocol/ag-ui/forks\" class=\"Link\">",
            "<svg><path d=\"M5 5.372\"></path></svg>\n  1,892</a>\n",
            "<span data-view-component=\"true\" class=\"d-inline-block float-sm-right\">",
            "<svg><path d=\"M8 .25\"></path></svg>\n  1,234 stars today\n</span>"
        ));
        let items = parse_items(&html);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.owner, "ag-ui-protocol");
        assert_eq!(item.name, "ag-ui");
        assert_eq!(item.description, "AG-UI & friends <beta> x.test");
        assert_eq!(item.language, "TypeScript");
        assert_eq!(item.language_color, "#3178c6");
        assert_eq!(item.stars, 18_364);
        assert_eq!(item.forks, 1_892);
        assert_eq!(item.gained, 1_234);
    }

    #[test]
    fn gained_accepts_today_week_and_month_wording() {
        for (text, expected) in [
            ("790 stars today", 790),
            ("5,692 stars this week", 5_692),
            ("25,448 stars this month", 25_448),
        ] {
            let html = article(&format!(
                "{REPO_HEAD}<span class=\"d-inline-block float-sm-right\">{text}</span>"
            ));
            assert_eq!(parse_items(&html)[0].gained, expected, "{text}");
        }
    }

    #[test]
    fn articles_are_parsed_in_order_and_broken_ones_skipped() {
        let html = format!(
            "{}{}{}",
            article(&format!("{REPO_HEAD}{}", stargazers_link("10"))),
            article("<h2>no link</h2>"),
            article("<h2 class=\"h3 lh-condensed\"><a href=\"/other/repo\">repo</a></h2>"),
        );
        let items = parse_items(&html);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].full_name(), "rust-lang/rust");
        assert_eq!(items[0].stars, 10);
        assert_eq!(items[1].full_name(), "other/repo");
    }

    #[test]
    fn thousands_separators_are_parsed() {
        assert_eq!(numbers("18,364"), vec![18_364]);
        assert_eq!(numbers("1,234,567"), vec![1_234_567]);
        assert_eq!(numbers("790"), vec![790]);
        assert_eq!(numbers("5,692 stars"), vec![5_692]);
        assert_eq!(numbers("no digits"), Vec::<u64>::new());
    }

    #[test]
    fn entities_are_unescaped() {
        assert_eq!(
            unescape("a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#x27;"),
            "a & b <c> \"d\" 'e'"
        );
        assert_eq!(unescape("unknown &bogus; stays"), "unknown &bogus; stays");
        assert_eq!(unescape("plain"), "plain");
        assert_eq!(unescape("trailing &"), "trailing &");
    }

    #[test]
    fn tags_are_stripped_without_eating_text() {
        assert_eq!(squeeze(&strip_tags("<b>a</b> <a href=\"/x\">b</a>")), "a b");
        assert_eq!(squeeze(&strip_tags("<a title=\"a>b\">x</a>")), "x");
    }

    #[test]
    fn repo_path_requires_two_segments() {
        assert_eq!(
            split_repo_path("/owner/name?tab=readme"),
            Some(("owner".to_owned(), "name".to_owned()))
        );
        assert_eq!(
            split_repo_path("/owner/name#readme"),
            Some(("owner".to_owned(), "name".to_owned()))
        );
        assert_eq!(split_repo_path("/owner"), None);
        assert_eq!(split_repo_path("/"), None);
    }
}
