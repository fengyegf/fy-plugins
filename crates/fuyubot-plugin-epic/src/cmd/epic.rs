//! `/epic`：发现 Epic 免费游戏并出图。
//!
//! 流程：抓促销接口 → 截断到配置上限 → 并发下载封面（失败就留占位）→
//! hyper-render 渲染卡片 → 发照片 → 删临时文件。
//!
//! 渲染失败不算致命：降级发一条纯文本清单，至少把信息送到。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use fuyubot_plugin_api::{HostApi, HostExt};

use crate::core::epic::{self, FreeGame, FreeGames, Query};
use crate::core::fetch::HttpClient;
use crate::host::{Ctx, Msg};
use crate::render::{self, RenderOptions};

/// 单张封面上限：Epic 的宽图通常几百 KB，12 MiB 足够且能挡住异常大文件。
const MAX_COVER_BYTES: u64 = 12 * 1024 * 1024;

/// 封面并发下载数：8 张图串行要好几秒，4 路足够又不会把 CDN 打疼。
const COVER_CONCURRENCY: usize = 4;

/// 命令入口。出错只影响本次流程，不向外传播。
pub fn run(ctx: &Arc<Ctx>, msg: Msg) {
    if let Err(error) = flow(ctx, msg) {
        ctx.error(&format!("epic: 处理 /epic 失败: {error}"));
        ctx.messenger
            .try_send_text(msg.chat_id, "获取Epic免费游戏信息失败。", Some(msg.id));
    }
}

/// 主流程。
fn flow(ctx: &Ctx, msg: Msg) -> Result<(), String> {
    let config = &ctx.config;

    let client = HttpClient::new(config.proxy.as_deref(), config.timeout, &config.user_agent)
        .map_err(|error| format!("初始化 HTTP 客户端失败: {error}"))?;

    let query = Query {
        api_url: config.api_url.clone(),
        locale: config.locale.clone(),
        country: config.country.clone(),
        tz_offset_hours: config.timezone_offset_hours,
    };
    let games =
        epic::fetch(&client, &query).map_err(|error| format!("拉取促销接口失败: {error}"))?;
    if ctx.is_shutdown() {
        return Ok(());
    }
    let games = trim(games, config.max_cards);
    ctx.info(&format!("epic: 接口返回 {games}"));

    if games.is_empty() {
        ctx.messenger
            .send_text(msg.chat_id, "当前没有 Epic 免费游戏。", Some(msg.id))
            .map_err(|error| format!("发送提示失败: {error}"))?;
        return Ok(());
    }

    let covers = download_covers(ctx, &client, &games);
    if ctx.is_shutdown() {
        return Ok(());
    }
    let options = options(ctx);
    let png = match render::render(&games, &covers, &options) {
        Ok(png) => png,
        Err(error) => {
            // 渲染是"锦上添花"：引擎出问题也要把游戏清单送到，别让用户什么都收不到。
            ctx.error(&format!("epic: 渲染卡片失败，降级为文本: {error}"));
            send_text_list(ctx, msg, &games);
            return Ok(());
        }
    };

    if ctx.is_shutdown() {
        return Ok(());
    }
    // 临时图用 RAII 守卫：正常返回、提前 return、panic unwind 都会删掉它。
    let image = TempImage::create(ctx, temp_path(ctx, msg), &png)?;

    let caption = (!config.caption.trim().is_empty()).then_some(config.caption.as_str());
    let sent = ctx
        .messenger
        .send_photo(msg.chat_id, &image.path, caption, Some(msg.id));
    // TDLib 的上传在 sendMessage 返回时已完成，之后文件就没用了；先删再处理结果。
    drop(image);
    let sent = sent.map_err(|error| format!("发送图片失败: {error}"))?;

    ctx.info(&format!(
        "epic: 已发送卡片图（消息 {}，{} 字节，缩放 {}，{games}）",
        sent.message_id,
        png.len(),
        options.scale
    ));
    Ok(())
}

