//! `/whois`：查询域名 whois 信息，渲染成信息卡片，以图片回复触发消息。
//!
//! 命令执行流程：
//!
//! ```text
//! /whois example.com
//!   → domain::normalize()        // core::domain
//!   → GET /api/whois/info/?raw=1 // core::fetch
//!   → GET /api/whois/ip/         // 失败不阻塞
//!   → GET <domain>/favicon.ico   // 失败回退内置图标
//!   → render::render_card()      // fuyubot.html2img 服务出图
//!   → sendMessage({photo})       // host::messenger
//!   → 删除 html2img 产物
//! ```
//!
//! 三个分支：接口报错 → 提示失败；`is_available == 1` → 提示未注册；其余 → 出卡片。

use std::time::Instant;

use crate::core::model::CardData;
use crate::core::{domain, favicon, fetch};
use crate::host::{Ctx, Msg};
use crate::render;

/// 没带域名时的用法提示。
const USAGE: &str = "请提供要查询的域名，例如 /whois example.com";
/// 域名清洗失败时的提示。
const BAD_DOMAIN: &str = "域名格式不正确，请输入例如 /whois example.com";
/// 未注册提示。
const NOT_REGISTERED: &str = "域名未被注册，未找到相关信息。";
/// 查询失败的兜底提示。
const QUERY_FAILED: &str = "域名信息查询失败，请稍后再试。";

/// 命令入口。`arg` 是 `/whois` 之后的第一个参数（可能是完整 URL）。
pub fn run(ctx: &Ctx, msg: Msg, arg: Option<String>) {
    if ctx.is_shutdown() {
        return;
    }
    let Some(raw) = arg
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        ctx.messenger
            .try_send_text(msg.chat_id, USAGE, Some(msg.id));
        return;
    };

    let Some(domain) = domain::normalize(raw) else {
        ctx.messenger
            .try_send_text(msg.chat_id, BAD_DOMAIN, Some(msg.id));
        return;
    };

    let started = Instant::now();
    let query = match fetch::query_whois(&ctx.config, &domain) {
        Ok(query) => query,
        // 接口明确说了原因（“域名不合法”之类），原样转给用户比一句“请稍后再试”有用。
        Err(fetch::Error::Api(reason)) => {
            ctx.warn(&format!("whois: 查询 {domain} 被接口拒绝: {reason}"));
            ctx.messenger.try_send_text(
                msg.chat_id,
                &format!("❌ 域名信息查询失败：{reason}"),
                Some(msg.id),
            );
            return;
        }
        Err(error) => {
            ctx.error(&format!("whois: 查询 {domain} 失败: {error}"));
            ctx.messenger
                .try_send_text(msg.chat_id, QUERY_FAILED, Some(msg.id));
            return;
        }
    };

    let whois = match query {
        fetch::WhoisQuery::Available => {
            ctx.info(&format!("whois: {domain} 未注册"));
            ctx.messenger
                .try_send_text(msg.chat_id, NOT_REGISTERED, Some(msg.id));
            return;
        }
        fetch::WhoisQuery::Registered(whois) => *whois,
    };

    // 停机后不再发起新请求：插件 stop 时会 join 业务线程，每个阶段边界检查一次
    // 可以把「停不下来」的最坏时间从「HTTP×2 + 渲染 + 上传」压到当前这一步。
    if ctx.is_shutdown() {
        return;
    }

    // IP 归属只是补充信息：拿不到不影响卡片主体。
    let ip = fetch::query_ip(&ctx.config, &domain);
    if ip.is_none() {
        ctx.info(&format!(
            "whois: {domain} 的 IP 归属查询失败，卡片跳过该组数据"
        ));
    }
    if ctx.is_shutdown() {
        return;
    }
    let card = CardData::new(&whois, ip.as_ref());

    if ctx.is_shutdown() {
        return;
    }

    // 网站图标只是点缀：抓不到就用内置的渐变地球，不阻断卡片。
    let icon = favicon::fetch(&ctx.config, &domain);
    match &icon {
        Some(icon) => ctx.info(&format!(
            "whois: {domain} 已获取网站图标 {}×{}",
            icon.width, icon.height
        )),
        None if ctx.config.favicon => {
            ctx.info(&format!("whois: {domain} 没有取到网站图标，使用内置图标"));
        }
        None => {}
    }

    // 出图交给宿主上的 fuyubot.html2img：插件只把卡片数据映射成 Vue 模板的 data，
    // 产物是服务临时目录里的本机文件（24 小时内有效）。这是阻塞调用，正好跑在
    // 当前业务线程上。
    let path = match render::render_card(&ctx.host, &card, ctx.config.width, icon.as_ref()) {
        Ok(path) => path,
        Err(error) => {
            ctx.error(&format!("whois: 卡片渲染失败（{domain}）: {error}"));
            ctx.messenger.try_send_text(
                msg.chat_id,
                &format!("❌ 卡片渲染失败：{error}"),
                Some(msg.id),
            );
            return;
        }
    };
    ctx.info(&format!(
        "whois: 卡片已生成 {}（{} ms，{} 状态 / {} DNS）",
        path.display(),
        started.elapsed().as_millis(),
        card.domain_status.len(),
        card.name_server.len()
    ));

    if ctx.is_shutdown() {
        return;
    }

    let sent = ctx.messenger.send_photo(msg.chat_id, &path, Some(msg.id));

    // 产物在 html2img 的临时目录里，上传完成后就没用了；发完就删，别堆图。
    // 删除失败只记日志（服务自己也会在 24 小时后清理）。
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => ctx.warn(&format!(
            "whois: 删除渲染产物失败 {}: {error}",
            path.display()
        )),
    }

    match sent {
        Ok(sent) => ctx.info(&format!(
            "whois: 已发送 {domain} 卡片（chat={} message_id={}）",
            sent.chat_id, sent.message_id
        )),
        Err(error) => {
            ctx.error(&format!("whois: 发送图片失败（{domain}）: {error}"));
            ctx.messenger.try_send_text(
                msg.chat_id,
                &format!("❌ 图片发送失败：{error}"),
                Some(msg.id),
            );
        }
    }
}
