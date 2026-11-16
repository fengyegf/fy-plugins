//! GitHub 头像抓取 + 缩到 40px + PNG data URI。
//!
//! 卡片由宿主上的 `fuyubot.html2img` 服务渲染，而那个渲染器**完全离线**：模板里
//! 不能写 `<img src="https://…">`，远程图片根本不会加载。所以仓库作者头像只能在
//! 插件侧抓下来、缩好、转成 `data:image/png;base64,…` 塞进数据里。
//!
//! 40px 是按卡片上 1:1 的显示尺寸取的（[`AVATAR_PX`]）：再大只会让整页 HTML 变长，
//! 而 html2img 单页上限是 128 KiB；再小在高分屏上会糊。
//!
//! 抓取是阻塞的（ureq），只能在业务线程里跑。15 条串行抓一次要好几秒，所以这里用
//! 固定 6 个 `std::thread` 工作线程从共享队列取任务、结果用 channel 收回；整批受
//! `config.avatar_timeout` 总预算约束，到点就收工，已经拿到的照用。任何一条失败都
//! 只是「这条没有头像」，模板会回退到首字母方块，不影响其他条目。
//!
//! 体积还有第二道闸 [`TOTAL_BUDGET`]：所有头像的 PNG 原始字节加起来超过 48 KiB 就
//! 不再塞。40px 头像一张约 2–6 KiB，15 张全塞可能顶到 90 KiB，加上榜单数据就把
//! HTML 顶过 128 KiB 了——宁可少几张头像，也不能让整张图渲染失败。

use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use image::ImageEncoder as _;

use crate::core::{Config, TrendingItem};

/// 头像显示边长（px，卡片按 1:1 渲染）。
pub const AVATAR_PX: u32 = 40;

/// 所有头像编码后的总预算（原始 PNG 字节），超了就不再塞，避免撞上 html2img 的
/// 128 KiB HTML 上限。
///
/// 注意这里数的是 **PNG 原始字节**，不是 base64 之后的长度；base64 会再涨 1/3，
/// 所以 48 KiB 的 PNG 最终在 HTML 里约占 64 KiB，剩下的一半留给榜单数据与模板。
pub const TOTAL_BUDGET: usize = 48 * 1024;

/// 工作线程数：15 条 ÷ 6 路，弱网下三轮就能抓完；再多会同时打 GitHub 的头像 CDN。
const WORKERS: usize = 6;

/// 单张头像的响应体上限：`?size=80` 的 PNG 一般 2–10 KiB，256 KiB 只是挡住异常响应。
const MAX_BYTES: u64 = 256 * 1024;

/// 原图长边上限（px）：GitHub 头像不会超过 1024，更大的当作解压炸弹直接丢掉。
const MAX_SOURCE_SIDE: u32 = 1024;

/// 原图边长下限（px）：1×1 的跟踪像素画出来只是个色块，当作没有头像。
const MIN_SOURCE_SIDE: u32 = 8;

/// 建连超时上限：头像 CDN 连不上时，别把整批预算都耗在建连上。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// 剩余预算低于这个值就不再发起新请求：发出去也来不及在预算内返回。
const MIN_BUDGET: Duration = Duration::from_millis(200);

/// 头像请求的尺寸参数：GitHub 会按它出图，取 80 是 40px 显示尺寸的 2 倍，
/// 方便高清屏，实际到本地还会再缩到 [`AVATAR_PX`]。
const REQUEST_PX: u32 = 80;

/// 头像地址前缀。
const AVATAR_BASE: &str = "https://github.com";

/// 浏览器 UA：GitHub 对空 UA / 脚本 UA 可能直接 403。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// [`fill_report`] 的返回值：成功几个、所有 data URI 里 PNG 的原始字节合计。
///
/// 头像抓取本身不打日志：core 层不依赖宿主，测试才能离线跑；要不要记日志、记什么，
/// 由 cmd 层拿这个统计值自己决定。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Filled {
    /// 成功塞进条目的头像数。
    pub ok: usize,
    /// 这些头像的 PNG 原始字节合计（不含 base64 膨胀）。
    pub bytes: usize,
}

/// 给每个条目填 `avatar`（data URI，失败留空）。阻塞，只在业务线程调用。
///
/// 只是 [`fill_report`] 的丢弃返回值版本；需要日志时用后者。
pub fn fill(config: &Config, items: &mut [TrendingItem]) {
    fill_report(config, items);
}

