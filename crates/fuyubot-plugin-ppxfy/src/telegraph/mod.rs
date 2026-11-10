//! Telegraph 页面：把作品的封面 / 视频 / 预览图发成一篇图文页。
//!
//! 只有「建页」这一件事：不解析页面、不发消息。走 [telegraph-rs]（async + reqwest），
//! 所以这里用一次性的 current-thread runtime 把它接进阻塞的业务线程。
//!
//! [telegraph-rs]: https://crates.io/crates/telegraph-rs

use std::collections::HashMap;

use telegraph_rs::{Node, NodeElement, Telegraph};
use url::Url;

use crate::core::config::TelegraphConfig;
use crate::galgame::Work;

/// 建页；失败（含没配 token）返回 `None`（命令层只做「失败就停」）。
pub fn create(work: &Work, config: &TelegraphConfig) -> Option<String> {
    // 空 token 打过去只会换回一个看不懂的 API 错误，先挡掉。
    if config.token.trim().is_empty() {
        return None;
    }
    let content = serde_json::to_string(&nodes(work)).ok()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;

    let page = runtime.block_on(async {
        // 客户端必须在 runtime 里建：reqwest 的连接池要挂到 reactor 上。
        let telegraph = Telegraph::new();
        let account = telegraph
            .account("ppxfy")
            .access_token(&config.token)
            .author_name(&config.author_name)
            .author_url(&config.author_url)
            .create()
            .await
            .ok()?;
        account.create_page(&work.title, &content, false).await.ok()
    })?;

    (!page.url.is_empty()).then_some(page.url)
}

/// 页面内容：封面 → 视频 → 预览图画廊。
fn nodes(work: &Work) -> Vec<Node> {
    let mut nodes = Vec::new();
    if !work.cover.is_empty() {
        nodes.push(figure(image(&absolute(&work.cover), &work.title)));
    }
    if let Some(video) = work.video.as_deref().filter(|link| !link.is_empty()) {
        nodes.push(figure(video_node(video)));
    }
    for (index, src) in work.images.iter().enumerate() {
        nodes.push(figure(image(&absolute(src), &format!("图{}", index + 1))));
    }
    nodes
}

/// Telegraph 只接受 `figure` 包裹的 img / video / iframe。
fn figure(node: Node) -> Node {
    element("figure", &[], vec![node])
}

fn image(src: &str, alt: &str) -> Node {
    element("img", &[("src", src), ("alt", alt)], Vec::new())
}

/// YouTube 用 embed 的 iframe，其它视频直接 `<video>`。
fn video_node(link: &str) -> Node {
    if link.contains("youtube.com") || link.contains("youtu.be") {
        let embed = format!(
            "https://telegra.ph/embed/youtube?url={}",
            encode(&youtube_watch_url(link))
        );
        return element(
            "iframe",
            &[
                ("src", embed.as_str()),
                ("width", "640"),
                ("height", "360"),
                ("frameborder", "0"),
                ("allowtransparency", "true"),
                ("allowfullscreen", "true"),
                ("scrolling", "no"),
            ],
            Vec::new(),
        );
    }
    element("video", &[("src", link), ("controls", "")], Vec::new())
}

fn element(tag: &str, attrs: &[(&str, &str)], children: Vec<Node>) -> Node {
    Node::NodeElement(NodeElement {
        tag: tag.to_owned(),
        attrs: (!attrs.is_empty()).then(|| {
            attrs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), Some((*value).to_owned())))
                .collect::<HashMap<_, _>>()
        }),
        children: (!children.is_empty()).then_some(children),
    })
}

/// 补全协议：`//x` → `https://x`。
fn absolute(url: &str) -> String {
    match url.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    }
}

/// 统一成 `watch?v=` 形式，Telegraph 的 YouTube 嵌入只认这个。
fn youtube_watch_url(link: &str) -> String {
    let Ok(parsed) = Url::parse(link) else {
        return link.to_owned();
    };
    let host = parsed.host_str().unwrap_or_default();

    if host == "youtu.be" {
        let id = parsed.path().trim_start_matches('/');
        return if id.is_empty() {
            link.to_owned()
        } else {
            format!("https://www.youtube.com/watch?v={id}")
        };
    }
    if host == "youtube.com" || host == "www.youtube.com" || host == "m.youtube.com" {
        if let Some((_, id)) = parsed.query_pairs().find(|(key, _)| key == "v") {
            return format!("https://www.youtube.com/watch?v={id}");
        }
        if let Some(id) = parsed
            .path()
            .strip_prefix("/embed/")
            .filter(|id| !id.is_empty())
        {
            return format!("https://www.youtube.com/watch?v={id}");
        }
    }
    link.to_owned()
}

/// 百分号编码。
fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}
