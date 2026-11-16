//! GitHub Trending 的数据模型与展示格式化。
//!
//! 卡片模板只做插值与 `v-for`，**不调用任何函数**，所以千分位、描述截断、
//! 相对进度条宽度这些派生值都在这里算好。

/// 展示给用户的最多条数：今天的榜单一页 15 条，4 列刚好排 4 行。
pub const MAX_ITEMS: usize = 15;

/// 描述最多保留的字符数（中文按字算）。
///
/// 卡片内容宽约 336px、13px 字号：一行约 50–56 个 ASCII 字符、约 26 个汉字。
/// 取 72 让英文简介基本铺满两行再省略（截断点通常落在第二行中间，省略号不会
/// 单独换行），中文简介也不会被砍得太短。
pub const DESC_MAX_CHARS: usize = 72;

/// 榜单周期：今日 / 本周 / 本月。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    /// `github.com/trending?since=daily`
    #[default]
    Today,
    /// `since=weekly`
    Weekly,
    /// `since=monthly`
    Monthly,
}

impl Period {
    /// 三个周期的固定顺序，模板用它排切换标签。
    pub const ALL: [Self; 3] = [Self::Today, Self::Weekly, Self::Monthly];

    /// `https://github.com/trending?since=…` 的取值。
    pub const fn since(self) -> &'static str {
        match self {
            Self::Today => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    /// 面板主标题（中文）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Today => "今日",
            Self::Weekly => "本周",
            Self::Monthly => "本月",
        }
    }

    /// 面板副标题（英文小字）。
    pub const fn en(self) -> &'static str {
        match self {
            Self::Today => "TODAY",
            Self::Weekly => "THIS WEEK",
            Self::Monthly => "THIS MONTH",
        }
    }

    /// 每条数据「新增 star」的时间口径，卡片里显示成 `+1,234 / 今日`。
    pub const fn gain_label(self) -> &'static str {
        match self {
            Self::Today => "今日",
            Self::Weekly => "本周",
            Self::Monthly => "本月",
        }
    }

    /// 时间范围的长描述，放在面板页眉。
    pub const fn window_label(self) -> &'static str {
        match self {
            Self::Today => "过去 24 小时",
            Self::Weekly => "过去 7 天",
            Self::Monthly => "过去 30 天",
        }
    }

    /// 解析命令参数：英文全称 / 缩写与中文都认，忽略大小写与首尾空白。
    pub fn from_arg(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "today" | "daily" | "day" | "d" | "1" | "今日" | "今天" | "日" | "日榜" => {
                Some(Self::Today)
            }
            "week" | "weekly" | "w" | "7" | "本周" | "这周" | "周" | "周榜" => {
                Some(Self::Weekly)
            }
            "month" | "monthly" | "m" | "30" | "本月" | "这个月" | "月" | "月榜" => {
                Some(Self::Monthly)
            }
            _ => None,
        }
    }
}

/// 榜单里的一条仓库。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrendingItem {
    /// 仓库所属用户 / 组织。
    pub owner: String,
    /// 仓库名。
    pub name: String,
    /// 仓库简介；页面上没有时是空串。
    pub description: String,
    /// 主语言；页面上没有时是空串（模板显示「未知」）。
    pub language: String,
    /// 主语言的色值（`#3178c6` 这类）；解析不到时是空串。
    pub language_color: String,
    /// 累计 star 数。
    pub stars: u64,
    /// 累计 fork 数。
    pub forks: u64,
    /// 本周期新增的 star 数（页面右上角的 `1,234 stars today`）。
    pub gained: u64,
    /// 作者头像的 data URI（`data:image/png;base64,…`，40px PNG）。
    ///
    /// 由 [`crate::core::avatar`] 抓取后回填；空串表示没抓到（关掉了头像、
    /// 超预算或请求失败），模板会用首字母方块兜底。
    pub avatar: String,
}

impl TrendingItem {
    /// `owner/name`。
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// 页眉里的语言分布统计。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageStat {
    /// 语言名。
    pub name: String,
    /// 色值，与条目里的 `language_color` 同源。
    pub color: String,
    /// 上榜仓库数。
    pub count: usize,
}

/// 渲染卡片用的最终数据：榜单条目 + 页眉 / 页脚的汇总。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardData {
    /// 榜单周期。
    pub period: Period,
    /// 榜单条目，最多 [`MAX_ITEMS`] 条。
    pub items: Vec<TrendingItem>,
    /// 抓取时间（本地时间，`2026-11-16 20:50`）。
    pub fetched_at: String,
    /// 上榜仓库的 star 增量合计。
    pub total_gained: u64,
    /// 上榜仓库的累计 star 合计。
    pub total_stars: u64,
    /// 单条最大的 star 增量，用来算条目的相对进度条。
    pub max_gained: u64,
    /// 语言分布前 3 名。
    pub top_languages: Vec<LanguageStat>,
}

impl CardData {
    /// 汇总榜单：条目截断到 [`MAX_ITEMS`]，并算好页眉 / 页脚要的统计值。
    pub fn build(period: Period, mut items: Vec<TrendingItem>, fetched_at: String) -> Self {
        items.truncate(MAX_ITEMS);
        let total_gained = items.iter().map(|item| item.gained).sum();
        let total_stars = items.iter().map(|item| item.stars).sum();
        let max_gained = items.iter().map(|item| item.gained).max().unwrap_or(0);
        Self {
            period,
            top_languages: language_stats(&items),
            items,
            fetched_at,
            total_gained,
            total_stars,
            max_gained,
        }
    }
}

