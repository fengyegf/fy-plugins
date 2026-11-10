//! galgame 作品信息：站点判定、页面解析、成品消息文本。
//!
//! 按职责拆文件：
//!
//! - [`dmm`] / [`dlsite`]：各站点一套解析，输出同一个 [`Work`]；
//! - [`dom`]：两侧共用的 HTML 取值小工具；
//! - [`text`]：核对结果 [`Info`] 与成品消息文本。
//!
//! 命令层（`crate::cmd::galgame`）只负责流程与交互，不碰 HTML 与排版细节。

mod dlsite;
mod dmm;
mod dom;
mod text;

use url::Url;

pub use text::{Info, Link, platform_name, share_text, split_commas, split_genres};

/// 解析出来的作品信息。
#[derive(Debug, Default, Clone)]
pub struct Work {
    /// 作品标题。
    pub title: String,
    /// 封面图地址。
    pub cover: String,
    /// 预览图（不含封面，按页面顺序）。
    pub images: Vec<String>,
    /// 发售时间，`YYYY-MM-DD`，解析失败时为空。
    pub release_date: String,
    /// 游戏类型原始文本（如 `ADV`），核对阶段再拆。
    pub genre: String,
    /// 品牌 / 社团名。
    pub brand: String,
    /// 预览视频（仅 DMM 同人页有）。
    pub video: Option<String>,
}

/// 作品页所属站点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    Dmm,
    Dlsite,
}

impl Site {
    /// 按链接判断站点；不认识的链接返回 `None`。
    pub fn detect(url: &str) -> Option<Self> {
        let host = Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
            // 不是合法 URL 时退回整串文本匹配。
            .unwrap_or_else(|| url.to_ascii_lowercase());

        if covers(&host, "dmm.co.jp") || covers(&host, "dmm.com") {
            Some(Self::Dmm)
        } else if covers(&host, "dlsite.com") {
            Some(Self::Dlsite)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dmm => "DMM",
            Self::Dlsite => "DLSite",
        }
    }
}

/// `host` 就是该域名、或它的子域；整串兜底匹配也算。
fn covers(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}")) || host.contains(domain)
}

/// 解析作品页。`url` 用来区分 DMM 的同人模板与普通模板。
pub fn parse(site: Site, html: &str, url: &str) -> Work {
    match site {
        Site::Dlsite => dlsite::parse(html),
        // 同人页是另一套模板，多一个预览视频。
        Site::Dmm if url.contains("/dc/doujin/") => dmm::doujin(html),
        Site::Dmm => dmm::product(html),
    }
}
