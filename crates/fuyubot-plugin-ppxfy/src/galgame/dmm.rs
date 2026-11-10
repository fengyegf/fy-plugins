//! DMM 作品页解析。
//!
//! 两套模板：付费作品页（`.productTitle` / `.slider-area` / `.contentsDetailTop|Bottom`）
//! 与同人页（`.m-productInfo` / `#fn-featureCarousel` / `.m-productInformation`）。

use scraper::{CaseSensitivity, ElementRef, Html};

use super::Work;
use super::dom::{self, Row};

/// 站内相对地址的补全前缀。
const BASE: &str = "https://www.dmm.co.jp";

/// 付费作品页。
pub fn product(html: &str) -> Work {
    let doc = Html::parse_document(html);

    let mut slides = dom::images(
        &doc,
        ".slider-area li.slick-slide:not(.slick-cloned) img",
        BASE,
    );
    if slides.is_empty() {
        // 没跑 JS 的页面里没有 slick 生成的 class，退回整块轮播。
        slides = dom::images(&doc, ".slider-area img", BASE);
    }

    let top = class_rows(&doc, "contentsDetailTop");
    let bottom = class_rows(&doc, "contentsDetailBottom");

    Work {
        title: dom::text(&doc, ".productTitle h1"),
        cover: slides.first().cloned().unwrap_or_default(),
        images: slides.into_iter().skip(1).collect(),
        release_date: dom::normalize_date(dom::pick(
            &bottom,
            &["配信開始日", "発売日", "販売日", "配信日", "Release date"],
            2,
        )),
        genre: dom::pick(
            &bottom,
            &["ゲームジャンル", "Game genre", "ジャンル", "Genre"],
            7,
        )
        .to_owned(),
        brand: dom::pick(&top, &["ブランド", "Brand", "メーカー"], 3).to_owned(),
        video: None,
    }
}

/// 同人页：比付费作品页多一个预览视频，图片要按值去重。
pub fn doujin(html: &str) -> Work {
    let doc = Html::parse_document(html);

    let slides = dom::images(&doc, "#fn-featureCarousel .productPreview__item img", BASE);
    let cover = slides.first().cloned().unwrap_or_default();
    // 轮播里第一项是视频、它的按钮图就是封面，所以按值去重而不是按下标跳。
    let previews = slides
        .iter()
        .filter(|url| **url != cover)
        .cloned()
        .collect();
    let video = dom::attr(&doc, "#fn-featureCarousel video source", "src")
        .or_else(|| dom::attr(&doc, "#fn-featureCarousel video", "src"))
        .map(|raw| dom::image_url(BASE, &raw))
        .filter(|url| !url.is_empty());

    let rows = info_rows(&doc);
    Work {
        title: dom::text(&doc, ".m-productInfo .productTitle h1"),
        cover,
        images: previews,
        release_date: dom::normalize_date(dom::pick(
            &rows,
            &["配信開始日", "発売日", "販売日", "Release date"],
            1,
        )),
        genre: dom::pick(
            &rows,
            &["ゲームジャンル", "Game genre", "ジャンル", "Genre"],
            5,
        )
        .to_owned(),
        brand: dom::text(&doc, ".m-circleInfo .circleName a"),
        video,
    }
}

/// `.xxx__tableRow` 形式的属性行（付费作品页）。
fn class_rows(doc: &Html, prefix: &str) -> Vec<Row> {
    dom::elements(doc, &format!(".{prefix}__tableRow"))
        .into_iter()
        .map(|row| Row {
            label: class_text(&row, &format!("{prefix}__tableDataLeft")),
            value: class_text(&row, &format!("{prefix}__tableDataRight")),
        })
        .collect()
}

/// `.informationList` 形式的信息行（同人页）。
fn info_rows(doc: &Html) -> Vec<Row> {
    dom::elements(doc, ".m-productInformation .informationList")
        .into_iter()
        .map(|row| Row {
            label: class_text(&row, "informationList__ttl"),
            value: class_text(&row, "informationList__txt"),
        })
        .collect()
}

/// 直接子元素里第一个指定 class 的文本。
fn class_text(row: &ElementRef, class: &str) -> String {
    row.child_elements()
        .find(|child| {
            child
                .value()
                .has_class(class, CaseSensitivity::CaseSensitive)
        })
        .map(|child| dom::clean(&child.text().collect::<String>()))
        .unwrap_or_default()
}
