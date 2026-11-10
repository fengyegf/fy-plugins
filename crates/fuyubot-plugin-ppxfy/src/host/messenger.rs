//! 消息收发：把 ABI v5 的 `simplify` 操作表包成几个直白的方法。
//!
//! 所有方法都是**阻塞**的：先用 `simplify_invoke` 拿到请求号，再在 `response`
//! 通道上等这个请求号的结果。因此只能在自己的业务线程里调用，绝不能在宿主回调里调。
//!
//! `args` 一律按 TDLib 原生字段名书写（`td_api.tl` 为准），详见
//! `fuyubot-sdk td-method sendMessage`。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};
use thiserror::Error;

/// 一次发送得到的消息定位信息。
#[derive(Debug, Clone, Copy)]
pub struct Sent {
    pub chat_id: i64,
    pub message_id: i64,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("调用宿主失败: {0}")]
    Host(String),
    #[error("等待 TDLib 响应超时")]
    Timeout,
    #[error("操作被拒绝（{code}）: {message}")]
    Op { code: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// 一段文本 + 样式。
///
/// 样式由插件算好、直接拼成 `formattedText` 的 entities，**不走** `parseTextEntities`：
/// 一是 TDLib 的两套 Markdown 有版本坑（`*x*` 才是粗体、引用块要收尾 `||` 才折叠），
/// 二是用户文本里的 `*_|#` 不会再把排版写坏——纯文本永远只是纯文本。
#[derive(Debug, Clone)]
pub struct Piece {
    text: String,
    style: Style,
}

/// 片段样式，一一对应 TDLib 的 `TextEntityType`。
#[derive(Debug, Clone)]
pub enum Style {
    /// 普通文字。
    Plain,
    /// 行内代码（Telegram 里点一下即可整段复制）。
    Code,
    /// 加粗。
    Bold,
    /// 可折叠引用块：客户端默认收成 3 行，点开展开全文。
    Quote,
    /// 显示成这段文字、点击打开 `url`。
    Link(String),
}

impl Piece {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::Plain,
        }
    }

    pub fn code(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::Code,
        }
    }

    pub fn bold(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::Bold,
        }
    }

    /// 折叠引用块。换行原样保留，实体覆盖整段。
    pub fn quote(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::Quote,
        }
    }

    pub fn link(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::Link(url.into()),
        }
    }

    /// 片段明文（用于算消息长度）。
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Style {
    /// 这段文字该挂哪个实体；普通文字与空片段（长度 0）不挂。
    fn entity(&self, offset: i64, length: i64) -> Option<Value> {
        let kind = match self {
            Self::Plain => return None,
            Self::Code => json!({ "@type": "textEntityTypeCode" }),
            Self::Bold => json!({ "@type": "textEntityTypeBold" }),
            // TDLib 里的写法是 `BlockQuote`（大写 Q），照 `td_api.tl` 抄。
            Self::Quote => json!({ "@type": "textEntityTypeExpandableBlockQuote" }),
            Self::Link(url) => json!({ "@type": "textEntityTypeTextUrl", "url": url }),
        };
        Some(json!({
            "@type": "textEntity",
            "offset": offset,
            "length": length,
            "type": kind,
        }))
    }
}

/// TDLib 的 `textEntity.offset / length` 按 UTF-16 码元计数，不能直接取字节长度。
fn utf16_len(text: &str) -> i64 {
    text.encode_utf16().count() as i64
}

/// 拼出 `formattedText`：每段按样式挂实体。
fn formatted_text(pieces: &[Piece]) -> Value {
    let mut text = String::new();
    let mut entities = Vec::new();
    let mut offset = 0i64;

    for piece in pieces {
        let length = utf16_len(&piece.text);
        // 空片段的实体长度是 0，TDLib 会直接判非法，丢掉。
        if let Some(entity) = piece.style.entity(offset, length) {
            entities.push(entity);
        }
        text.push_str(&piece.text);
        offset += length;
    }

    json!({ "@type": "formattedText", "text": text, "entities": entities })
}

/// 文本消息的 `inputMessageContent`（纯文本，不挂实体）。
fn text_content(text: &str) -> Value {
    content(formatted_text(&[Piece::plain(text)]))
}

fn content(text: Value) -> Value {
    json!({ "@type": "inputMessageText", "text": text })
}

