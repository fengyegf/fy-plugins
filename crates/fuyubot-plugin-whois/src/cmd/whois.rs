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
//!   → render::render_card()      // hyper-render
//!   → sendMessage({photo})       // host::messenger
//!   → 删除临时文件
//! ```
//!
//! 三个分支：接口报错 → 提示失败；`is_available == 1` → 提示未注册；其余 → 出卡片。

use std::path::PathBuf;
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

    let png = match render::render_card(&card, ctx.config.width, icon.as_ref()) {
        Ok(png) => png,
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
        "whois: 卡片已生成 {} bytes（{} ms，{} 状态 / {} DNS）",
        png.len(),
        started.elapsed().as_millis(),
        card.domain_status.len(),
        card.name_server.len()
    ));

    if ctx.is_shutdown() {
        return;
    }

    let path = temp_path(msg);
    if let Err(error) = std::fs::write(&path, &png) {
        ctx.error(&format!(
            "whois: 写临时文件失败 {}: {error}",
            path.display()
        ));
        ctx.messenger
            .try_send_text(msg.chat_id, "❌ 临时文件写入失败", Some(msg.id));
        return;
    }

    match ctx.messenger.send_photo(msg.chat_id, &path, Some(msg.id)) {
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

    // 上传走的是本地文件，发完就删；失败也删，别在系统临时目录里堆图。
    if let Err(error) = std::fs::remove_file(&path) {
        ctx.warn(&format!(
            "whois: 删除临时文件失败 {}: {error}",
            path.display()
        ));
    }
}

/// 临时文件路径：每次命令一个，重名也不会互相覆盖。
fn temp_path(msg: Msg) -> PathBuf {
    let name = format!("fuyubot-whois-{}-{}.png", msg.chat_id, msg.id);
    std::env::temp_dir().join(name)
}
