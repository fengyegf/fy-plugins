//! 图片消息解析：判断消息内容族、取 caption、挑最大档图片、取引用关系。
//!
//! 形状依据：TDLib 1.8.x 的 `message.content`；`sizes` 里每档是
//! `{type, width, height, photo:{id, size, …}}`。**本仓没有图片消息先例**，
//! 落地时用 `testdata/replay_photo.json` 回放验证一次（设计文档 V1）。

use serde_json::Value;

/// 消息内容族。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    /// 图片。
    Photo,
    /// 文件（可能是图片）。
    Document,
    /// 动图。
    Animation,
    /// 贴纸。
    Sticker,
    /// 文本。
    Text,
    /// 其它（视频、语音……）。
    Other,
}

impl ContentKind {
    /// 从 `content.@type` 判定。
    pub fn from_type(kind: &str) -> Self {
        match kind {
            "messagePhoto" => Self::Photo,
            "messageDocument" => Self::Document,
            "messageAnimation" => Self::Animation,
            "messageSticker" => Self::Sticker,
            "messageText" => Self::Text,
            _ => Self::Other,
        }
    }

    /// 是否带可下载的图片文件。
    pub fn is_image(self) -> bool {
        matches!(self, Self::Photo | Self::Document | Self::Animation)
    }
}

/// 一条可能触发 OCR 的入站消息。
#[derive(Debug, Clone)]
pub struct Incoming {
    /// 会话 id（负数是群/频道，正数是私聊）。
    pub chat_id: i64,
    /// 消息 id。
    pub message_id: i64,
    /// 是否私聊（`chat_id > 0` 的近似判定，够用）。
    pub is_private: bool,
    /// 内容族。
    pub kind: ContentKind,
    /// 文本：文本消息取正文，媒体取 caption。
    pub text: String,
    /// 图片的 TDLib file id（文档/贴纸取 `file.id`）。
    pub file_id: Option<i64>,
    /// 引用回复的目标消息 id。
    pub reply_to_message_id: Option<i64>,
}

impl Incoming {
    /// 从 `updateNewMessage` 解析；不是消息就返回 `None`。
    pub fn parse(update: &Value) -> Option<Self> {
        if update.get("@type").and_then(Value::as_str) != Some("updateNewMessage") {
            return None;
        }
        let message = update.get("message")?;
        let content = message.get("content")?;
        let content_type = content.get("@type").and_then(Value::as_str)?;
        let kind = ContentKind::from_type(content_type);
        let chat_id = message.get("chat_id").and_then(Value::as_i64)?;
        Some(Self {
            chat_id,
            message_id: message.get("id").and_then(Value::as_i64)?,
            is_private: chat_id > 0,
            kind,
            text: text_of(content, content_type),
            file_id: file_id_of(content),
            reply_to_message_id: reply_to_of(message),
        })
    }
}

