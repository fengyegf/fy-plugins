//! fuyubot.html2img 的**软依赖** + 用户名片卡片。
//!
//! 软依赖：宿主上装了 fuyubot.html2img 就渲染图片卡片，没装就退回文字版，绝不让插件停摆。
//! 所以不用 Plugin::inject（硬依赖会让插件在缺渲染器时停在 Pending），而是运行时探
//! list_services，渲染失败再逐级降级：
//!
//! 1. html2img 可用 → 渲染名片图（头像内嵌 data URI）→ 发照片；
//! 2. 有头像但没渲染器 → 发头像照片 + 文字版 caption；
//! 3. 都没有 → 发一条纯文字名片。
//!
//! 模板在 assets/card.html（Vue + 内联 CSS），只展示：头像、名称、@用户名 · ID、语言。

use std::path::{Path, PathBuf};

use fuyubot_plugin_api::{HostApi, LogLevel};
use serde_json::{Value, json};

use crate::host::UserInfo;

/// 提供方注册的服务名（契约定死，不是插件名）。
pub const SERVICE: &str = "fuyubot.html2img";

/// Vue 模板：编译进 dll，运行时不需要额外文件。
pub const TEMPLATE: &str = include_str!("../../assets/card.html");

/// 渲染出来的图片交给 TDLib 上传；超过这个尺寸就不要了。
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// 拼名片需要的现场参数。
#[derive(Debug, Clone, Default)]
pub struct CardContext {
    /// 头像 data URI；空串表示没有。
    pub avatar: String,
}

/// 名片数据：模板里直接用这些字段名。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CardData {
    /// 头像 data URI；空串 = 模板显示首字母占位。
    pub avatar: String,
    pub name: String,
    /// 名字首字符（模板不支持函数调用，首字母在这里算好）。
    pub initial: String,
    /// 带 @；没有用户名就是空串。
    pub username: String,
    pub user_id: String,
}

impl CardData {
    pub fn build(info: &UserInfo, context: CardContext) -> Self {
        let CardContext { avatar } = context;
        let name = if info.name.trim().is_empty() {
            "未知用户".to_owned()
        } else {
            info.name.trim().to_owned()
        };
        // 首字母只从**真实昵称**取；没有昵称就显示问号。
        let initial = info
            .name
            .trim()
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_owned());
        let username = info
            .username
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(|name| format!("@{name}"))
            .unwrap_or_default();

        Self {
            avatar,
            name,
            initial,
            username,
            user_id: info.user_id.to_string(),
        }
    }

    /// 模板 / 服务的 data 入参。
    pub fn to_json(&self) -> Value {
        json!({
            "avatar": self.avatar,
            "name": self.name,
            "initial": self.initial,
            "username": self.username,
            "user_id": self.user_id,
        })
    }

    /// 文字降级版：和图片卡片一样只保留这几项。
    pub fn fallback_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("{}\n", self.name));
        let mut ident = String::new();
        if !self.username.is_empty() {
            ident.push_str(&self.username);
        }
        if !self.user_id.is_empty() {
            if !ident.is_empty() {
                ident.push_str(" · ");
            }
            ident.push_str(&self.user_id);
        }
        if !ident.is_empty() {
            out.push_str(&format!("{ident}\n"));
        }
        out.trim_end().to_owned()
    }
}

/// 宿主上有没有 fuyubot.html2img（软依赖的运行时探测）。
pub fn available(host: &HostApi) -> bool {
    let raw = host.list_services();
    let Ok(services) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    services.as_array().is_some_and(|list| {
        list.iter()
            .any(|item| item.get("name").and_then(Value::as_str) == Some(SERVICE))
    })
}