/// 同 [`fill`]，但返回成功数与总字节数，方便调用方记一条统计日志。
///
/// 关掉头像（`config.avatar == false`）时**不发任何请求**，但仍会把旧值清空——
/// 调用方可能复用同一批条目重试，残留上一次的 data URI 会让「关闭头像」失效。
pub fn fill_report(config: &Config, items: &mut [TrendingItem]) -> Filled {
    fill_with(config, items, &http_fetch)
}

/// 可注入抓取函数的实现：网络分支不可单测，测试里换成假数据跑通整条管线。
///
/// `fetch` 收到「头像地址 + 允许花的时间」，返回图片原始字节；返回 `None` 表示这次
/// 抓取失败（超时、404、非图片都算），对应条目留空。
fn fill_with<F>(config: &Config, items: &mut [TrendingItem], fetch: &F) -> Filled
where
    F: Fn(&str, Duration) -> Option<Vec<u8>> + Sync,
{
    // 先清空：重复 fill 时不能残留上一次的 data URI。
    for item in items.iter_mut() {
        item.avatar.clear();
    }
    if !config.avatar || items.is_empty() {
        return Filled::default();
    }

    // 先把能拼出地址的条目排进队列：owner 为空 / 只有非法字符的条目直接跳过，
    // 不值得为它们起线程。
    let mut queue = VecDeque::new();
    for (index, item) in items.iter().enumerate() {
        if let Some(url) = avatar_url(&item.owner) {
            queue.push_back((index, url));
        }
    }
    let expected = queue.len();
    if expected == 0 {
        return Filled::default();
    }

    let deadline = Instant::now() + config.avatar_timeout;
    let queue = Arc::new(Mutex::new(queue));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let workers = WORKERS.min(expected);
    let mut results: Vec<Option<(String, usize)>> = vec![None; items.len()];

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let cancelled = Arc::clone(&cancelled);
            let tx = tx.clone();
            handles.push(scope.spawn(move || {
                worker(&queue, &cancelled, deadline, config.timeout, fetch, &tx);
            }));
        }
        // 主线程不投递结果；这里丢掉原始发送端，所有工作线程退出后 channel 才会断开，
        // 收端才能靠 `Disconnected` 提前收工（而不是干等到 deadline）。
        drop(tx);

        let mut received = 0;
        while received < expected {
            let now = Instant::now();
            if now >= deadline {
                cancelled.store(true, Ordering::Relaxed);
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok((index, payload)) => {
                    received += 1;
                    results[index] = payload;
                }
                // 到点：已完成的照用，没抓完的不等了。
                Err(RecvTimeoutError::Timeout) => {
                    cancelled.store(true, Ordering::Relaxed);
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // 工作线程可能在「到点」前后刚好把结果投进 channel：不阻塞地收一遍，
        // 已经算完的不浪费；还在请求里的那些等 join 完也不要了（超了预算）。
        cancelled.store(true, Ordering::Relaxed);
        while let Ok((index, payload)) = rx.try_recv() {
            results[index] = payload;
        }
        for handle in handles {
            let _ = handle.join();
        }
    });

    // 预算按条目顺序（也就是卡片上的名次顺序）结算：名次靠前的优先有头像，
    // 结果比「谁先抓完谁先占坑」更稳定，也不会因为线程调度不同而忽有忽无。
    let mut filled = Filled::default();
    for (index, payload) in results.into_iter().enumerate() {
        let Some((uri, bytes)) = payload else {
            continue;
        };
        if !within_budget(filled.bytes, bytes) {
            continue;
        }
        filled.bytes += bytes;
        filled.ok += 1;
        items[index].avatar = uri;
    }
    filled
}

