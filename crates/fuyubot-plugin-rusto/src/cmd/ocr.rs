//! /ocr 的执行：定位图片 → 下载 → 识别 → 回复。
//!
//! 流程里每一步都可能失败，失败要变成一条人能看懂的回复，而不是静默：
//! 没图就告诉用户怎么发，模型缺失就告诉用户缺哪个文件。

use std::sync::Arc;

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

use crate::cmd::Job;
use crate::cmd::parser::Command;
use crate::engine::{EngineError, ImageInput, OcrRequest};
use crate::host::Ctx;
use crate::host::download;
use crate::host::messenger;
use crate::host::photo;

/// 执行 /ocr 家族命令。
pub fn handle(ctx: Arc<Ctx>, job: Job) {
    match &job.command {
        Command::Help => reply_plain(&ctx, &job, HELP),
        Command::Status => {
            let status = ctx.engine.status();
            reply_plain(&ctx, &job, &format!("引擎状态：{}", pretty(&status)));
        }
        Command::Warm => match ctx.engine.warm() {
            Ok(()) => reply_plain(&ctx, &job, "模型已加载，/ocr 可以直接用了。"),
            Err(error) => reply_plain(&ctx, &job, &format!("预热失败：{error}")),
        },
        Command::Path { path } => {
            let request = OcrRequest {
                image: ImageInput::Path(std::path::PathBuf::from(path)),
                detail: ctx.config.detail_default,
                max_side: ctx.config.max_side,
                timeout: ctx.config.timeout,
            };
            recognize_and_reply(&ctx, &job, request, ctx.config.detail_default);
        }
        Command::Ocr { detail } => {
            let Some(file_id) = resolve_file_id(&ctx, &job) else {
                reply_plain(&ctx, &job, NO_IMAGE);
                return;
            };
            let path = match download::file_path(&ctx.host, file_id, ctx.config.timeout) {
                Ok(path) => path,
                Err(error) => {
                    reply_plain(&ctx, &job, &format!("下载图片失败：{error}"));
                    return;
                }
            };
            let request = OcrRequest {
                image: ImageInput::Path(path),
                detail: *detail,
                max_side: ctx.config.max_side,
                timeout: ctx.config.timeout,
            };
            recognize_and_reply(&ctx, &job, request, *detail);
        }
    }
}

/// 没有图片时的提示。
const NO_IMAGE: &str = "请把图片发给我并回复 /ocr，或者直接在图片说明里写 /ocr。\n\n用法：/ocr、/ocr detail、/ocr status、/ocr warm、/ocr path <本机路径>";

/// 用法说明。
const HELP: &str = "RustO OCR\n\n- /ocr：识别图片文字（回复图片 / 图片说明写 /ocr）\n- /ocr detail：附行级置信度与坐标\n- /ocr status：引擎状态与最近耗时\n- /ocr warm：预加载模型\n- /ocr path <本机路径>：识别本机图片（排障）\n\n其它插件可以调共享服务 rusto.ocr 的 recognize 方法。";

/// 定位待识别的图片：消息自带，或者引用消息里的图片。
fn resolve_file_id(ctx: &Ctx, job: &Job) -> Option<i64> {
    if let Some(file_id) = job.incoming.file_id {
        return Some(file_id);
    }
    let reply_to = job.incoming.reply_to_message_id?;
    let request_id = ctx
        .host
        .invoke_value(
            "getMessage",
            json!({ "chat_id": job.incoming.chat_id, "message_id": reply_to }),
        )
        .ok()?;
    let message =
        match download::await_response(&ctx.host, request_id, ctx.config.timeout, "getMessage") {
            Ok(value) => value,
            Err(error) => {
                ctx.warn(&format!("rusto: 取引用消息失败：{error}"));
                return None;
            }
        };
    photo::file_id_of(message.get("content")?)
}

