//! fuyubot.html2img 服务客户端：把 HTML / Vue 模板交给宿主上的渲染插件出图。
//!
//! 契约（html2img 插件 README）：
//!
//! - 服务名 fuyubot.html2img，方法 render，入参 / 出参都是 JSON；
//! - 入参必填 html；vue 为 true 时按 Vue 模板编译，用 data 渲染；
//! - 可选 width / height / scale / format / auto_height / transparent / dark；
//! - 成功回 {"ok":true,"path":"…","format":"jpg","width":…,"height":…}，
//!   失败回 {"ok":false,"error":"中文原因"}；
//! - path 是本机文件，调用方要在 24 小时内用掉（发送 / 转存）。
//!
//! 这里只做「拼入参 → 调服务 → 解出 path」，不关心模板长什么样。

use std::path::PathBuf;

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

/// 提供方注册的服务名（契约名，不是插件名）。
pub const SERVICE: &str = "fuyubot.html2img";

/// 一次渲染请求。
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// 要渲染的 HTML；vue 为 true 时是 Vue 模板（不含 template 外壳）。
    pub html: &'a str,
    /// true 时把 html 当 Vue 模板编译，用 data 渲染。
    pub vue: bool,
    /// Vue 模板的数据；vue 为 true 且不传时按空对象。
    pub data: Option<&'a Value>,
    /// 视口宽度（CSS px）；输出宽度 = width × scale。
    pub width: u32,
    /// 视口高度；auto_height 时只作「内容不足这么高」的兜底。
    pub height: Option<u32>,
    /// 输出倍率，1.0 = CSS 像素 1:1。
    pub scale: f32,
    /// jpg / png（也认 jpeg）。
    pub format: &'a str,
    /// 高度按内容撑开。
    pub auto_height: bool,
    /// 透明底（只对 PNG 有意义）。
    pub transparent: bool,
    /// 深色底，同时影响 prefers-color-scheme。
    pub dark: bool,
}

impl<'a> Request<'a> {
    /// 拼服务入参。抽成纯函数：单测不必造 HostApi。
    pub fn payload(&self) -> Value {
        let mut payload = json!({
            "html": self.html,
            "width": self.width,
            "scale": self.scale,
            "format": self.format,
            "auto_height": self.auto_height,
            "transparent": self.transparent,
            "dark": self.dark,
        });
        if self.vue {
            payload["vue"] = json!(true);
            payload["data"] = self.data.cloned().unwrap_or_else(|| json!({}));
        }
        if let Some(height) = self.height {
            payload["height"] = json!(height);
        }
        payload
    }
}

/// 调用失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 服务不存在 / 方法不存在 / 入参带 NUL 等 ABI 层失败。
    #[error("调用 {SERVICE} 失败：{0}")]
    Host(String),
    /// 服务回了不是 JSON 的文本。
    #[error("{SERVICE} 返回的不是 JSON：{0}")]
    Json(String),
    /// 服务正常应答但渲染失败（ok:false）。
    #[error("{SERVICE} 渲染失败：{0}")]
    Render(String),
    /// 服务说成功却没给 path。
    #[error("{SERVICE} 成功但没有返回图片路径")]
    MissingPath,
}

/// 调 fuyubot.html2img 渲染，成功时返回成品在本机的路径。
///
/// 这是**阻塞**调用：渲染在调用方线程上跑完才返回，只能在自己的业务线程里调，
/// 不能在宿主回调里调。
pub fn render(host: &HostApi, request: &Request<'_>) -> Result<PathBuf, Error> {
    let input = request.payload().to_string();
    let reply = host
        .call_service(SERVICE, "render", &input)
        .map_err(|error| Error::Host(error.to_string()))?;
    parse_reply(&reply)
}

/// 解析服务的应答 JSON。抽成纯函数，单测直接喂字符串。
fn parse_reply(reply: &str) -> Result<PathBuf, Error> {
    let value: Value =
        serde_json::from_str(reply).map_err(|error| Error::Json(error.to_string()))?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(Error::Render(message.to_owned()));
    }
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .ok_or(Error::MissingPath)?;
    Ok(PathBuf::from(path))
}
