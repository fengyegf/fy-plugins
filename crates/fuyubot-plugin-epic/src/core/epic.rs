//! Epic 免费游戏接口：请求、解析、日期换算。
//!
//! 读取 Epic 促销接口，按免费促销规则筛选（`discountPercentage === 0` 才算免费），
//! 再处理封面优先级与「神秘游戏」标题。
//!
//! 解析规则：
//! - 只有 `discountType == "PERCENTAGE"` **且** `discountPercentage == 0` 才是免费；
//!   打折 60% / 55% 的促销虽然也是 PERCENTAGE，但不是免费，必须排除。
//! - 免费时间窗在 `promotionalOffers[0].promotionalOffers[0]`，
//!   即将免费在 `upcomingPromotionalOffers` 的同构结构里；两者都可能整体缺失 /
//!   是空数组 / 是 `null`，一律当「没有」处理。
//! - 日期换算不许引 chrono：接口固定返回 UTC（`...Z`），加偏移后只需要
//!   `YYYY-MM-DD`，自己用 Howard Hinnant 的 days_from_civil / civil_from_days 算。

use std::fmt;

use serde_json::Value;

use crate::core::fetch::{FetchError, HttpClient};

/// 接口 JSON 的响应体上限。真实响应约 45 KB，8 MiB 足够宽松又能挡住异常大文件。
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// 数据模型：一款促销中的游戏。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreeGame {
    /// 标题（可能是「神秘游戏」）。
    pub title: String,
    /// 免费开始日期（本地时区，`YYYY-MM-DD`）。
    pub start_date: String,
    /// 免费结束日期（本地时区，`YYYY-MM-DD`）。
    pub end_date: String,
    /// 封面地址；接口没给就是 `None`。
    pub offer_image: Option<String>,
}

/// 一次查询的结果：当前免费 + 即将免费。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreeGames {
    pub current: Vec<FreeGame>,
    pub upcoming: Vec<FreeGame>,
}

impl FreeGames {
    pub fn is_empty(&self) -> bool {
        self.current.is_empty() && self.upcoming.is_empty()
    }

    /// 按「当前免费 + 即将免费」的顺序遍历，渲染层据此排卡片。
    pub fn iter(&self) -> impl Iterator<Item = &FreeGame> {
        self.current.iter().chain(self.upcoming.iter())
    }
}

/// 查询参数。
#[derive(Debug, Clone)]
pub struct Query {
    pub api_url: String,
    pub locale: String,
    pub country: String,
    /// 日期换算用的时区偏移（小时）。
    pub tz_offset_hours: i64,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            api_url: super::config::DEFAULT_API_URL.to_owned(),
            locale: "zh-CN".to_owned(),
            country: "CN".to_owned(),
            tz_offset_hours: 8,
        }
    }
}

/// 接口层的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum EpicError {
    #[error("HTTP 请求失败: {0}")]
    Fetch(#[from] FetchError),
    #[error("接口返回非法 JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("接口返回结构异常: {0}")]
    Shape(String),
}

/// 拉取并解析。网络失败、结构异常都会返回 `Err`，由命令层兜底提示。
pub fn fetch(client: &HttpClient, query: &Query) -> Result<FreeGames, EpicError> {
    let url = build_url(query);
    let body = client.get_text(&url, MAX_RESPONSE_BYTES)?;
    parse(&body, query)
}

/// 解析接口原始 JSON。
pub fn parse(body: &str, query: &Query) -> Result<FreeGames, EpicError> {
    let root: Value = serde_json::from_str(body)?;
    let elements = catalog_elements(&root)?;

    let mut games = FreeGames::default();
    for element in elements {
        // 没标题的条目渲染不出来，直接跳过（外部数据不做 unwrap）。
        let Some(title) = element.get("title").and_then(Value::as_str) else {
            continue;
        };

        let promotions = element.get("promotions");
        // 同一款游戏一般只命中一边；若两边都命中（促销叠加），按「当前免费」优先，避免重复计数。
        let (window, upcoming) = match free_window(
            promotions.and_then(|promotions| promotions.get("promotionalOffers")),
        ) {
            Some(window) => (window, false),
            None => match free_window(
                promotions.and_then(|promotions| promotions.get("upcomingPromotionalOffers")),
            ) {
                Some(window) => (window, true),
                None => continue,
            },
        };

        let game = FreeGame {
            title: display_title(title),
            start_date: local_date(window.0, query.tz_offset_hours),
            end_date: local_date(window.1, query.tz_offset_hours),
            offer_image: cover_url(element),
        };
        if upcoming {
            games.upcoming.push(game);
        } else {
            games.current.push(game);
        }
    }
    Ok(games)
}

