//! `/kfc`：抓一句肯德基文案，渲染成推特卡片，以图片回复触发消息。
//!
//! 流程与 TS 版 `index.ts` 一一对应：
//! `axios.get(apiUrl)` → `normalizeContent` → `generateImage(kfc.vue, {content})`
//! → `sendMessage({photo})` → 删除临时文件。
//!
//! 渲染走宿主上的 `fuyubot.html2img` 服务（见 [`crate::render`]）：插件只拼 HTML，
//! 不链接渲染引擎；产物是服务临时目录里的文件，发完就删。

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
    let path = match render::render_card(&ctx.host, content, ctx.config.width) {
        Ok(path) => path,
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
        "kfc: 卡片已生成 {}（{} ms, {} 字）",
        path.display(),
        started.elapsed().as_millis(),
        content.chars().count()
    ));

    let sent = ctx.messenger.send_photo(msg.chat_id, &path, Some(msg.id));

    // html2img 的产物在服务临时目录里，24 小时后也会被服务自己清掉；上传完成后
    // 文件就没用了，发完就删，别在临时目录里堆图。删失败只记日志。
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => ctx.warn(&format!(
            "kfc: 删除渲染产物失败 {}: {error}",
            path.display()
        )),
    }

    match sent {
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
}
