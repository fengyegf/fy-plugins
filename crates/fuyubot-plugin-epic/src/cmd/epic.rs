//! `/epic`：发现 Epic 免费游戏并出图。
//!
//! 流程：抓促销接口 → 截断到配置上限 → 生成 Vue 模板的 `data` → 调
//! `fuyubot.html2img` 渲染 → 发照片 → 删服务产物。界面在 `assets/card.html`；
//! 封面不下载、不合成：模板直接把 Epic CDN 的 URL 写进 `<img src>`，由渲染服务
//! 去拉取。
//!
//! 渲染失败不算致命：降级发一条纯文本清单，至少把信息送到。

use std::sync::Arc;

use crate::core::epic::{self, FreeGames, Query};
use crate::core::fetch::HttpClient;
use crate::host::{Ctx, Msg};
use crate::render::{self, RenderOptions};

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

    let options = options(ctx);
    let path = match render::render_card(&ctx.host, &games, &options) {
        Ok(path) => path,
        Err(error) => {
            // 渲染是「锦上添花」：服务出问题也要把游戏清单送到，别让用户什么都收不到。
            ctx.error(&format!("epic: 渲染卡片失败，降级为文本: {error}"));
            send_text_list(ctx, msg, &games);
            return Ok(());
        }
    };

    if ctx.is_shutdown() {
        // 图片已经渲出来了，停机不走发送流程也要把它删掉。
        remove_artifact(ctx, &path);
        return Ok(());
    }

    let caption = (!config.caption.trim().is_empty()).then_some(config.caption.as_str());
    let sent = ctx
        .messenger
        .send_photo(msg.chat_id, &path, caption, Some(msg.id));
    // TDLib 的上传在 sendMessage 返回时已完成，之后文件就没用了；先删再处理结果。
    remove_artifact(ctx, &path);
    let sent = sent.map_err(|error| format!("发送图片失败: {error}"))?;

    ctx.info(&format!(
        "epic: 已发送卡片图（消息 {}，{}，配置缩放 {}，{games}）",
        sent.message_id,
        path.display(),
        options.scale
    ));
    Ok(())
}

/// 删除 html2img 的产物：它在服务临时目录里（24 小时后服务也会自己清），
/// 上传完成后就没用了。删失败只记日志，不影响命令结果。
fn remove_artifact(ctx: &Ctx, path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => ctx.warn(&format!(
            "epic: 删除渲染产物失败（{}）: {error}",
            path.display()
        )),
    }
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