/// 把接口的 ISO-8601 UTC 时间换算成本地日期。
///
/// 例：`2026-11-05T16:00:00.000Z` + UTC+8 → `2026-11-06`。
///
/// 解析失败（缺字段、月份越界、非 `Z` 的时区偏移等）返回 `None`，由调用方决定兜底方式。
pub fn format_date(iso: &str, tz_offset_hours: i64) -> Option<String> {
    let (days, seconds) = parse_utc(iso)?;
    let local = days
        .checked_mul(86_400)?
        .checked_add(seconds)?
        .checked_add(tz_offset_hours.checked_mul(3_600)?)?;

    // div_euclid：负偏移落到前一天时也要向下取整（-1 / 86400 的截断除法是 0）。
    let (year, month, day) = civil_from_days(local.div_euclid(86_400));
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// 拼接口地址：`{api_url}?locale={locale}&country={country}&allowCountries={country}`。
///
/// 参数都是 `zh-CN` / `CN` 这种纯 ASCII，无需 percent-encode；
/// 允许 `api_url` 自带查询串（换镜像时常见），此时用 `&` 续接。
fn build_url(query: &Query) -> String {
    let separator = if query.api_url.contains('?') {
        '&'
    } else {
        '?'
    };
    format!(
        "{}{separator}locale={}&country={}&allowCountries={}",
        query.api_url, query.locale, query.country, query.country
    )
}

/// 取 `data.Catalog.searchStore.elements`；多包一层 `data` 的镜像也兼容。
fn catalog_elements(root: &Value) -> Result<&[Value], EpicError> {
    let data = root
        .get("data")
        .ok_or_else(|| EpicError::Shape("缺少 data 字段".to_owned()))?;
    // 正常是 data.Catalog…；有的包装会多一层 data.data.Catalog…。
    let payload = if data.get("Catalog").is_some() {
        data
    } else {
        data.get("data").unwrap_or(data)
    };
    payload
        .pointer("/Catalog/searchStore/elements")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| EpicError::Shape("缺少 data.Catalog.searchStore.elements[]".to_owned()))
}

/// 取一条「免费促销」的时间窗。
///
/// 结构：`[{ promotionalOffers: [{ startDate, endDate, discountSetting }] }]`。
/// 只有 `discountType == "PERCENTAGE"` 且 `discountPercentage == 0` 才算免费；
/// 缺失 / 空数组 / `null` 都返回 `None`。
fn free_window(offers: Option<&Value>) -> Option<(&str, &str)> {
    let entry = offers?.as_array()?.first()?;
    let offer = entry.get("promotionalOffers")?.as_array()?.first()?;
    let setting = offer.get("discountSetting")?;

    if setting.get("discountType").and_then(Value::as_str) != Some("PERCENTAGE") {
        return None;
    }
    // JSON 的 0 解析成 f64 是精确的 0.0，这里只做判零，不参与其它运算。
    if setting.get("discountPercentage").and_then(Value::as_f64) != Some(0.0) {
        return None;
    }

    let start = offer.get("startDate").and_then(Value::as_str)?;
    let end = offer.get("endDate").and_then(Value::as_str)?;
    Some((start, end))
}

/// 日期换算失败时退回空串：游戏本身是免费的，不该因为时间格式异常被整个丢掉。
fn local_date(iso: &str, tz_offset_hours: i64) -> String {
    format_date(iso, tz_offset_hours).unwrap_or_default()
}

/// 封面优先级：`OfferImageWide` → `heroCarouselVideo` 的 `cover=` 参数 → `DieselStoreFrontWide`。
///
/// 注意优先级不是「第一张图」：有的条目把 `heroCarouselVideo` 排在前面，
/// 但仍要优先用后面出现的 `OfferImageWide`。
fn cover_url(element: &Value) -> Option<String> {
    let images = element
        .get("keyImages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    image_of_type(images, "OfferImageWide")
        .map(str::to_owned)
        .or_else(|| carousel_cover(images))
        .or_else(|| image_of_type(images, "DieselStoreFrontWide").map(str::to_owned))
}

/// 按 `type` 找第一张图的 url。
fn image_of_type<'a>(images: &'a [Value], kind: &str) -> Option<&'a str> {
    images
        .iter()
        .filter(|image| image.get("type").and_then(Value::as_str) == Some(kind))
        .find_map(|image| image.get("url").and_then(Value::as_str))
}

