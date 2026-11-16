//! `/trending`：抓榜单 → 渲染 4 列卡片墙 → 以图片回复触发消息。
//!
//! 命令执行流程：
//!
//! ```text
//! /trending [today|weekly|monthly]
//!   → Period::from_arg()          // core::model，缺省今日
//!   → fetch::fetch_trending()     // core::fetch，抓 github.com/trending?since=…
//!   → CardData::build()           // core::model，截断 + 汇总
//!   → render::render_card()       // fuyubot.html2img 服务出图
//!   → host::command.reply         // 回复命令消息，形态由宿主决定
//!   → 删除 html2img 产物
//! ```
//!
//! 三个阶段各自失败时都回中文提示并记日志，不把技术细节甩给用户；
//! 每个阶段边界检查 `ctx.is_shutdown()`，让插件停机不用等满一次 HTTP + 渲染 + 上传。

use std::time::Instant;

use crate::core::model::{self, CardData, Period};
use crate::core::{avatar, fetch};
use crate::host::{Ctx, Msg};
use crate::render;

/// 参数不认识时的用法提示。
const USAGE: &str = "用法：/trending [today|weekly|monthly]\n\
示例：/trending today、/trending weekly、/trending 本月\n\
不带参数时看今日榜。";

/// 抓取失败的提示（技术原因写在日志里）。
const FETCH_FAILED: &str = "❌ 榜单抓取失败，请稍后再试。";

/// 页面解析出 0 条时的提示。
const EMPTY: &str = "榜单暂时没有内容，请稍后再试。";

/// 北京时间相对 UTC 的固定偏移（秒）。
///
/// 中国全境使用 UTC+8 且**不实行夏令时**，所以这里按固定偏移近似本地时间就够了，
/// 不需要引入 chrono / time 这类带时区数据库的依赖（本插件也没有它们）。
const BEIJING_OFFSET_SECS: u64 = 8 * 3600;

/// 一天的秒数。
const DAY_SECS: u64 = 24 * 3600;

/// 命令入口。`arg` 是 `/trending` 之后的第一个参数（`today` / `weekly` / `monthly`，中文也认）。
pub fn run(ctx: &Ctx, msg: Msg, arg: Option<String>) {
    if ctx.is_shutdown() {
        return;
    }

    // 缺省今日；给了但不认识的参数就回用法，不替用户猜。
    let period = match arg
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        None => Period::Today,
        Some(raw) => match Period::from_arg(raw) {
            Some(period) => period,
            None => {
                ctx.messenger
                    .try_send_text(msg.chat_id, USAGE, Some(msg.id));
                return;
            }
        },
    };

    let started = Instant::now();
    let mut items = match fetch::fetch_trending(&ctx.config, period) {
        Ok(items) => items,
        Err(error) => {
            ctx.error(&format!("trending: 抓取{}榜失败: {error}", period.label()));
            ctx.messenger
                .try_send_text(msg.chat_id, FETCH_FAILED, Some(msg.id));
            return;
        }
    };

    // 停机后不再往下走：渲染与上传都是阻塞的，每个阶段边界检查一次可以把
    // 「停不下来」的最坏时间压到当前这一步。
    if ctx.is_shutdown() {
        return;
    }

    // 配置的条数可能比抓到的少，先截断再汇总，避免页脚统计里出现不展示的条目；
    // `CardData::build` 内部还有 `model::MAX_ITEMS` 的硬上限。
    items.truncate(ctx.config.max_items.min(model::MAX_ITEMS));
    if items.is_empty() {
        ctx.warn(&format!("trending: {}榜没有解析出任何仓库", period.label()));
        ctx.messenger
            .try_send_text(msg.chat_id, EMPTY, Some(msg.id));
        return;
    }

    // 头像要联网抓（15 个头像并发，最多 avatar_timeout_ms），关掉时只清空字段。
    // 失败/超预算的条目留空，模板回退成首字母方块，不影响出图。
    let filled = avatar::fill_report(&ctx.config, &mut items);
    if ctx.config.avatar {
        ctx.info(&format!(
            "trending: 头像 {}/{}（{} KiB）",
            filled.ok,
            items.len(),
            filled.bytes / 1024
        ));
    }
    if ctx.is_shutdown() {
        return;
    }

    let card = CardData::build(period, items, beijing_now());

    // 出图交给宿主上的 fuyubot.html2img：插件只把卡片数据映射成 Vue 模板的 data，
    // 产物是服务临时目录里的本机文件（24 小时内有效）。这是阻塞调用。
    let path = match render::render_card(&ctx.host, &card, ctx.config.width) {
        Ok(path) => path,
        Err(error) => {
            ctx.error(&format!(
                "trending: 卡片渲染失败（{}榜）: {error}",
                period.label()
            ));
            ctx.messenger.try_send_text(
                msg.chat_id,
                &format!("❌ 卡片渲染失败：{error}"),
                Some(msg.id),
            );
            return;
        }
    };
    ctx.info(&format!(
        "trending: 已生成 {}榜卡片 {}（{} ms，{} 条）",
        period.label(),
        path.display(),
        started.elapsed().as_millis(),
        card.items.len()
    ));

    if ctx.is_shutdown() {
        // 停机了没人发这张图，直接删掉，不留给 html2img 的 24 小时清理。
        remove_artifact(ctx, &path);
        return;
    }

    let sent = ctx.messenger.send_photo(msg.chat_id, &path, Some(msg.id));

    // 产物在上传完成后就没用了，发完即删；删除失败只记日志（服务自己也会清理）。
    remove_artifact(ctx, &path);

    match sent {
        Ok(sent) => ctx.info(&format!(
            "trending: 已发送 {}榜卡片（chat={} message_id={}）",
            period.label(),
            sent.chat_id,
            sent.message_id
        )),
        Err(error) => {
            ctx.error(&format!(
                "trending: 发送图片失败（{}榜）: {error}",
                period.label()
            ));
            ctx.messenger.try_send_text(
                msg.chat_id,
                &format!("❌ 图片发送失败：{error}"),
                Some(msg.id),
            );
        }
    }
}

