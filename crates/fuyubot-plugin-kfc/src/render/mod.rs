//! 图像生成：Vue 卡片模板 + 数据 → `fuyubot.html2img` 服务 → 本机图片文件。
//!
//! **界面不在 Rust 里拼**：整张卡片的 HTML / CSS / Vue 指令都在
//! [`assets/card.html`](../../assets/card.html) 里（`<style>` + Vue 模板），
//! 本模块只做三件事：
//!
//! 1. 把业务文案、宽度和互动数字凑成 Vue 模板的 `data`（[JSON](serde_json::Value)）；
//! 2. 调宿主上的 html2img 服务（`vue: true`），拿本机图片路径；
//! 3. 把错误翻译成插件层能展示的中文。
//!
//! 渲染器只有宿主那一份（hyper-render 的本地 fork：Blitz + Stylo + Taffy + Vello CPU），
//! 插件不再链接 hyper-render，也不再自己拼 HTML 字符串。
//!
//! 互动区的四个数字是**装饰**：每次出图现掷，在 Rust 里格式化成 X 的中文计数后再交给模板
//! （渲染器不执行 JS，模板只做插值）。
//!
//! html2img 是**硬依赖**（`Plugin::inject`）：没有它出不了图，插件停在 Pending
//! 比启动后每条命令都失败更容易排查。

pub mod html2img;

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

/// 卡片模板：`<style>` + Vue 模板，编译期嵌入。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 卡片宽度允许的范围。
///
/// 下限 460 不是拍脑袋：再窄内容列就放不下「头像 + KFC + 蓝标 + @句柄」，
/// 句柄会被迫在「刚/刚」这种地方断行，用户名行进入换行兜底后卡片底部
/// 还会多出一条空白带（实测 400–456 有 96–139px 空白）。上限 1600：
/// 再宽就不像推特卡片了。
const MIN_WIDTH: u32 = 460;
const MAX_WIDTH: u32 = 1600;

/// 交给 html2img 的视口高度下限：`auto_height` 时真实高度由内容撑开，
/// 这里给一个比卡片最小高度还小的值，避免短文案被留白撑高。
const VIEWPORT_HEIGHT: u32 = 100;

/// 单次 HTML 上限，与 html2img 默认的 `max_html_bytes`（128 KiB）对齐。
pub const MAX_HTML_BYTES: usize = 128 * 1024;

/// 渲染失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 渲染服务调用失败（服务不在 / 模板编译失败 / 渲染报错 / 应答不合法）。
    #[error("渲染服务失败: {0}")]
    Service(#[from] html2img::Error),
    /// 模板本身超过服务上限（编译期就该发现，留作兜底）。
    #[error("卡片模板 {0} 字节，超过 128 KiB 上限")]
    TemplateTooLarge(usize),
}

/// 把文案渲染成卡片图，返回 html2img 产出的本机文件路径；`width` 是卡片宽度（px）。
///
/// 调用方负责在用完之后删掉 `path`（html2img 自己会在 24 小时后清理）。
pub fn render_card(host: &HostApi, content: &str, width: u32) -> Result<PathBuf, Error> {
    if TEMPLATE.len() > MAX_HTML_BYTES {
        return Err(Error::TemplateTooLarge(TEMPLATE.len()));
    }
    let width = clamp_width(width);
    let data = view_data(content, width);
    let request = html2img::Request {
        html: TEMPLATE,
        // 卡片是 Vue 模板：循环 / 条件 / 插值都在模板里，Rust 只给数据。
        vue: true,
        data: Some(&data),
        width,
        // 短文案不被默认的 768px 视口留白撑高；长文案由 auto_height 撑开。
        height: Some(VIEWPORT_HEIGHT),
        // 与老版本地渲染一致：800 CSS px 宽、1:1 输出。
        scale: 1.0,
        // 白底卡片，JPEG 体积小、Telegram 照片上传够用。
        format: "jpg",
        auto_height: true,
        transparent: false,
        dark: false,
    };
    Ok(html2img::render(host, &request)?)
}

/// 把宽度夹到 `[MIN_WIDTH, MAX_WIDTH]`。
pub fn clamp_width(width: u32) -> u32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// 卡片内边距（模板里的 `padding:20px`）。
pub const CARD_PADDING: u32 = 20;
/// 卡片边框宽度（模板里的 `border:1px`）。
pub const CARD_BORDER: u32 = 1;
/// 头像边长（模板里的 `width/height:100px`）。
pub const AVATAR_SIZE: u32 = 100;
/// 头像右侧间距（模板里的 `margin-right:15px`）。
pub const AVATAR_GAP: u32 = 15;
/// 内容列右侧留白（模板里的 `margin-right:30px`）。
pub const CONTENT_MARGIN: u32 = 30;
/// 正文宽度占内容列的比例：模板原来是 `width:95%`。
const TEXT_WIDTH_NUM: u32 = 95;
const TEXT_WIDTH_DEN: u32 = 100;