/// `heroCarouselVideo` 的 url 形如 `com.epicgames.video://<id>?cover=<percent-encoded>`，
/// 从中取 `cover=` 参数并 percent-decode 才是真正的图片地址。
fn carousel_cover(images: &[Value]) -> Option<String> {
    images
        .iter()
        .filter(|image| image.get("type").and_then(Value::as_str) == Some("heroCarouselVideo"))
        .filter_map(|image| image.get("url").and_then(Value::as_str))
        .find_map(cover_parameter)
}

/// 从 URL 的 query 里取 `cover=` 参数的值（可能还跟着 `&` 其它参数）。
fn cover_parameter(url: &str) -> Option<String> {
    let value = url
        .split_once('?')?
        .1
        .split('&')
        .find_map(|pair| pair.strip_prefix("cover="))?;
    let value = value.split('#').next().unwrap_or(value);
    if value.is_empty() {
        None
    } else {
        Some(percent_decode(value))
    }
}

/// 最小的 percent-decode（只处理 `%XX`）：`%3A%2F%2F` → `://`。
///
/// URL 的 query 里 `+` 不是空格，所以不处理 `+`；非法转义原样保留，不报错。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 单个十六进制字符 → 数值。
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 「神秘游戏」：Epic 有时用 `Mystery Game` 占位标题。
///
/// 只要是 `Mystery Game` 开头，整串标题都换成「神秘游戏」
/// （后缀是 Epic 的占位编号，对用户没有意义）。
fn display_title(title: &str) -> String {
    const MYSTERY: &str = "Mystery Game";
    if title.starts_with(MYSTERY) {
        return "神秘游戏".to_owned();
    }
    // 弯撇号（U+2019）在 hyper-render 的字体回退里会换到另一套字体，实测
    // "We’re" 会渲染成 "We’ re"（多出一个空隙）。展示层统一成 ASCII 撇号。
    title.replace('\u{2019}', "'")
}

/// 解析 `YYYY-MM-DDTHH:MM:SS[.fff]Z` 的 UTC 时间，返回 `(1970-01-01 起的天数, 当天秒数)`。
///
/// 只接受 `Z`（或省略时区，同样按 UTC 处理）：接口固定返回 UTC，
/// 带 `+08:00` 这类偏移的输入视为不支持，返回 `None` 而不是悄悄算错。
fn parse_utc(iso: &str) -> Option<(i64, i64)> {
    let iso = iso.trim();
    let body = iso
        .strip_suffix('Z')
        .or_else(|| iso.strip_suffix('z'))
        .unwrap_or(iso);
    // 去掉 `Z` 后时间部分还带 `+` / `-`，说明是时区偏移，不支持。
    let (date, time) = body.split_once('T')?;
    if time.contains('+') || time.contains('-') {
        return None;
    }

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    // 秒可能有小数（`.000`）：整数部分必须是纯数字，小数部分若存在也只能是数字，
    // 这样 `00junk`、`00.000Zjunk` 之类都会在下面被拒掉，而不是悄悄当 0 秒。
    let seconds_field = time_parts.next().unwrap_or("0");
    let (second_text, fraction) = match seconds_field.split_once('.') {
        Some((second, fraction)) => (second, Some(fraction)),
        None => (seconds_field, None),
    };
    if !second_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let second: i64 = second_text.parse().ok()?;
    if time_parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    Some((
        days_from_civil(year, month, day),
        hour * 3_600 + minute * 60 + second,
    ))
}

/// 闰年判断（公历规则）。
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// 某年某月的天数。
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        _ => 28,
    }
}

/// Howard Hinnant 的 `days_from_civil`：公历日期 → 距 1970-01-01 的天数。
///
/// 用「400 年一个 era」的整除写法规避闰年分支，负年份也成立。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let month_prime = (i64::from(month) + 9) % 12; // 3 月 = 0
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Howard Hinnant 的 `civil_from_days`：距 1970-01-01 的天数 → 公历日期。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11]，3 月 = 0
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 供日志 / 调试用的简要描述。
impl fmt::Display for FreeGames {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "当前免费 {} 个 / 即将免费 {} 个",
            self.current.len(),
            self.upcoming.len()
        )
    }
}