/// 本地文件 -> `inputMessageDocument`，用于上传压缩包。
fn document_content(path: &std::path::Path) -> Value {
    json!({
        "@type": "inputMessageDocument",
        "document": {
            "@type": "inputDocument",
            "document": { "@type": "inputFileLocal", "path": path.to_string_lossy() }
        }
    })
}

/// 上传超时：大分卷动辄几个 GB，比普通消息放宽得多。
const UPLOAD_TIMEOUT_MS: u64 = 1_800_000;

/// 编辑去重表的上限：超过就整表清空，避免长期运行下无限增长。
const EDIT_CACHE_LIMIT: usize = 256;

/// 消息网关。`HostApi` 是 `Copy`，套一层 `Arc` 只是为了带上编辑去重表。
#[derive(Debug, Clone)]
pub struct Messenger {
    host: HostApi,
    /// 每条消息上一次发出去的文本：内容没变就不发请求。
    ///
    /// TDLib 对「内容相同的 edit」会直接返回 `MESSAGE_NOT_MODIFIED`（400），
    /// 进度刷新这类高频调用必须先在这里挡掉。
    last_edits: Arc<Mutex<HashMap<(i64, i64), String>>>,
}

impl Messenger {
    pub fn new(host: HostApi) -> Self {
        Self {
            host,
            last_edits: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 发送一条纯文本消息，`reply_to` 填消息 id 即「回复」。
    pub fn send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Result<Sent> {
        self.send_content(chat_id, text_content(text), reply_to)
    }

    /// 发送一条纯文本消息，失败只记日志、返回 `None`。
    pub fn try_send_text(&self, chat_id: i64, text: &str, reply_to: Option<i64>) -> Option<Sent> {
        self.try_send_content(chat_id, text_content(text), reply_to)
    }

    /// 发送一条带样式的消息（加粗 / 引用块 / 链接 / 行内代码）。
    pub fn send_styled(&self, chat_id: i64, pieces: &[Piece]) -> Result<Sent> {
        self.send_content(chat_id, content(formatted_text(pieces)), None)
    }

    /// 发送一条带样式的消息，失败只记日志、返回 `None`。
    pub fn try_send_rich(
        &self,
        chat_id: i64,
        pieces: &[Piece],
        reply_to: Option<i64>,
    ) -> Option<Sent> {
        self.try_send_content(chat_id, content(formatted_text(pieces)), reply_to)
    }

    fn try_send_content(
        &self,
        chat_id: i64,
        content: Value,
        reply_to: Option<i64>,
    ) -> Option<Sent> {
        match self.send_content(chat_id, content, reply_to) {
            Ok(sent) => Some(sent),
            Err(error) => {
                self.host.warn(&format!("ppxfy: 发送消息失败: {error}"));
                None
            }
        }
    }

    fn send_content(&self, chat_id: i64, content: Value, reply_to: Option<i64>) -> Result<Sent> {
        let mut args = json!({
            "chat_id": chat_id,
            "input_message_content": content,
        });
        if let Some(message_id) = reply_to {
            args["reply_to"] = json!({
                "@type": "inputMessageReplyToMessage",
                "message_id": message_id,
            });
        }
        let message = self.op("send_message", args, 30_000)?;
        Ok(Sent {
            chat_id,
            message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
        })
    }

    /// 编辑一条文本消息；用于所有进度与交互提示，失败不抛出。
    ///
    /// 文案与上次完全相同就直接跳过——这既是性能优化，也是 `MESSAGE_NOT_MODIFIED`
    /// 的根因所在（TDLib 会把它当错误返回）。
    pub fn edit_text(&self, chat_id: i64, message_id: i64, text: &str) {
        self.edit_content(chat_id, message_id, text_content(text));
    }

    fn edit_content(&self, chat_id: i64, message_id: i64, content: Value) {
        // 去重键用整段内容（含实体）：文案一样但实体不同也算变化。
        let text = content.to_string();
        let key = (chat_id, message_id);
        {
            let mut last = self
                .last_edits
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if last.get(&key).is_some_and(|previous| previous == &text) {
                return;
            }
            if last.len() >= EDIT_CACHE_LIMIT {
                last.clear();
            }
            last.insert(key, text);
        }

        let args = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "input_message_content": content,
        });
        match self.op("edit_message_text", args, 30_000) {
            Ok(_) => {}
            // 并发下仍可能撞上「内容其实没变」，属于正常现象，不必刷日志。
            Err(Error::Op { message, .. }) if message.contains("MESSAGE_NOT_MODIFIED") => {
                self.host
                    .info(&format!("ppxfy: 消息内容未变化，跳过编辑 ({message_id})"));
            }
            Err(error) => {
                // 失败就别缓存，否则后续同样的文案会被误判成「已发过」。
                self.last_edits
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&key);
                self.host.warn(&format!("ppxfy: 编辑消息失败: {error}"));
            }
        }
    }

    /// 删除消息（`/cancel` 清理用户回复也用它）。
    pub fn delete_messages(&self, chat_id: i64, message_ids: &[i64]) {
        if message_ids.is_empty() {
            return;
        }
        let args = json!({
            "chat_id": chat_id,
            "message_ids": message_ids,
            "revoke": true,
        });
        if let Err(error) = self.op("delete_messages", args, 30_000) {
            self.host.warn(&format!("ppxfy: 删除消息失败: {error}"));
        }
    }

    /// 以文件（document）形式上传一个本地文件。上传耗时远大于普通消息，超时放宽。
    pub fn send_document(&self, chat_id: i64, path: &std::path::Path) -> Result<Sent> {
        let args = json!({
            "chat_id": chat_id,
            "input_message_content": document_content(path),
        });
        let message = self.op("send_message", args, UPLOAD_TIMEOUT_MS)?;
        Ok(Sent {
            chat_id,
            message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
        })
    }

    /// 以照片形式发一条消息：本地文件 + 可选说明（带样式）+ 可选剧透遮罩。
    ///
    /// 这是 TDLib 的「上传本地照片」分支：`inputPhoto` 里塞 `inputFileLocal`，
    /// 宽高交给服务器替换（客户端拿不到真图尺寸，也不必为了它去解一遍图片）。
    pub fn send_photo(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        caption: Option<&[Piece]>,
        spoiler: bool,
    ) -> Result<Sent> {
        let mut content = json!({
            "@type": "inputMessagePhoto",
            "photo": {
                "@type": "inputPhoto",
                "photo": { "@type": "inputFileLocal", "path": path.to_string_lossy() },
                "width": 0,
                "height": 0,
            },
            "has_spoiler": spoiler,
        });
        if let Some(pieces) = caption {
            content["caption"] = formatted_text(pieces);
        }
        let args = json!({
            "chat_id": chat_id,
            "input_message_content": content,
        });
        let message = self.op("send_message", args, UPLOAD_TIMEOUT_MS)?;
        Ok(Sent {
            chat_id,
            message_id: message.get("id").and_then(Value::as_i64).unwrap_or(0),
        })
    }

    /// 以相册（媒体组）形式一次上传 2–10 个文件。
    ///
    /// 走 `send_message_album`（`sendMessageAlbum`）：整组共用一个超时预算、逐项等
    /// 发送结局，任一项失败即返回错误。调用方负责把条数控制在 2–10。
    pub fn send_album(&self, chat_id: i64, paths: &[std::path::PathBuf]) -> Result<()> {
        debug_assert!((2..=10).contains(&paths.len()), "相册只能是 2–10 个文件");
        let contents: Vec<Value> = paths.iter().map(|path| document_content(path)).collect();
        let args = json!({
            "chat_id": chat_id,
            "input_message_contents": contents,
        });
        self.op("send_message_album", args, UPLOAD_TIMEOUT_MS)
            .map(|_| ())
    }

    /// 跑一个 simplify 操作并等它的结果。
    ///
    /// 结果信封形如
    /// `{"@type":"simplifyResult","op":…,"ok":true,"result":…,"request_id":N}`，
    /// 成功失败同形状，只看 `ok`。
    fn op(&self, name: &str, args: Value, timeout_ms: u64) -> Result<Value> {
        let mut args = args;
        args["timeout_ms"] = json!(timeout_ms);

        let request_id = self
            .host
            .simplify_invoke_json(json!({ "op": name, "args": args }))
            .map_err(|error| Error::Host(error.to_string()))?;

        // 多给 5 秒余量：宿主要先等 TDLib，再回投响应。
        let filter = json!({ "event_type": "response", "request_id": request_id.id });
        let response = self
            .host
            .wait_for_update_json(filter, timeout_ms + 5_000)
            .ok_or(Error::Timeout)?;

        if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
        let error = response.get("error").cloned().unwrap_or(Value::Null);
        Err(Error::Op {
            code: error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未提供错误信息")
                .to_owned(),
        })
    }
}