/// 语言分布：按上榜数量降序，同数量的按名字升序；空语言（页面上没有主语言）忽略。
fn language_stats(items: &[TrendingItem]) -> Vec<LanguageStat> {
    let mut stats: Vec<LanguageStat> = Vec::new();
    for item in items {
        let name = item.language.trim();
        if name.is_empty() {
            continue;
        }
        match stats.iter_mut().find(|stat| stat.name == name) {
            Some(stat) => stat.count += 1,
            None => stats.push(LanguageStat {
                name: name.to_owned(),
                color: item.language_color.trim().to_owned(),
                count: 1,
            }),
        }
    }
    stats.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    stats.truncate(3);
    stats
}

/// 千分位：`18364` → `18,364`。
pub fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 卡片底部的短数字：`18,364` → `18.4k`，`1,234,567` → `1.2M`。
///
/// 卡片宽 372px 的底行还要放增量与 fork，四位数以上的完整数字放不下，
/// 这里统一压成一位小数的短格式；`1000` 以下原样返回。
pub fn compact(value: u64) -> String {
    if value < 1_000 {
        return value.to_string();
    }
    let (scaled, unit) = if value < 1_000_000 {
        (value as f64 / 1_000.0, "k")
    } else {
        (value as f64 / 1_000_000.0, "M")
    };
    // `{:.1}` 会把整数补成 "20.0"，短格式里没必要留这个 ".0"。
    let text = format!("{scaled:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text);
    format!("{text}{unit}")
}

/// 压掉换行与连续空白：GitHub 的简介里偶尔有换行，模板不会替我们处理。
pub fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// 截断到 `max_chars` 个字符（按 `char` 数，不是字节），超长时补一个省略号。
///
/// 英文简介尽量在**空格处**断词：硬切会留下 `into F…` 这种半个单词，观感差。
/// 断点太靠前（保留不到 60%）就还是硬切——不然很短的一句话会被砍掉一大截。
/// 中文没有空格，自然走硬切。
pub fn truncate(text: &str, max_chars: usize) -> String {
    let text = squeeze(text);
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text;
    }
    let budget = max_chars.saturating_sub(1);
    let mut cut = budget;
    if let Some(space) = chars[..budget].iter().rposition(|ch| ch.is_whitespace())
        && space * 10 >= budget * 6
    {
        cut = space;
    }
    let out: String = chars[..cut].iter().collect();
    format!("{}…", out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, language: &str, stars: u64, gained: u64) -> TrendingItem {
        TrendingItem {
            owner: "o".to_owned(),
            name: name.to_owned(),
            description: "desc".to_owned(),
            language: language.to_owned(),
            language_color: "#fff".to_owned(),
            stars,
            forks: 1,
            gained,
            avatar: String::new(),
        }
    }

    #[test]
    fn period_maps_to_github_since() {
        assert_eq!(Period::Today.since(), "daily");
        assert_eq!(Period::Weekly.since(), "weekly");
        assert_eq!(Period::Monthly.since(), "monthly");
    }

    #[test]
    fn period_parses_english_and_chinese() {
        for raw in ["today", "DAILY", " 日榜 ", "今日", "1"] {
            assert_eq!(Period::from_arg(raw), Some(Period::Today), "{raw}");
        }
        for raw in ["week", "Weekly", "周榜", "本周", "7"] {
            assert_eq!(Period::from_arg(raw), Some(Period::Weekly), "{raw}");
        }
        for raw in ["month", "MONTHLY", "月榜", "本月", "30"] {
            assert_eq!(Period::from_arg(raw), Some(Period::Monthly), "{raw}");
        }
        assert_eq!(Period::from_arg(""), None);
        assert_eq!(Period::from_arg("year"), None);
    }

    #[test]
    fn build_sums_and_truncates() {
        let items: Vec<TrendingItem> = (0..20).map(|i| item("r", "Rust", 10, i)).collect();
        let card = CardData::build(Period::Today, items, "2026-11-16 20:50".to_owned());
        assert_eq!(card.items.len(), MAX_ITEMS);
        assert_eq!(card.items[0].name, "r");
        assert_eq!(card.total_stars, 150);
        assert_eq!(card.total_gained, (0..15).sum::<u64>());
        assert_eq!(card.max_gained, 14);
    }

    #[test]
    fn build_keeps_top_three_languages() {
        let items = vec![
            item("a", "Rust", 1, 1),
            item("b", "Rust", 1, 1),
            item("c", "TypeScript", 1, 1),
            item("d", "Go", 1, 1),
            item("e", "Python", 1, 1),
            item("f", "", 1, 1),
        ];
        let card = CardData::build(Period::Weekly, items, String::new());
        let names: Vec<&str> = card.top_languages.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Rust", "Go", "Python"]);
        assert_eq!(card.top_languages[0].count, 2);
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(18_364), "18,364");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn compact_shortens_large_counts() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1k");
        assert_eq!(compact(18_364), "18.4k");
        assert_eq!(compact(370_705), "370.7k");
        assert_eq!(compact(1_234_567), "1.2M");
        assert_eq!(compact(60_000), "60k");
    }

    #[test]
    fn squeeze_and_truncate_respect_char_boundaries() {
        assert_eq!(squeeze("  a\n  b\t c "), "a b c");
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("中文中文中文", 4), "中文中…");
        assert_eq!(truncate("anything", 0), "");
        // 英文在空格处断词，不留半个单词。
        assert_eq!(
            truncate("AG-UI: the Agent-User Interaction Protocol.", 24),
            "AG-UI: the Agent-User…"
        );
        // 断点太靠前（不到 60%）时退回硬切：预算 19 → "a " + 17 个 b + 省略号。
        let hard = format!("a {}…", "b".repeat(17));
        assert_eq!(truncate(&format!("a {}", "b".repeat(20)), 20), hard);
    }
}