/// 内容列（头像右边那一列）的宽度，单位 CSS 像素。
///
/// **必须算成确定值交给模板**：html2img 的渲染器不会按 `flex-grow` 把内容列
/// 收窄进卡片，而是取 max-content 宽度——正文和图标行会直接顶出卡片右侧
/// （线上表现：正文第一行被卡片右边缘切掉、五个互动图标只剩第一个可见）。
/// 这里把宽度算好，模板只读字段，不再依赖 flex 收缩。
pub fn content_width(width: u32) -> u32 {
    width.saturating_sub(
        2 * (CARD_BORDER + CARD_PADDING) + AVATAR_SIZE + AVATAR_GAP + CONTENT_MARGIN,
    )
}

/// 正文宽度：内容列的 95%，也就是 `white-space:pre-wrap` 的换行宽度。
pub fn text_width(width: u32) -> u32 {
    content_width(width) * TEXT_WIDTH_NUM / TEXT_WIDTH_DEN
}

/// 互动区的四个随机数字（原始值，模板里读到的是格式化后的字符串）。
///
/// 每次出图现掷：回复是四位数（带千分位），转帖 / 喜欢 / 查看在「万」上下浮动，
/// 由 [`format_count`] 统一成 X 的中文计数（4,584 / 1.2万 / 12万 / 368万）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// 回复数：300–19,999，可能带千分位，也可能落到「万」。
    pub reply: u64,
    /// 转帖数：900–99,999，可能落到「万」。
    pub repost: u64,
    /// 喜欢数：4,000–999,999，多半是「万」。
    pub like: u64,
    /// 查看数：30,000–9,999,999，一定是「万」。
    pub views: u64,
}

/// 随机掷一组互动数字。
pub fn random_counts() -> Counts {
    Counts {
        reply: random_between(300, 19_999),
        repost: random_between(900, 99_999),
        like: random_between(4_000, 999_999),
        views: random_between(30_000, 9_999_999),
    }
}

/// 进程内递增序号：同一纳秒内连续出图时也给随机种子补一点熵。
static RANDOM_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 取一个 [min, max] 内的随机整数。
///
/// 不为了四个装饰数字引 rand：xorshift64* 的分布足够，种子混了时间、栈地址
/// 和进程内序号，连续两次出图不会撞出同一组数字。
fn random_between(min: u64, max: u64) -> u64 {
    use std::sync::atomic::Ordering;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let stack = &nanos as *const u64 as u64;
    let seq = RANDOM_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut state =
        nanos.rotate_left(17) ^ stack.rotate_left(31) ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);

    // xorshift64*：先推进状态，再乘一个奇数做输出混合。
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    let value = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
    min + value % (max - min + 1)
}

/// 把互动数字格式化成 X 的中文计数：
///
/// - 小于 1 万：千分位（4,584）；
/// - 1 万–10 万：一位小数的「万」，按 X 的做法截断不四舍五入（1.2万）；
/// - 10 万以上：整数「万」（12万 / 368万）。
pub fn format_count(value: u64) -> String {
    if value < 10_000 {
        let digits = value.to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3);
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                out.push(',');
            }
            out.push(digit);
        }
        return out;
    }

    let wan = value / 10_000;
    if wan >= 10 {
        return format!("{wan}万");
    }
    // 1.0 万到 9.9 万：取一位小数，直接截断（19,258 → 1.9万，与 X 一致）。
    let tenth = (value % 10_000) / 1_000;
    if tenth == 0 {
        format!("{wan}万")
    } else {
        format!("{wan}.{tenth}万")
    }
}

/// 互动行的基准：800 宽卡片（内容列 613px）下图标 32px、数字 22px。
const ACTION_ICON_AT_FULL: u32 = 32;
const ACTION_FONT_AT_FULL: u32 = 22;
/// 窄卡片的兜底下限：图标 / 数字再小就看不清了。
const ACTION_ICON_MIN: u32 = 16;
const ACTION_FONT_MIN: u32 = 10;
/// 800 宽卡片的内容列宽度：互动行按它做等比缩放的基准。
const ACTION_BASE_CONTENT: u32 = 613;

/// 互动图标边长：跟随内容列宽度等比缩放，窄卡片不至于把数字挤出右边。
pub fn action_icon_size(content_width: u32) -> u32 {
    let scaled = content_width as u64 * ACTION_ICON_AT_FULL as u64 / ACTION_BASE_CONTENT as u64;
    (scaled as u32).clamp(ACTION_ICON_MIN, ACTION_ICON_AT_FULL)
}

/// 互动数字的字号：按内容列宽度等比缩放，窄卡片保底 10px。
///
/// 不跟随图标算，是因为 460 的窄卡片上图标有 16px 下限，字号跟着图标会把
/// 「19,999」这种最长的数字挤出四等分的格子；这里让字号比图标掉得更快一点。
pub fn action_font_size(content_width: u32) -> u32 {
    let scaled = content_width as u64 * ACTION_FONT_AT_FULL as u64 / ACTION_BASE_CONTENT as u64;
    (scaled as u32).clamp(ACTION_FONT_MIN, ACTION_FONT_AT_FULL)
}

