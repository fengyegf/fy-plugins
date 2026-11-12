//! 业务模型 → Vue 模板 `data` 的映射（纯函数，不碰渲染引擎）。
//!
//! 界面在 [`assets/card.html`](../../assets/card.html)：卡片、徽章、日期行、
//! 循环与条件全在模板里；这里只把 Epic 接口的数据整理成模板认识的结构：
//!
//! ```json
//! {
//!   "width": 1228, "padding": 24, "cardWidth": 280, "coverHeight": 157, "gap": 20,
//!   "title": "免费游戏", "emptyText": "当前没有免费游戏",
//!   "background": "rgba(32,32,36,1.000)",
//!   "present": [{ "title": "…", "startDate": "…", "endDate": "…", "offerImage": "https://…" }],
//!   "Coming":  [{ "title": "…", "startDate": "…", "endDate": "…", "offerImage": "" }]
//! }
//! ```
//!
//! 数据键沿用 web 版 `epic.vue` 的 props：`present` / `Coming`（注意大写 C），
//! 每项 `{title, startDate, endDate, offerImage}`。
//!
//! 模板表达式只支持「字面量 / 变量 / 成员访问 / 运算 / 三元」，没有函数调用、
//! 没有 `Math` / `Date`，也不支持 `?.` / `??`。所以「标题按宽度截断」
//! 「封面 URL 能不能直接交给渲染器」这类判断必须先在这里算好，模板只读字段。

use serde_json::{Value, json};

use crate::core::epic::{FreeGame, FreeGames};
use crate::render::{Layout, RenderOptions};

/// 标题字号（CSS px），必须与模板里的 `text-sm`（0.875rem = 14px）一致：
/// 截断预算按它计算，字号改了这里也要改，否则两行放不下或过早截断。
pub const TITLE_FONT_SIZE: u32 = 14;
/// 标题最多展示几行（模板没有 line-clamp，靠这里截断保证不撑破画布）。
pub const TITLE_MAX_LINES: usize = 2;

/// 计划好的卡片：当前免费 / 即将免费，已按 `max_cards` 截断。
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// 当前免费（模板里的 `present`）。
    pub present: Vec<Value>,
    /// 即将免费（模板里的 `Coming`）。
    pub coming: Vec<Value>,
}

impl Plan {
    /// 总卡片数。
    pub fn len(&self) -> usize {
        self.present.len() + self.coming.len()
    }

    /// 没有任何卡片（模板走空列表提示）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 把游戏列表整理成模板项：截断到 `max_cards`，当前免费优先。
pub fn plan(games: &FreeGames, options: &RenderOptions) -> Plan {
    let limit = options.max_cards.max(1);
    let present: Vec<&FreeGame> = games.current.iter().take(limit).collect();
    let left = limit - present.len();
    let coming: Vec<&FreeGame> = games.upcoming.iter().take(left).collect();
    Plan {
        present: present
            .into_iter()
            .map(|game| item(game, options))
            .collect(),
        coming: coming.into_iter().map(|game| item(game, options)).collect(),
    }
}

/// 一项卡片的模板数据。
fn item(game: &FreeGame, options: &RenderOptions) -> Value {
    json!({
        "title": fit_title(&game.title, title_budget(options.card_width)),
        "startDate": game.start_date,
        "endDate": game.end_date,
        // 模板用 v-if 判断有没有封面：空串 = 没有可直接交给渲染器的封面。
        "offerImage": usable_cover(game).unwrap_or_default(),
    })
}

/// 整份模板数据：卡片列表 + 版式参数（宽度、间距、文案、背景色）。
pub fn view_data(plan: &Plan, layout: &Layout, options: &RenderOptions) -> Value {
    json!({
        "width": layout.width,
        "padding": options.padding,
        "cardWidth": options.card_width.max(1),
        "coverHeight": options.cover_height.max(1),
        "gap": options.gap,
        "title": options.title,
        "emptyText": options.empty_text,
        "background": css_color(options.background),
        "present": plan.present,
        "Coming": plan.coming,
    })
}

/// 封面 URL 是否可以直接交给 html2img。
///
/// 服务只加载公网 http(s) 与 data: 图片，并且会拒绝私网 / 回环地址；这里提前把
/// 不可用的 URL 筛掉，让那张卡退回深灰占位，而不是让整次渲染失败（一个坏 URL
/// 不该让整张图都出不来）。只接受小写 https:// 前缀：Epic 的 CDN 都是 https，
/// 接口也不会给出别的协议。
fn usable_cover(game: &FreeGame) -> Option<&str> {
    let url = game.offer_image.as_deref()?.trim();
    if url.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.split(':').next().unwrap_or_default();
    host_is_public(host).then_some(url)
}

/// 主机名是否像公网地址：拒绝空串、localhost、私网 / 回环 / 链路本地 IP，
/// 以及不带点的单段主机名（内网习惯，服务侧也会拒）。
fn host_is_public(host: &str) -> bool {
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(ip) => {
                !(ip.is_private()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || ip.is_unspecified()
                    || ip.is_broadcast())
            }
            std::net::IpAddr::V6(ip) => !(ip.is_loopback() || ip.is_unspecified()),
        };
    }
    host.contains('.')
}

/// 标题的宽度预算，单位是「全角字符数」：两行，每行 = floor(卡片宽 / 字号)。
///
/// 标题与卡片同宽，280px / 14px = 20，一行 20 个全角字；取 floor 留一点余量，
/// 宁可早点截断也不要让第三行被裁掉。
fn title_budget(card_width: u32) -> f32 {
    let per_line = card_width.max(1) / TITLE_FONT_SIZE.max(1);
    (per_line.max(1) * TITLE_MAX_LINES as u32) as f32
}

/// 按「字符宽度」截断标题并补省略号。
///
/// 不依赖 CSS 的 text-overflow / line-clamp（渲染器不一定支持，两行溢出的表现
/// 也不可控）；这里按全角/半角宽度估算截断点，保证不会超出标题区。
pub fn fit_title(title: &str, budget: f32) -> String {
    if budget <= 0.0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0.0f32;
    let mut truncated = false;

    for ch in title.chars() {
        let width = char_width(ch);
        if used + width > budget {
            truncated = true;
            break;
        }
        used += width;
        out.push(ch);
    }

    if truncated {
        // 给省略号腾出 1 个全角宽度的位置。
        while used + 1.0 > budget {
            let Some(ch) = out.pop() else { break };
            used -= char_width(ch);
        }
        out.push('…');
    }
    out
}

/// 单字符宽度（单位：全角字符 = 1.0；ASCII 约 0.55）。
fn char_width(ch: char) -> f32 {
    if is_wide(ch) { 1.0 } else { 0.55 }
}

/// 是否宽字符（CJK / 全角标点 / 韩文 / 假名 / 中西文混排里按全角渲染的破折号）。
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F
        | 0x2013..=0x2014
        | 0x2026
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD
    )
}

/// [r, g, b, a] 转成 CSS rgba(...)（模板用它当画布背景）。
fn css_color(rgba: [u8; 4]) -> String {
    format!(
        "rgba({},{},{},{:.3})",
        rgba[0],
        rgba[1],
        rgba[2],
        rgba[3] as f32 / 255.0
    )
}
