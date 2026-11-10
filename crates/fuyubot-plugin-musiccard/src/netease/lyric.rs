//! LRC 解析 + 「从 35% 处挑 5 行」的取词逻辑。
//!
//! 时间戳格式、过滤规则、取词窗口和截断长度集中在这里维护。
//!
//! 文本截断复用 [`crate::util::truncate`]，按字符处理，中文和 emoji 不会被切坏。

use std::collections::HashMap;

use crate::model::LyricLine;
use crate::util::truncate;

/// 无歌词时卡片上显示的默认文案。
pub const EMPTY_LYRIC: &str = "暂无歌词，愿旋律陪你度过今天。";

/// 原文截断长度。
const ORIGINAL_MAX_CHARS: usize = 31;
/// 译文截断长度。
const TRANSLATION_MAX_CHARS: usize = 42;
/// 卡片上最多放几行。
const MAX_LINES: usize = 5;
/// 取词窗口起点比例。
const START_RATIO: f64 = 0.35;

/// 制作人员名单类行，直接丢弃。
const CREDIT_PREFIXES: [&str; 5] = ["作词", "作曲", "编曲", "制作人", "纯音乐"];

/// 一行带时间戳的歌词；翻译要靠时间对齐，所以内部得留着时间。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TimedLine {
    /// 秒（`mm * 60 + ss + 小数部分`）。
    pub time: f64,
    pub text: String,
}

/// 解析 LRC：
///
/// - 每行必须以 `[mm:ss]` 或 `[mm:ss.fff]` 开头（分钟 1-3 位、秒固定 2 位、
///   小数 1-3 位），后面跟正文；
/// - 正文 `trim` 后为空的行丢弃；
/// - 以 作词/作曲/编曲/制作人/纯音乐 开头的行丢弃。
pub(crate) fn parse_lyric(raw: &str) -> Vec<TimedLine> {
    raw.split('\n')
        .filter_map(|row| parse_row(row.strip_suffix('\r').unwrap_or(row)))
        .collect()
}

/// 解析单行；不符合 `^\[\d{1,3}:\d{2}(?:\.\d{1,3})?\](.*)$` 就返回 `None`。
fn parse_row(row: &str) -> Option<TimedLine> {
    let rest = row.strip_prefix('[')?;
    // 时间戳到第一个 `]` 结束；正文里即使还有 `]` 也属于歌词。
    let (stamp, text) = rest.split_once(']')?;

    let (minutes, tail) = stamp.split_once(':')?;
    let (seconds, fraction) = match tail.split_once('.') {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (tail, None),
    };

    if !(1..=3).contains(&minutes.len()) || !is_ascii_digits(minutes) {
        return None;
    }
    if seconds.len() != 2 || !is_ascii_digits(seconds) {
        return None;
    }
    let fraction = match fraction {
        None => 0.0,
        Some(digits) => {
            if digits.is_empty() || digits.len() > 3 || !is_ascii_digits(digits) {
                return None;
            }
            // `34` → 0.34、`5` → 0.5。
            let value: f64 = digits.parse().ok()?;
            value / 10f64.powi(digits.len() as i32)
        }
    };

    let text = text.trim();
    if text.is_empty() || CREDIT_PREFIXES.iter().any(|p| text.starts_with(p)) {
        return None;
    }

    let minutes: u32 = minutes.parse().ok()?;
    let seconds: u32 = seconds.parse().ok()?;
    Some(TimedLine {
        time: f64::from(minutes) * 60.0 + f64::from(seconds) + fraction,
        text: text.to_owned(),
    })
}

fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// 时间对齐用的 key，精确到 0.1 秒。
fn time_key(time: f64) -> i64 {
    (time * 10.0).round() as i64
}

/// 从 LRC 里挑最多 5 行，并给每行配翻译。
///
/// 选词规则：
/// 1. 原文解析不出任何一行 → 返回一句默认文案；
/// 2. 起止窗口 `start = min(floor(len * 0.35), max(0, len - count))`，
///    `count = min(5, len)`，保证不越界；
/// 3. 原文截断 31 字符，译文按时间戳（四舍五入到 0.1s）匹配后截断 42 字符；
/// 4. 匹配不到译文的行 `translation` 为 `None`。
pub fn select_lyrics(original: &str, translated: &str) -> Vec<LyricLine> {
    let lines = parse_lyric(original);
    if lines.is_empty() {
        return vec![LyricLine {
            text: EMPTY_LYRIC.to_owned(),
            translation: None,
        }];
    }

    // 同一时间戳出现多次时，后写入的译文覆盖先写入的。
    let mut translation_by_time: HashMap<i64, String> = HashMap::new();
    for line in parse_lyric(translated) {
        translation_by_time.insert(time_key(line.time), line.text);
    }

    let count = lines.len().min(MAX_LINES);
    let start = (((lines.len() as f64) * START_RATIO).floor() as usize).min(lines.len() - count);

    lines[start..start + count]
        .iter()
        .map(|line| LyricLine {
            text: truncate(&line.text, ORIGINAL_MAX_CHARS),
            translation: translation_by_time
                .get(&time_key(line.time))
                .map(|translation| truncate(translation, TRANSLATION_MAX_CHARS)),
        })
        .collect()
}
