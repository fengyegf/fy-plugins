//! DLSite 作品页解析：`#work_name` / `.product-slider-data` / `#work_outline`。

use scraper::{ElementRef, Html};

use super::Work;
use super::dom::{self, Row};

/// 站内相对地址的补全前缀。
const BASE: &str = "https://www.dlsite.com";

/// 解析作品页。
pub fn parse(html: &str) -> Work {
    let doc = Html::parse_document(html);

    let title = dom::first(&[
        dom::text(&doc, "#work_name"),
        dom::attr(&doc, "meta[property=\"og:title\"]", "content").unwrap_or_default(),
        dom::text(&doc, "title"),
    ]);
    let title = strip_site_suffix(&title);

    // 轮播地址在 `.product-slider-data` 子 div 的 data-src 上（Vue 模板，未渲染时没有 li/img）。
    let mut slides = dom::images(&doc, ".product-slider-data > div[data-src]", BASE);
    if slides.is_empty() {
        slides = dom::images(&doc, "li.slider_item img", BASE);
    }

    let cover = slides.first().cloned().unwrap_or_else(|| {
        dom::attr(&doc, "#work_left meta[itemprop=\"image\"]", "content")
            .map(|raw| dom::image_url(BASE, &raw))
            .unwrap_or_default()
    });

    let rows = table_rows(&doc);
    let brand = dom::first(&[
        dom::text(&doc, "#work_maker tbody tr td span a"),
        dom::text(&doc, "#work_maker a"),
        dom::pick(
            &rows,
            &[
                "品牌名",
                "ブランド名",
                "サークル名",
                "Circle",
                "Brand",
                "品牌",
                "メーカー",
            ],
            0,
        )
        .to_owned(),
    ]);

    Work {
        title,
        cover,
        images: slides.into_iter().skip(1).collect(),
        release_date: dom::normalize_date(dom::pick(
            &rows,
            &[
                "发售时间",
                "販売日",
                "发售日",
                "発売日",
                "Release date",
                "Release",
            ],
            1,
        )),
        genre: dom::pick(
            &rows,
            &[
                "游戏类型",
                "ゲームジャンル",
                "作品形式",
                "Game genre",
                "Genre",
                "ジャンル",
                "类型",
            ],
            7,
        )
        .to_owned(),
        brand,
        video: None,
    }
}

/// `#work_outline` 的 `th` / `td` 信息行。
fn table_rows(doc: &Html) -> Vec<Row> {
    dom::elements(doc, "#work_outline tr")
        .into_iter()
        .map(|row| Row {
            label: tag_text(&row, "th"),
            value: tag_text(&row, "td"),
        })
        .filter(|row| !row.value.is_empty())
        .collect()
}

/// 直接子元素里第一个指定标签的文本。
fn tag_text(row: &ElementRef, tag: &str) -> String {
    row.child_elements()
        .find(|child| child.value().name() == tag)
        .map(|child| dom::clean(&child.text().collect::<String>()))
        .unwrap_or_default()
}

/// `<title>` 形如「作品名 | DLsite 同人 - R18」，取竖线前那段。
fn strip_site_suffix(title: &str) -> String {
    match title.find('|') {
        Some(index)
            if title[index + 1..]
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("dlsite") =>
        {
            title[..index].trim_end().to_owned()
        }
        _ => title.to_owned(),
    }
}