/// 正文：文本消息取 `text`，媒体取 `caption`（两者都是 formattedText）。
pub fn text_of(content: &Value, content_type: &str) -> String {
    let field = if content_type == "messageText" {
        "text"
    } else {
        "caption"
    };
    content
        .get(field)
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 取图片的 file id。
///
/// TDLib 的形状有个坑：`photo` 的每一档是 `photoSize{ photo: file }`，
/// 而 `document` / `animation` / `sticker` 的文件字段**与内容族同名**
/// （`document.document: file`、`animation.animation: file`、`sticker.sticker: file`），
/// 不是 `file`。这里按真实形状取。
pub fn file_id_of(content: &Value) -> Option<i64> {
    if let Some(sizes) = content
        .get("photo")
        .and_then(|photo| photo.get("sizes"))
        .and_then(Value::as_array)
    {
        return best_size(sizes)
            .and_then(|size| size.get("photo"))
            .and_then(|photo| photo.get("id"))
            .and_then(Value::as_i64);
    }
    // 内容族名 = 内层文件字段名。
    ["document", "animation", "sticker"]
        .iter()
        .find_map(|field| {
            content
                .get(*field)
                .and_then(|item| item.get(*field))
                .and_then(|file| file.get("id"))
                .and_then(Value::as_i64)
        })
}

/// 挑最大档：排除 `type == "c"`（抠图占位），按像素面积取最大。
fn best_size(sizes: &[Value]) -> Option<&Value> {
    sizes
        .iter()
        .filter(|size| size.get("type").and_then(Value::as_str) != Some("c"))
        .max_by_key(|size| {
            let width = size.get("width").and_then(Value::as_u64).unwrap_or(0);
            let height = size.get("height").and_then(Value::as_u64).unwrap_or(0);
            width.saturating_mul(height)
        })
}

/// 引用关系：只认 `messageReplyToMessage`。
fn reply_to_of(message: &Value) -> Option<i64> {
    let reply = message.get("reply_to")?;
    if reply.get("@type").and_then(Value::as_str) != Some("messageReplyToMessage") {
        return None;
    }
    reply.get("message_id").and_then(Value::as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn photo_update() -> Value {
        json!({
            "@type": "updateNewMessage",
            "message": {
                "@type": "message",
                "id": 101,
                "chat_id": 7770001,
                "content": {
                    "@type": "messagePhoto",
                    "caption": { "@type": "formattedText", "text": "/ocr", "entities": [] },
                    "photo": {
                        "@type": "photo",
                        "sizes": [
                            { "type": "s", "width": 320, "height": 180, "photo": { "id": 11 } },
                            { "type": "c", "width": 1600, "height": 900, "photo": { "id": 99 } },
                            { "type": "x", "width": 1280, "height": 720, "photo": { "id": 22 } }
                        ]
                    }
                }
            }
        })
    }

    #[test]
    fn parses_photo_and_skips_masked_size() {
        let incoming = Incoming::parse(&photo_update()).expect("应当解析出图片消息");
        assert_eq!(incoming.kind, ContentKind::Photo);
        assert_eq!(incoming.text, "/ocr");
        assert_eq!(incoming.file_id, Some(22), "要挑最大档且排除 type=c");
        assert!(incoming.is_private);
    }

    #[test]
    fn reads_caption_and_reply() {
        let mut update = photo_update();
        update["message"]["reply_to"] =
            json!({ "@type": "messageReplyToMessage", "message_id": 7 });
        let incoming = Incoming::parse(&update).unwrap();
        assert_eq!(incoming.reply_to_message_id, Some(7));
    }

    #[test]
    fn document_file_id_is_nested_under_document() {
        let update = json!({
            "@type": "updateNewMessage",
            "message": {
                "id": 9,
                "chat_id": 1,
                "content": {
                    "@type": "messageDocument",
                    "caption": { "@type": "formattedText", "text": "/ocr" },
                    "document": {
                        "@type": "document",
                        "file_name": "a.png",
                        "document": { "@type": "file", "id": 66 }
                    }
                }
            }
        });
        let incoming = Incoming::parse(&update).unwrap();
        assert_eq!(incoming.kind, ContentKind::Document);
        assert_eq!(incoming.file_id, Some(66));
    }

    #[test]
    fn sticker_file_id_is_nested_under_sticker() {
        let content = json!({
            "@type": "messageSticker",
            "sticker": { "@type": "sticker", "sticker": { "@type": "file", "id": 77 } }
        });
        assert_eq!(file_id_of(&content), Some(77));
    }

    #[test]
    fn text_message_is_not_an_image() {
        let update = json!({
            "@type": "updateNewMessage",
            "message": {
                "id": 5,
                "chat_id": -100,
                "content": { "@type": "messageText", "text": { "text": "/ocr" } }
            }
        });
        let incoming = Incoming::parse(&update).unwrap();
        assert_eq!(incoming.kind, ContentKind::Text);
        assert!(!incoming.kind.is_image());
        assert_eq!(incoming.file_id, None);
        assert!(!incoming.is_private);
    }
}
