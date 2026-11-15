//! DLSite 作品页解析。
//!
//! 页面改版后是 Nuxt SSR：结构化数据放在 `application/ld+json` 的 `Product` 节点里，
//! 图片（`image`，首张是封面）与品牌（`brand.name`）优先从那里读，抗改版；
//! DOM 选择器只按**新版**结构兜底。发售日 / 类型仍然只有 DOM 里有
//! （`.c-productInfoPc__list--outline` 的 `dt` / `dd`）。

use scraper::Html;
use serde_json::Value;

use super::Work;
use super::dom::{self, Row};

/// 站内相对地址的补全前缀。
const BASE: &str = "https://www.dlsite.com";

/// 新版图集：`#work_visual` 里的 keen-slider（`/resize/` 缩略图会被过滤）。
const GALLERY: &str = "#work_visual .key-visual-slider .keen-slider__slide img";

/// 新版信息表：每项是 `dt.c-productInfo__key` + `dd.c-productInfo__value`。
const OUTLINE: &str = ".c-productInfoPc__list--outline .c-productInfo__item";

/// 解析作品页。
pub fn parse(html: &str) -> Work {
    let doc = Html::parse_document(html);
    let product = product(&doc);

    let title = strip_site_suffix(&dom::first(&[
        json_text(product.as_ref(), "name"),
        dom::text(&doc, "#work_name"),
        dom::attr(&doc, "meta[property=\"og:title\"]", "content").unwrap_or_default(),
        dom::text(&doc, "title"),
    ]));

    let slides = gallery(&doc, product.as_ref());
    let cover = slides.first().cloned().unwrap_or_else(|| {
        dom::attr(&doc, "meta[property=\"og:image\"]", "content")
            .map(|raw| dom::image_url(BASE, &raw))
            .unwrap_or_default()
    });

    let rows = outline(&doc);
    Work {
        title,
        cover,
        images: slides.into_iter().skip(1).collect(),
        release_date: dom::normalize_date(dom::pick(
            &rows,
            &[
                "販売日",
                "配信日",
                "発売日",
                "発売時間",
                "Release date",
                "Release",
            ],
            0,
        )),
        genre: dom::pick(
            &rows,
            &["ジャンル", "ゲームジャンル", "作品形式", "Genre", "类型"],
            0,
        )
        .to_owned(),
        brand: dom::first(&[
            json_brand(product.as_ref()),
            dom::text(&doc, ".c-productInfo__item--maker a"),
        ]),
        video: None,
    }
}

/// 页面里 `application/ld+json` 的 `Product` 节点。
fn product(doc: &Html) -> Option<Value> {
    dom::elements(doc, "script[type=\"application/ld+json\"]")
        .into_iter()
        .filter_map(|node| serde_json::from_str::<Value>(&node.text().collect::<String>()).ok())
        .find(|value| value.get("@type").and_then(Value::as_str) == Some("Product"))
}

/// JSON-LD 里的字符串字段。
fn json_text(product: Option<&Value>, key: &str) -> String {
    product
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

/// JSON-LD 里的品牌名（`{"@type":"Brand","name":...}` 或直接是字符串）。
fn json_brand(product: Option<&Value>) -> String {
    let Some(brand) = product.and_then(|value| value.get("brand")) else {
        return String::new();
    };
    match brand {
        Value::String(name) => name.trim().to_owned(),
        Value::Object(map) => map
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        _ => String::new(),
    }
}

/// JSON-LD 里的图片列表（字符串数组，或 `{"url": ...}` 对象数组）。
fn json_images(product: &Value) -> Vec<String> {
    match product.get("image") {
        Some(Value::String(url)) => vec![url.trim().to_owned()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(url) => Some(url.trim().to_owned()),
                Value::Object(map) => map
                    .get("url")
                    .and_then(Value::as_str)
                    .map(|url| url.trim().to_owned()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// 图集：优先 JSON-LD，取不到再退回新版 DOM；去掉缩略图并按值去重。
fn gallery(doc: &Html, product: Option<&Value>) -> Vec<String> {
    let mut urls: Vec<String> = product.map(json_images).unwrap_or_default();
    if urls.is_empty() {
        urls = dom::images(doc, GALLERY, BASE);
    }

    let mut seen = std::collections::HashSet::new();
    urls.into_iter()
        .map(|raw| dom::image_url(BASE, &raw))
        .filter(|url| !url.is_empty() && !url.contains("/resize/"))
        .filter(|url| seen.insert(url.clone()))
        .collect()
}

/// 信息行：新版 `dt` / `dd` 成对取。
fn outline(doc: &Html) -> Vec<Row> {
    dom::elements(doc, OUTLINE)
        .iter()
        .map(|item| Row {
            label: dom::inner_text(item, "dt.c-productInfo__key"),
            value: dom::inner_text(item, "dd.c-productInfo__value"),
        })
        .filter(|row| !row.value.is_empty())
        .collect()
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