/// 逐张下载封面；任何一张失败都只记日志，保留占位。
///
/// 用 `std::thread::scope` 分片并发：`HttpClient` 内部是 `ureq::Agent`，可共享。
fn download_covers(ctx: &Ctx, client: &HttpClient, games: &FreeGames) -> Vec<Option<Vec<u8>>> {
    let items: Vec<&FreeGame> = games.iter().collect();
    let total = items.len();
    let slots: Mutex<Vec<Option<Vec<u8>>>> = Mutex::new((0..total).map(|_| None).collect());
    let next = Mutex::new(0usize);
    let workers = COVER_CONCURRENCY.min(total).max(1);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = {
                        let mut cursor = next.lock().unwrap_or_else(|e| e.into_inner());
                        if *cursor >= total {
                            break;
                        }
                        *cursor += 1;
                        *cursor - 1
                    };
                    if ctx.is_shutdown() {
                        break;
                    }
                    let game = items[index];
                    let Some(url) = game.offer_image.as_deref() else {
                        continue;
                    };
                    match client.get_bytes(url, MAX_COVER_BYTES) {
                        Ok(bytes) => {
                            slots.lock().unwrap_or_else(|e| e.into_inner())[index] = Some(bytes);
                        }
                        Err(error) => {
                            ctx.warn(&format!("epic: 封面下载失败（{}）: {error}", game.title))
                        }
                    }
                }
            });
        }
    });

    slots.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// 渲染失败的降级路径：发一条纯文本清单。
fn send_text_list(ctx: &Ctx, msg: Msg, games: &FreeGames) {
    let mut text = String::from("Epic 免费游戏\n");
    if !games.current.is_empty() {
        text.push_str("\n【当前免费】\n");
        for game in &games.current {
            text.push_str(&format!("· {}（截止 {}）\n", game.title, game.end_date));
        }
    }
    if !games.upcoming.is_empty() {
        text.push_str("\n【即将免费】\n");
        for game in &games.upcoming {
            text.push_str(&format!(
                "· {}（{} – {}）\n",
                game.title, game.start_date, game.end_date
            ));
        }
    }
    ctx.messenger
        .try_send_text(msg.chat_id, text.trim_end(), Some(msg.id));
}

/// 配置 → 渲染参数。
fn options(ctx: &Ctx) -> RenderOptions {
    let config = &ctx.config;
    RenderOptions {
        scale: config.scale,
        card_width: config.card_width,
        cover_height: config.cover_height,
        max_per_row: config.max_per_row,
        max_cards: config.max_cards,
        ..RenderOptions::default()
    }
}

/// 按配置上限截断（当前免费优先，剩下的名额给即将免费）。
fn trim(games: FreeGames, max: usize) -> FreeGames {
    let current: Vec<_> = games.current.into_iter().take(max).collect();
    let left = max.saturating_sub(current.len());
    let upcoming = games.upcoming.into_iter().take(left).collect();
    FreeGames { current, upcoming }
}

/// 临时文件路径。
///
/// 用 `chat_id + 命令消息 id` 而不是时间戳：同一条命令重投/并发重复执行时也只会
/// 覆盖同名文件，不会在缓存目录里堆一堆互不相干的图。
fn temp_path(ctx: &Ctx, msg: Msg) -> PathBuf {
    ctx.config
        .cache_dir
        .join(format!("epic-{}-{}.png", msg.chat_id, msg.id))
}

/// 临时图守卫：`Drop` 时删除文件，unwind 也能兜住。
struct TempImage {
    path: PathBuf,
    host: HostApi,
}

impl TempImage {
    /// 落盘并交给守卫；目录不存在就建。
    fn create(ctx: &Ctx, path: PathBuf, bytes: &[u8]) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建缓存目录失败: {error}"))?;
        }
        std::fs::write(&path, bytes).map_err(|error| format!("写入渲染结果失败: {error}"))?;
        Ok(Self {
            path,
            host: ctx.host,
        })
    }
}

impl Drop for TempImage {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            // 已经不在了（比如被手工清理）不算错误。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => self.host.warn(&format!(
                "epic: 删除临时文件失败（{}）: {error}",
                self.path.display()
            )),
        }
    }
}