/// 渲染一张名片图。失败返回 None，由调用方降级——这里从不向上抛错。
pub fn render_card(host: &HostApi, data: &CardData, width: u32) -> Option<PathBuf> {
    if !available(host) {
        host.log(
            LogLevel::Debug,
            format!("bidibot: 没看到 {SERVICE} 服务，名片走文字降级").as_str(),
        );
        return None;
    }
    let payload = json!({
        "html": TEMPLATE,
        "vue": true,
        "data": data.to_json(),
        "width": width.clamp(320, 1_200),
        "scale": 2.0,
        // 只要卡片本身：PNG + 透明底，外圈没有页面背景色。
        "format": "png",
        "auto_height": true,
        "transparent": true,
        "dark": false,
    });
    let reply = match host.call_service(SERVICE, "render", &payload.to_string()) {
        Ok(reply) => reply,
        Err(error) => {
            host.log(
                LogLevel::Warn,
                format!("bidibot: 调 {SERVICE} 渲染失败: {error}").as_str(),
            );
            return None;
        }
    };
    let value: Value = match serde_json::from_str(&reply) {
        Ok(value) => value,
        Err(error) => {
            host.log(
                LogLevel::Warn,
                format!("bidibot: {SERVICE} 应答不是 JSON: {error}").as_str(),
            );
            return None;
        }
    };
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        host.log(
            LogLevel::Warn,
            format!(
                "bidibot: {SERVICE} 渲染失败: {}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误")
            )
            .as_str(),
        );
        return None;
    }
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())?;
    let path = PathBuf::from(path);
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
        return None;
    }
    Some(path)
}

/// 把本地头像读成 data URI（模板里 img src 直接用）。
///
/// 读不到 / 太大 / 不是认识的图片格式都返回空串：模板会显示首字母占位，不影响出图。
pub fn avatar_data_uri(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return String::new();
    };
    if meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
        return String::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let Some(mime) = image_mime(&bytes) else {
        return String::new();
    };
    format!("data:{mime};base64,{}", encode_base64(&bytes))
}

/// 按魔数认图片类型；只认模板能显示的三种。
fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }
    if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// 标准 base64（带 = 填充）。自己写是为了不引第三方依赖，长度很小。
pub fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, username: Option<&str>) -> UserInfo {
        UserInfo {
            user_id: 123456789,
            name: name.to_owned(),
            username: username.map(str::to_owned),
            is_bot: false,
            photo_file_id: None,
        }
    }

    fn context() -> CardContext {
        CardContext {
            avatar: String::new(),
        }
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        let raw = [0u8, 0xFF, 0x10];
        assert_eq!(encode_base64(&raw), "AP8Q");
    }

    #[test]
    fn mime_is_detected_from_magic_bytes() {
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(
            image_mime(&[0x89, b'P', b'N', b'G', 0x0D]),
            Some("image/png")
        );
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBPVP8 ");
        assert_eq!(image_mime(&webp), Some("image/webp"));
        assert_eq!(image_mime(b"GIF89a"), None);
    }

    #[test]
    fn card_keeps_only_three_rows() {
        let data = CardData::build(&info("张三", Some("zhangsan")), context());
        assert_eq!(data.name, "张三");
        assert_eq!(data.initial, "张");
        assert_eq!(data.username, "@zhangsan");
        assert_eq!(data.user_id, "123456789");
    }

    #[test]
    fn empty_name_falls_back_to_placeholder() {
        let data = CardData::build(&info("", None), context());
        assert_eq!(data.name, "未知用户");
        assert_eq!(data.initial, "?", "占位不能拿『未知用户』的首字");
        assert!(data.username.is_empty());
    }

    #[test]
    fn text_fallback_matches_the_card_rows() {
        let data = CardData::build(&info("王五", Some("wangwu")), context());
        let text = data.fallback_text();
        assert_eq!(text, "王五\n@wangwu · 123456789");
        for unwanted in ["语言", "首次", "状态", "新对话", "译文"] {
            assert!(!text.contains(unwanted), "不要 {unwanted}");
        }
    }

    #[test]
    fn json_field_names_match_the_template_contract() {
        let data = CardData::build(
            &info("赵六", None),
            CardContext {
                avatar: "data:image/jpeg;base64,AAA".to_owned(),
            },
        );
        let json = data.to_json();
        for key in ["avatar", "name", "initial", "username", "user_id"] {
            assert!(json.get(key).is_some(), "模板契约缺字段 {key}");
        }
        assert!(json.get("lang_text").is_none(), "语言行已删除");
        assert!(json.get("status").is_none(), "模板不再有状态字段");
        assert_eq!(json["avatar"], "data:image/jpeg;base64,AAA");
    }
}
