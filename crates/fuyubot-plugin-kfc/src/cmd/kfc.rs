//! `/kfc`：抓一句肯德基文案，渲染成推特卡片，以图片回复触发消息。
//!
//! 流程与 TS 版 `index.ts` 一一对应：
//! `axios.get(apiUrl)` → `normalizeContent` → `generateImage(kfc.vue, {content})`
//! → `sendMessage({photo})` → 删除临时文件。

use std::path::PathBuf;

use crate::core::{fetch, text};
use crate::host::{Ctx, Msg};
use crate::render;

/// 命令入口。
pub fn run(ctx: &Ctx, msg: Msg) {
    let content = match fetch::fetch_content(&ctx.config.api_url, ctx.config.timeout) {
        Ok(raw) => text::normalize(&raw),
        Err(error) => {
            ctx.warn(&format!("kfc: 拉取文案失败: {error}"));
            ctx.messenger.try_send_text(
                msg.chat_id,
                &format!("❌ 拿不到肯德基文案：{error}"),
                Some(msg.id),
            );
            return;
        }
    };

    let content = content.trim();
    if content.is_empty() {
        ctx.warn("kfc: 接口返回空文案");
        ctx.messenger
            .try_send_text(msg.chat_id, "❌ 今天的肯德基没有疯话", Some(msg.id));
        return;
    }

    let started = std::time::Instant::now();
    let png = match render::render_card(content, ctx.config.width) {
        Ok(png) => png,
        Err(error) => {
            ctx.error(&format!("kfc: 渲染失败: {error}"));
            ctx.messenger.try_send_text(
                msg.chat_id,
                &format!("❌ 卡片渲染失败：{error}"),
                Some(msg.id),
            );
            return;
        }
    };
    ctx.info(&format!(
        "kfc: 卡片已生成 {} bytes（{} ms, {} 字）",
        png.len(),
        started.elapsed().as_millis(),
        content.chars().count()
    ));

    let path = temp_path(msg);
    if let Err(error) = std::fs::write(&path, &png) {
        ctx.error(&format!("kfc: 写临时文件失败 {}: {error}", path.display()));
        ctx.messenger
            .try_send_text(msg.chat_id, "❌ 临时文件写入失败", Some(msg.id));
        return;
    }

    match ctx.messenger.send_photo(msg.chat_id, &path, Some(msg.id)) {
        Ok(sent) => ctx.info(&format!(
            "kfc: 已发送（chat={} message_id={}）",
            sent.chat_id, sent.message_id
        )),
        Err(error) => {
            ctx.error(&format!("kfc: 发送图片失败: {error}"));
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
            "kfc: 删除临时文件失败 {}: {error}",
            path.display()
        ));
    }
}

/// 临时文件路径：每次命令一个，重名也不会互相覆盖。
fn temp_path(msg: Msg) -> PathBuf {
    let name = format!("fuyubot-kfc-{}-{}.png", msg.chat_id, msg.id);
    std::env::temp_dir().join(name)
}