/// 识别并把结果回复到原消息。
fn recognize_and_reply(ctx: &Ctx, job: &Job, request: OcrRequest, detail: bool) {
    let placeholder = start_placeholder(ctx, job);
    let started = std::time::Instant::now();
    let output = match ctx.engine.recognize(request) {
        Ok(output) => output,
        Err(error) => {
            let message = match error {
                EngineError::ModelMissing(ref reason) => format!(
                    "模型缺失：{reason}\n\n把 PP-OCRv6 的 det / rec / dict 放到配置的 model_dir 下，或让插件联网下载一次。"
                ),
                other => format!("识别失败：{other}"),
            };
            finish(ctx, job, placeholder, &message);
            return;
        }
    };
    let elapsed = started.elapsed().as_millis() as u64;
    let body = if output.text.trim().is_empty() {
        "（没有识别到文字）".to_owned()
    } else {
        output.text.trim().to_owned()
    };
    let mut text = if detail {
        format!(
            "{body}\n\n—— {} 行，{} ms{}",
            output.lines.len(),
            elapsed,
            if output.truncated { "，已截断" } else { "" }
        )
    } else {
        body
    };
    if detail {
        let rows: Vec<String> = output
            .lines
            .iter()
            .take(40)
            .map(|line| format!("{:.2}  {}", line.score, line.text))
            .collect();
        if !rows.is_empty() {
            text.push_str("\n\n");
            text.push_str(&rows.join("\n"));
        }
    }
    finish(ctx, job, placeholder, &text);
}

/// 先回一条「识别中…」，拿到消息 id 供后面编辑。失败就退化成直接回复。
fn start_placeholder(ctx: &Ctx, job: &Job) -> Option<messenger::Sent> {
    if !ctx.config.reply_placeholder {
        return None;
    }
    match messenger::reply(
        &ctx.host,
        job.incoming.chat_id,
        job.incoming.message_id,
        "识别中…",
        ctx.config.timeout,
    ) {
        Ok(sent) if sent.message_id != 0 => Some(sent),
        Ok(_) => None,
        Err(error) => {
            ctx.warn(&format!("rusto: 发占位消息失败：{error}"));
            None
        }
    }
}

/// 把最终文本落到消息上：有占位就编辑，否则直接回复；超长自动分块。
fn finish(ctx: &Ctx, job: &Job, placeholder: Option<messenger::Sent>, text: &str) {
    let chunks = split(text, ctx.config.chunk_size);
    for (index, chunk) in chunks.iter().enumerate() {
        let body = if chunks.len() > 1 {
            format!("（{}/{}）\n{chunk}", index + 1, chunks.len())
        } else {
            chunk.clone()
        };
        if index == 0
            && let Some(sent) = placeholder
            && sent.message_id != 0
        {
            match messenger::edit(
                &ctx.host,
                sent.chat_id,
                sent.message_id,
                &body,
                ctx.config.timeout,
            ) {
                Ok(()) => continue,
                Err(error) => {
                    ctx.warn(&format!("rusto: 编辑占位消息失败，改为新消息：{error}"));
                }
            }
        }
        if let Err(error) = messenger::reply(
            &ctx.host,
            job.incoming.chat_id,
            job.incoming.message_id,
            &body,
            ctx.config.timeout,
        ) {
            ctx.error(&format!("rusto: 回复失败：{error}"));
            return;
        }
    }
}

/// 直接回复一段文本（不走占位/分块）。
fn reply_plain(ctx: &Ctx, job: &Job, text: &str) {
    if let Err(error) = messenger::reply(
        &ctx.host,
        job.incoming.chat_id,
        job.incoming.message_id,
        text,
        ctx.config.timeout,
    ) {
        ctx.error(&format!("rusto: 回复失败：{error}"));
    }
}

/// 按字符边界分块（不切坏 UTF-8）。
pub fn split(text: &str, limit: usize) -> Vec<String> {
    if text.chars().count() <= limit {
        return vec![text.to_owned()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for ch in text.chars() {
        if count == limit {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// 紧凑 JSON 打印（状态用）。
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// 宿主能力别名（上层与单测用）。
pub type Host = HostApi;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(split("你好", 10), vec!["你好".to_owned()]);
    }

    #[test]
    fn long_text_splits_on_char_boundaries() {
        let chunks = split("一二三四五六", 2);
        assert_eq!(
            chunks,
            vec!["一二".to_owned(), "三四".to_owned(), "五六".to_owned()]
        );
    }

    #[test]
    fn hints_are_not_empty() {
        assert!(!NO_IMAGE.is_empty());
        assert!(HELP.contains("/ocr detail"));
    }
}