/// 数字与图标之间的间距。
fn action_gap(icon: u32) -> u32 {
    (icon / 4).max(3)
}

/// 右区（查看 / 书签 / 分享）内部的间距。
fn action_cluster_gap(icon: u32) -> u32 {
    (icon / 2).max(6)
}

/// 左区与右区之间的间距。
fn action_cluster_margin(icon: u32) -> u32 {
    (icon * 5 / 8).max(8)
}

/// 卡片模板要的数据：正文 + 卡片宽度 + 内容列 / 正文的确定宽度 +
/// 互动区四个随机数字（已格式化）+ 互动行的缩放尺寸。
pub fn view_data(content: &str, width: u32) -> Value {
    view_data_with_counts(content, width, &random_counts())
}

/// 与 [`view_data`] 相同，但互动数字由调用方给（测试 / 示例用，方便复现）。
pub fn view_data_with_counts(content: &str, width: u32, counts: &Counts) -> Value {
    let column = content_width(width);
    let icon = action_icon_size(column);
    let font = action_font_size(column);
    json!({
        "content": content,
        "width": width,
        "contentWidth": column,
        "textWidth": text_width(width),
        "replyCount": format_count(counts.reply),
        "repostCount": format_count(counts.repost),
        "likeCount": format_count(counts.like),
        "viewsCount": format_count(counts.views),
        "actionIcon": icon,
        "actionFont": font,
        "actionGap": action_gap(icon),
        "actionClusterGap": action_cluster_gap(icon),
        "actionClusterMargin": action_cluster_margin(icon),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_column_leaves_room_for_avatar_and_padding() {
        // 800 宽：800 - 2*(1+20) - 100 - 15 - 30 = 613，与 TS 版实测的内容列一致。
        assert_eq!(content_width(800), 613);
        assert_eq!(text_width(800), 582);
    }

    #[test]
    fn narrow_cards_do_not_underflow() {
        // 最窄 460：460 - 42 - 145 = 273，正文 259。
        assert_eq!(content_width(MIN_WIDTH), 273);
        assert_eq!(text_width(MIN_WIDTH), 259);
        // 比装饰宽度还窄时退化成 0，而不是下溢成天文数字。
        assert_eq!(content_width(0), 0);
        assert_eq!(text_width(0), 0);
    }

    #[test]
    fn view_data_exposes_definite_widths_and_counts() {
        let counts = Counts {
            reply: 4_584,
            repost: 12_000,
            like: 120_000,
            views: 3_680_000,
        };
        let data = view_data_with_counts("文案", 800, &counts);
        assert_eq!(data["content"], "文案");
        assert_eq!(data["width"], 800);
        assert_eq!(data["contentWidth"], 613);
        assert_eq!(data["textWidth"], 582);
        // 四个数字按 X 的中文计数格式化，模板只做插值。
        assert_eq!(data["replyCount"], "4,584");
        assert_eq!(data["repostCount"], "1.2万");
        assert_eq!(data["likeCount"], "12万");
        assert_eq!(data["viewsCount"], "368万");
        // 800 宽是互动行的基准尺寸：图标 32 / 数字 22。
        assert_eq!(data["actionIcon"], 32);
        assert_eq!(data["actionFont"], 22);
        assert_eq!(data["actionGap"], 8);
        assert_eq!(data["actionClusterGap"], 16);
        assert_eq!(data["actionClusterMargin"], 20);
    }

    #[test]
    fn format_count_matches_x_chinese_counting() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(4_584), "4,584");
        assert_eq!(format_count(9_999), "9,999");
        // 1 万–10 万：一位小数，截断不四舍五入。
        assert_eq!(format_count(10_000), "1万");
        assert_eq!(format_count(12_000), "1.2万");
        assert_eq!(format_count(19_258), "1.9万");
        assert_eq!(format_count(99_999), "9.9万");
        // 10 万以上：整数万。
        assert_eq!(format_count(100_000), "10万");
        assert_eq!(format_count(120_000), "12万");
        assert_eq!(format_count(134_462), "13万");
        assert_eq!(format_count(3_680_000), "368万");
        assert_eq!(format_count(9_999_999), "999万");
    }

    #[test]
    fn random_counts_stay_in_their_ranges() {
        // 随机只影响观感：每种计数都必须落在模板能排下的范围内。
        for _ in 0..64 {
            let counts = random_counts();
            assert!((300..=19_999).contains(&counts.reply));
            assert!((900..=99_999).contains(&counts.repost));
            assert!((4_000..=999_999).contains(&counts.like));
            assert!((30_000..=9_999_999).contains(&counts.views));
        }
    }

    #[test]
    fn action_row_scales_down_for_narrow_cards() {
        // 最窄 460：内容列 273，图标缩到下限 16、数字 10。
        let column = content_width(MIN_WIDTH);
        let icon = action_icon_size(column);
        assert_eq!(icon, 16);
        assert_eq!(action_font_size(column), 10);
        assert_eq!(action_gap(icon), 4);
        // 窄卡片的下限也不能大过内容列本身。
        assert!(icon <= column);
    }
}