/// 工作线程：从共享队列取「条目下标 + 头像地址」，抓回来转成 data URI 再送回。
///
/// 每处理一个任务就投一条结果（成功是 `Some`，失败是 `None`）；到点或收到取消信号
/// 立刻退出，不会把没抓完的任务拖着。
fn worker<F>(
    queue: &Arc<Mutex<VecDeque<(usize, String)>>>,
    cancelled: &Arc<AtomicBool>,
    deadline: Instant,
    per_request: Duration,
    fetch: &F,
    tx: &mpsc::Sender<(usize, Option<(String, usize)>)>,
) where
    F: Fn(&str, Duration) -> Option<Vec<u8>> + Sync,
{
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        // 剩余预算太少就连任务都不取了：发出去也来不及。
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining < MIN_BUDGET {
            return;
        }
        let task = match queue.lock() {
            Ok(mut queue) => queue.pop_front(),
            // 锁中毒说明别的线程 panic 了，直接收工，别在坏状态上继续抓。
            Err(_) => return,
        };
        let Some((index, url)) = task else {
            return;
        };
        // 单次 HTTP 的上限还要再被「剩余总预算」压一次，免得最后一个请求拖过 deadline。
        let timeout = per_request.min(remaining);
        let payload = fetch(&url, timeout).and_then(|bytes| to_data_uri(&bytes));
        if tx.send((index, payload)).is_err() {
            return;
        }
    }
}

/// 拼头像地址：`https://github.com/{owner}.png?size=80`；owner 为空返回 `None`。
///
/// owner 来自解析后的 HTML，理论上只会是 GitHub 用户名（字母数字与 `-`），但不信任
/// 外部输入：白名单（字母数字 `-` `_` `.`）之外的字节一律百分号编码，避免空格、
/// `/`、`?`、`#` 改变 URL 结构，也避免非 ASCII 直接进请求行。
fn avatar_url(owner: &str) -> Option<String> {
    let owner = owner.trim();
    if owner.is_empty() {
        return None;
    }
    let mut encoded = String::with_capacity(owner.len());
    for byte in owner.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(char::from(byte));
        } else {
            // 百分号编码按字节做：UTF-8 的每个字节各编一次，拼出来的是合法 URL。
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Some(format!("{AVATAR_BASE}/{encoded}.png?size={REQUEST_PX}"))
}

/// 抓头像的原始字节：超预算、非 2xx、读失败都返回 `None`。
fn http_fetch(url: &str, remaining: Duration) -> Option<Vec<u8>> {
    if remaining < MIN_BUDGET {
        return None;
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT.min(remaining))
        .timeout(remaining)
        .build();

    let response = agent
        .get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "image/png,image/jpeg,image/*;q=0.8")
        .set("Accept-Language", "en-US,en;q=0.9")
        .call()
        .ok()?;
    if !(200..300).contains(&response.status()) {
        return None;
    }

    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES)
        .read_to_end(&mut body)
        .ok()?;
    (!body.is_empty()).then_some(body)
}

/// 把一张图片的字节转成 40px PNG 的 data URI，返回 `(data_uri, PNG 原始字节数)`。
///
/// 纯函数：不碰网络、不碰文件，单测可以直接喂内存里的 PNG。解码失败（格式不认识）、
/// 原图太小、编码失败都返回 `None`，调用方留空由模板兜底。
fn to_data_uri(bytes: &[u8]) -> Option<(String, usize)> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    // 解压炸弹防线：几十 KB 的 PNG 可以解出上亿像素，先按尺寸和内存上限拦一道。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);

    let image = reader.decode().ok()?.into_rgba8();
    let (width, height) = fit(image.width(), image.height())?;
    // 统一先转 RGBA8 再缩：GitHub 的头像可能是 JPEG（无透明通道）或 PNG，
    // 输出 PNG 时颜色空间一致，模板里也不会有格式分支。
    let rgba = if (width, height) != image.dimensions() {
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };

    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            rgba.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
    Some((format!("data:image/png;base64,{encoded}"), png.len()))
}

/// 等比缩放到最长边正好 [`AVATAR_PX`]；原图本来就小就保持原样（不放大）。
///
/// 返回 `None` 表示原图小到没法用（跟踪像素之类）。
fn fit(width: u32, height: u32) -> Option<(u32, u32)> {
    if width < MIN_SOURCE_SIDE || height < MIN_SOURCE_SIDE {
        return None;
    }
    let longest = width.max(height);
    if longest <= AVATAR_PX {
        return Some((width, height));
    }
    let scale = f64::from(AVATAR_PX) / f64::from(longest);
    Some((
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    ))
}