/// 删除 html2img 产物：`NotFound` 静默（本来就没生成 / 已被清理），其余记 warn。
fn remove_artifact(ctx: &Ctx, path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => ctx.warn(&format!(
            "trending: 删除渲染产物失败 {}: {error}",
            path.display()
        )),
    }
}

/// 当前北京时间，格式 `YYYY-MM-DD HH:MM`（卡片页脚的抓取时间）。
///
/// 用固定 +8 小时近似：中国无夏令时，「UTC 加 8 小时」就是北京时间。
fn beijing_now() -> String {
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    format_beijing(unix_secs)
}

/// 把 Unix 秒（UTC）格式化成北京时间的 `YYYY-MM-DD HH:MM`。
///
/// 抽成纯函数便于单测：不需要等到某个真实时刻。
fn format_beijing(unix_secs: u64) -> String {
    // 固定 +8 小时近似北京时间（见 `BEIJING_OFFSET_SECS` 的说明）。
    let local = unix_secs.saturating_add(BEIJING_OFFSET_SECS);
    let days = (local / DAY_SECS) as i64;
    let seconds_of_day = local % DAY_SECS;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = seconds_of_day % 3600 / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// 自 1970-01-01 起的天数 → `(年, 月, 日)`。
///
/// Howard Hinnant 的 `civil_from_days` 算法：纯整数运算，不依赖时区数据库，
/// 对 1970 前后的日期都成立（本插件只会用到 2020 之后）。
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    // 把纪元挪到 0000-03-01，让闰日落在年末，闰年规则就只剩 4 / 100 / 400 三条。
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11]，3 月是 0
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32; // [1, 31]
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_maps_to_beijing_morning() {
        assert_eq!(format_beijing(0), "1970-01-01 08:00");
    }

    #[test]
    fn known_instants_are_formatted() {
        // 2025-01-01 00:00:00 UTC → 北京时间 08:00。
        assert_eq!(format_beijing(1_735_689_600), "2025-01-01 08:00");
        // 2024-02-29 12:00:00 UTC → 北京时间 20:00（闰日）。
        assert_eq!(format_beijing(1_709_208_000), "2024-02-29 20:00");
        // 2026-11-16 12:34:00 UTC → 北京时间 20:34。
        assert_eq!(format_beijing(1_794_832_440), "2026-11-16 20:34");
    }

    #[test]
    fn midnight_rollover_crosses_the_day_boundary() {
        // 2026-11-16 16:00:00 UTC → 北京时间次日 00:00。
        assert_eq!(format_beijing(1_794_844_800), "2026-11-17 00:00");
    }

    #[test]
    fn every_period_parses_from_its_chinese_label() {
        assert_eq!(Period::from_arg("今日"), Some(Period::Today));
        assert_eq!(Period::from_arg("周榜"), Some(Period::Weekly));
        assert_eq!(Period::from_arg("月榜"), Some(Period::Monthly));
        assert_eq!(Period::from_arg("nonsense"), None);
    }
}