/// 再放一张 `bytes` 字节的 PNG 会不会超出 [`TOTAL_BUDGET`]。
///
/// 用饱和加法：坏数据给出 `usize::MAX` 时也不能让判断回绕成「放得下」。
fn within_budget(used: usize, bytes: usize) -> bool {
    used.saturating_add(bytes) <= TOTAL_BUDGET
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 现场编码一张 PNG（不引 fixture 文件）：测试要的是「图片字节」这件事本身。
    fn png_of(image: &image::RgbaImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )
            .expect("编码测试 PNG");
        bytes
    }

    /// 伪随机噪点图：PNG 几乎压不动，用来把 40px 头像撑到几 KB，好测总预算。
    fn noise(width: u32, height: u32) -> image::RgbaImage {
        let mut seed = 0x1234_5678_u32;
        image::RgbaImage::from_fn(width, height, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let bytes = seed.to_le_bytes();
            image::Rgba([bytes[0], bytes[1], bytes[2], 255])
        })
    }

    fn items(count: usize) -> Vec<TrendingItem> {
        (0..count)
            .map(|index| TrendingItem {
                owner: format!("owner{index}"),
                name: format!("repo{index}"),
                ..TrendingItem::default()
            })
            .collect()
    }

    #[test]
    fn avatar_url_builds_the_github_path() {
        assert_eq!(
            avatar_url("vuejs").as_deref(),
            Some("https://github.com/vuejs.png?size=80")
        );
        // 合法的 GitHub 用户名 / 组织名里有连字符、点、下划线，都不该被编码。
        assert_eq!(
            avatar_url("my-org_1.2").as_deref(),
            Some("https://github.com/my-org_1.2.png?size=80")
        );
        assert_eq!(
            avatar_url("  torvalds  ").as_deref(),
            Some("https://github.com/torvalds.png?size=80")
        );
    }

    #[test]
    fn avatar_url_percent_encodes_unsafe_bytes() {
        assert_eq!(
            avatar_url("a b").as_deref(),
            Some("https://github.com/a%20b.png?size=80")
        );
        assert_eq!(
            avatar_url("a/b").as_deref(),
            Some("https://github.com/a%2Fb.png?size=80")
        );
        assert_eq!(
            avatar_url("a?b#c%").as_deref(),
            Some("https://github.com/a%3Fb%23c%25.png?size=80")
        );
        // 非 ASCII 按 UTF-8 字节百分号编码（用 = E7 94 A8，户 = E6 88 B7）。
        assert_eq!(
            avatar_url("用户").as_deref(),
            Some("https://github.com/%E7%94%A8%E6%88%B7.png?size=80")
        );
    }

    #[test]
    fn avatar_url_rejects_blank_owner() {
        assert_eq!(avatar_url(""), None);
        assert_eq!(avatar_url("   "), None);
    }

    #[test]
    fn to_data_uri_scales_the_longest_side_to_avatar_px() {
        let source = noise(160, 80);
        let (uri, bytes) = to_data_uri(&png_of(&source)).expect("转 data URI");

        assert!(uri.starts_with("data:image/png;base64,"), "{uri}");
        let encoded = uri.strip_prefix("data:image/png;base64,").expect("前缀");
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64");
        assert_eq!(png.len(), bytes, "统计值应该是 PNG 原始字节数");

        let decoded = image::load_from_memory(&png).expect("解码回图片");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (AVATAR_PX, AVATAR_PX / 2),
            "等比缩到最长边 40"
        );
    }

    #[test]
    fn to_data_uri_keeps_small_images_unchanged() {
        let source = image::RgbaImage::from_pixel(20, 10, image::Rgba([1, 2, 3, 255]));
        let (uri, _) = to_data_uri(&png_of(&source)).expect("转 data URI");
        let encoded = uri.strip_prefix("data:image/png;base64,").expect("前缀");
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64");
        let decoded = image::load_from_memory(&png).expect("解码回图片");
        assert_eq!((decoded.width(), decoded.height()), (20, 10), "不放大");
    }

    #[test]
    fn to_data_uri_rejects_garbage_and_tiny_images() {
        assert!(to_data_uri(b"not an image").is_none());
        assert!(to_data_uri(&[]).is_none());

        let tiny = image::RgbaImage::from_pixel(1, 1, image::Rgba([9, 9, 9, 255]));
        assert!(to_data_uri(&png_of(&tiny)).is_none(), "1×1 没有展示价值");
    }

    #[test]
    fn budget_is_a_hard_cap() {
        assert!(within_budget(0, 0));
        assert!(within_budget(0, TOTAL_BUDGET));
        assert!(within_budget(TOTAL_BUDGET - 1, 1));
        assert!(!within_budget(TOTAL_BUDGET, 1));
        // 坏数据给出 usize::MAX 时不能回绕成「放得下」。
        assert!(!within_budget(1, usize::MAX));
        assert!(!within_budget(usize::MAX, usize::MAX));
    }

    #[test]
    fn disabling_avatars_clears_stale_values_without_fetching() {
        let config = Config {
            avatar: false,
            ..Config::default()
        };
        let mut items = items(3);
        items[0].avatar = "data:image/png;base64,stale".to_owned();

        let filled = fill_report(&config, &mut items);
        assert_eq!(filled, Filled::default());
        assert!(items.iter().all(|item| item.avatar.is_empty()));
    }

    #[test]
    fn failures_leave_avatars_empty() {
        let config = Config::default();
        let mut items = items(3);
        let filled = fill_with(&config, &mut items, &|_: &str, _: Duration| None);
        assert_eq!(filled, Filled::default());
        assert!(items.iter().all(|item| item.avatar.is_empty()));
    }

    #[test]
    fn fill_spreads_the_work_across_threads() {
        let source = image::RgbaImage::from_pixel(80, 80, image::Rgba([20, 120, 220, 255]));
        let png = png_of(&source);
        let threads = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let seen = Arc::clone(&threads);
        let config = Config::default();
        let mut items = items(15);

        let filled = fill_with(&config, &mut items, &|_: &str, _: Duration| {
            seen.lock()
                .expect("记录线程")
                .insert(std::thread::current().id());
            // 稍微让一下，保证 6 个工作线程都有机会拿到任务（不然第一个线程可能全包了）。
            std::thread::sleep(Duration::from_millis(5));
            Some(png.clone())
        });

        assert_eq!(filled.ok, 15);
        assert_eq!(filled.bytes, 15 * png_size(&png));
        assert!(
            items
                .iter()
                .all(|item| item.avatar.starts_with("data:image/png;base64,"))
        );
        assert!(
            threads.lock().expect("读线程集合").len() >= 2,
            "15 条应该被多个工作线程分担"
        );
    }

    #[test]
    fn fill_stops_at_the_total_budget() {
        // 噪点图的 40px PNG 约 6 KiB，15 张远超 48 KiB，必然被截断。
        let png = png_of(&noise(80, 80));
        let config = Config::default();
        let mut items = items(15);

        let filled = fill_with(&config, &mut items, &|_: &str, _: Duration| {
            Some(png.clone())
        });

        assert!(filled.ok >= 1, "预算至少放得下一张");
        assert!(filled.ok < items.len(), "15 张噪点头像必然超预算");
        assert!(filled.bytes <= TOTAL_BUDGET);
        // 按名次顺序结算：前面的有头像，后面的留空。
        for item in &items[..filled.ok] {
            assert!(!item.avatar.is_empty());
        }
        for item in &items[filled.ok..] {
            assert!(item.avatar.is_empty());
        }
    }

    #[test]
    fn fill_gives_up_at_the_deadline() {
        let source = image::RgbaImage::from_pixel(80, 80, image::Rgba([1, 2, 3, 255]));
        let png = png_of(&source);
        let config = Config {
            avatar_timeout: Duration::from_millis(20),
            ..Config::default()
        };
        let mut items = items(15);
        let started = Instant::now();

        // 每次抓取都睡 80ms，远超 20ms 的总预算：一条都不该收。
        let filled = fill_with(&config, &mut items, &|_: &str, _: Duration| {
            std::thread::sleep(Duration::from_millis(80));
            Some(png.clone())
        });

        assert_eq!(filled, Filled::default());
        assert!(items.iter().all(|item| item.avatar.is_empty()));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "到点就该收工，不能等每个请求都跑完"
        );
    }

    /// 同一张 PNG 转出来的 data URI 字节数（预算测试里用来算期望值）。
    fn png_size(png: &[u8]) -> usize {
        to_data_uri(png).expect("转 data URI").1
    }
}
