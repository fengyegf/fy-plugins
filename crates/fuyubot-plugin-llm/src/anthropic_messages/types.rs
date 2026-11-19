//! Anthropic Messages 协议类型定义。
//!
//! 覆盖 Messages API 的请求、响应、内容块、工具、思考、用量，以及 beta 新特性的透传兜底。
//!
//! 约定：
//! - 可选字段一律 `Option<T>` + `skip_serializing_if = "Option::is_none"`；
//! - 每个结构体带 `#[serde(flatten)] extra`，未建模字段原样透传；
//! - 多态内容块用「结构体 + type 字段 + extra」表示，未知块类型也能往返；
//! - 不使用 `deny_unknown_fields`，上游新增字段不会让反序列化失败。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 未建模字段兜底：反序列化时收集未知字段，序列化时原样写回。
pub type Extra = Map<String, Value>;

/// Prompt caching 控制（`cache_control`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheControl {
    /// 缓存类型，官方目前只有 `ephemeral`。
    #[serde(rename = "type")]
    pub cache_type: String,
    /// 可选存活时间（例如 `5m` / `1h`，取决于模型与账号）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl Default for CacheControl {
    fn default() -> Self {
        Self {
            cache_type: "ephemeral".to_owned(),
            ttl: None,
            extra: Extra::new(),
        }
    }
}

impl CacheControl {
    /// 默认的持久化缓存标记：`{"type": "ephemeral"}`。
    pub fn ephemeral() -> Self {
        Self::default()
    }

    /// 带 ttl 的持久化缓存标记。
    pub fn ephemeral_ttl(ttl: impl Into<String>) -> Self {
        Self {
            ttl: Some(ttl.into()),
            ..Self::default()
        }
    }
}

/// 内容块来源：`image` / `document` 的 base64、url、text，或服务端工具的纯字符串来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Source {
    /// 结构化来源（base64 / url / text）。
    Object(SourceObject),
    /// 纯字符串来源（例如 `search_result.source`）。
    Text(String),
}

impl Default for Source {
    fn default() -> Self {
        Self::Object(SourceObject::default())
    }
}

impl From<SourceObject> for Source {
    fn from(object: SourceObject) -> Self {
        Self::Object(object)
    }
}

impl From<String> for Source {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for Source {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

/// 结构化内容来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceObject {
    /// `base64` / `url` / `text`。
    #[serde(rename = "type")]
    pub source_type: String,
    /// base64 数据的媒体类型（`image/png`、`application/pdf` 等）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// base64 数据（不带 data URI 前缀）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// url 来源的地址。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl Default for SourceObject {
    fn default() -> Self {
        Self {
            source_type: "base64".to_owned(),
            media_type: None,
            data: None,
            url: None,
            extra: Extra::new(),
        }
    }
}

impl SourceObject {
    /// base64 来源。
    pub fn base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            source_type: "base64".to_owned(),
            media_type: Some(media_type.into()),
            data: Some(data.into()),
            ..Self::default()
        }
    }

    /// url 来源。
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            source_type: "url".to_owned(),
            url: Some(url.into()),
            ..Self::default()
        }
    }

    /// 纯文本来源（document 的 text 类型）。
    pub fn text(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            source_type: "text".to_owned(),
            media_type: Some(media_type.into()),
            data: Some(data.into()),
            ..Self::default()
        }
    }
}

/// 一条内容块（请求的 content block param 与响应的 content block 共用同一形状）。
///
/// 多态通过 `block_type` 区分：`text` / `image` / `document` / `tool_use` / `tool_result` /
/// `thinking` / `redacted_thinking` / `server_tool_use` / `web_search_tool_result` /
/// `search_result` / `mcp_tool_use` / `mcp_tool_result` / `container_upload` 等。
/// 未建模的字段（含未知块类型的全部字段）都在 `extra` 里，因此可以原样往返。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentBlock {
    /// 块类型。
    #[serde(rename = "type")]
    pub block_type: String,
    /// text / thinking 块的文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// image / document / search_result 的来源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// document / search_result 的标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// document 的上下文说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// 引用信息：响应是 citation 数组，请求 document 是 `{"enabled": true}`，故保留原始 JSON。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<Value>,
    /// tool_use / server_tool_use / mcp_tool_use 的 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 工具名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 工具输入（tool_use / server_tool_use / mcp_tool_use）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// MCP 工具所属服务器名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// tool_result / mcp_tool_result / web_search_tool_result 对应的 tool_use id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// tool_result / web_search_tool_result / search_result 的内容（string 或块数组）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// tool_result 是否表示错误。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// thinking 块的思考文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// thinking 块的签名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// redacted_thinking 的加密数据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// container_upload 的文件 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    /// 请求侧的 prompt caching 标记。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl ContentBlock {
    /// 文本块。
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            block_type: "text".to_owned(),
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// base64 图片块。
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            block_type: "image".to_owned(),
            source: Some(Source::Object(SourceObject::base64(media_type, data))),
            ..Self::default()
        }
    }

    /// url 图片块。
    pub fn image_url(url: impl Into<String>) -> Self {
        Self {
            block_type: "image".to_owned(),
            source: Some(Source::Object(SourceObject::url(url))),
            ..Self::default()
        }
    }

    /// 工具调用块（assistant 消息 / 响应内容）。
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: impl Into<Value>,
    ) -> Self {
        Self {
            block_type: "tool_use".to_owned(),
            id: Some(id.into()),
            name: Some(name.into()),
            input: Some(input.into()),
            ..Self::default()
        }
    }

    /// 工具结果块，content 可以是字符串或块数组的 JSON。
    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<Value>) -> Self {
        Self {
            block_type: "tool_result".to_owned(),
            tool_use_id: Some(tool_use_id.into()),
            content: Some(content.into()),
            ..Self::default()
        }
    }

    /// 工具结果块（块数组形式）。
    pub fn tool_result_blocks(tool_use_id: impl Into<String>, blocks: Vec<ContentBlock>) -> Self {
        let content = serde_json::to_value(blocks).unwrap_or(Value::Null);
        Self::tool_result(tool_use_id, content)
    }

    /// 扩展思考块。
    pub fn thinking(thinking: impl Into<String>, signature: impl Into<String>) -> Self {
        Self {
            block_type: "thinking".to_owned(),
            thinking: Some(thinking.into()),
            signature: Some(signature.into()),
            ..Self::default()
        }
    }

    /// 被加密的思考块。
    pub fn redacted_thinking(data: impl Into<String>) -> Self {
        Self {
            block_type: "redacted_thinking".to_owned(),
            data: Some(data.into()),
            ..Self::default()
        }
    }

    /// 是否是某个类型。
    pub fn is_type(&self, block_type: &str) -> bool {
        self.block_type == block_type
    }

    /// 文本块的内容。
    pub fn as_text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// 工具调用块的 (id, name, input)。
    pub fn as_tool_use(&self) -> Option<(&str, &str, &Value)> {
        if self.block_type != "tool_use" {
            return None;
        }
        Some((
            self.id.as_deref()?,
            self.name.as_deref()?,
            self.input.as_ref()?,
        ))
    }
}

/// 消息内容：纯字符串或内容块数组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// 纯字符串。
    Text(String),
    /// 内容块数组。
    Blocks(Vec<ContentBlock>),
    /// 兜底：未识别的形状原样保留。
    Raw(Value),
}

impl Default for MessageContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<&str> for MessageContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for MessageContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Vec<ContentBlock>> for MessageContent {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        Self::Blocks(blocks)
    }
}

impl From<Value> for MessageContent {
    fn from(value: Value) -> Self {
        Self::Raw(value)
    }
}

/// 一条输入消息。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// `user` 或 `assistant`。
    #[serde(default)]
    pub role: String,
    /// 消息内容。
    pub content: MessageContent,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl Message {
    /// 构造一条消息。
    pub fn new(role: impl Into<String>, content: impl Into<MessageContent>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            extra: Extra::new(),
        }
    }

    /// 用户消息。
    pub fn user(content: impl Into<MessageContent>) -> Self {
        Self::new("user", content)
    }

    /// 助手消息。
    pub fn assistant(content: impl Into<MessageContent>) -> Self {
        Self::new("assistant", content)
    }
}

/// 系统提示词：纯字符串或文本块数组（块数组可带 cache_control）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemPrompt {
    /// 纯字符串。
    Text(String),
    /// 文本块数组。
    Blocks(Vec<ContentBlock>),
    /// 兜底。
    Raw(Value),
}

impl Default for SystemPrompt {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<&str> for SystemPrompt {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for SystemPrompt {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Vec<ContentBlock>> for SystemPrompt {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        Self::Blocks(blocks)
    }
}

/// 请求元数据。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// 调用方用户 id（官方字段）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// 工具定义：自定义工具（name + input_schema）或内置工具（computer / bash / text_editor / web_search）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// 内置工具的类型标记，例如 `computer_20250124`、`bash_20250124`、`web_search_20250305`；
    /// 自定义工具可省略（官方默认 `custom`）。
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub tool_type: Option<String>,
    /// 工具名。
    #[serde(default)]
    pub name: String,
    /// 工具描述。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 自定义工具的 JSON Schema。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// 请求侧 prompt caching 标记。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
    /// 内置工具：web_search 的最大使用次数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// 内置工具：允许的域名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
    /// 内置工具：禁止的域名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_domains: Option<Vec<String>>,
    /// 内置工具：用户位置（web_search）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_location: Option<Value>,
    /// 内置工具：computer 的显示宽度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_width_px: Option<u32>,
    /// 内置工具：computer 的显示高度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_height_px: Option<u32>,
    /// 内置工具：computer 的显示编号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_number: Option<u32>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl Tool {
    /// 自定义工具。
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: impl Into<Value>,
    ) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            input_schema: Some(input_schema.into()),
            ..Self::default()
        }
    }

    /// 内置工具（computer / bash / text_editor / web_search …）。
    pub fn builtin(tool_type: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            tool_type: Some(tool_type.into()),
            name: name.into(),
            ..Self::default()
        }
    }
}

/// 工具选择策略。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolChoice {
    /// `auto` / `any` / `tool` / `none`。
    #[serde(rename = "type")]
    pub choice_type: String,
    /// `type = "tool"` 时强制调用的工具名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 是否禁止并行工具调用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_parallel_tool_use: Option<bool>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl ToolChoice {
    /// 交给模型决定。
    pub fn auto() -> Self {
        Self {
            choice_type: "auto".to_owned(),
            ..Self::default()
        }
    }

    /// 必须调用某个工具（不指定具体名字）。
    pub fn any() -> Self {
        Self {
            choice_type: "any".to_owned(),
            ..Self::default()
        }
    }

    /// 强制调用指定工具。
    pub fn tool(name: impl Into<String>) -> Self {
        Self {
            choice_type: "tool".to_owned(),
            name: Some(name.into()),
            ..Self::default()
        }
    }

    /// 禁止调用工具。
    pub fn none() -> Self {
        Self {
            choice_type: "none".to_owned(),
            ..Self::default()
        }
    }
}

/// 扩展思考配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Thinking {
    /// `enabled` / `disabled` / `adaptive`。
    #[serde(rename = "type")]
    pub thinking_type: String,
    /// `enabled` 时的思考预算（token）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    /// 是否展示思考内容（新模型支持 `summarized` / `omitted`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl Thinking {
    /// 开启扩展思考。
    pub fn enabled(budget_tokens: u32) -> Self {
        Self {
            thinking_type: "enabled".to_owned(),
            budget_tokens: Some(budget_tokens),
            ..Self::default()
        }
    }

    /// 关闭扩展思考。
    pub fn disabled() -> Self {
        Self {
            thinking_type: "disabled".to_owned(),
            ..Self::default()
        }
    }
}

/// `POST {base}/messages` 请求体。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageRequest {
    /// 模型 id。
    #[serde(default)]
    pub model: String,
    /// 最大输出 token（必填）。
    pub max_tokens: u32,
    /// 消息列表。
    #[serde(default)]
    pub messages: Vec<Message>,
    /// 系统提示词（string 或 block 数组）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,
    /// 元数据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// 停止序列。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// 是否流式；调用方设置，[`Client::create_stream`] 会强制为 `true`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 采样温度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// top-k 采样。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// nucleus 采样。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// 工具列表。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    /// 工具选择策略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// 扩展思考配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    /// 服务等级（`auto` / `standard_only`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 未建模字段（`container`、`mcp_servers`、`betas` 等新参数自动透传）。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl MessageRequest {
    /// 构造最小请求。
    pub fn new(model: impl Into<String>, max_tokens: u32, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            max_tokens,
            messages,
            ..Self::default()
        }
    }

    /// 设置系统提示词。
    pub fn with_system(mut self, system: impl Into<SystemPrompt>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// 设置采样温度。
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

/// 服务端工具用量。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerToolUsage {
    /// web_search 请求次数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search_requests: Option<u64>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

/// token 用量。
///
/// 流式 `message_delta` 里的 usage 只带部分字段（历史上只有 output_tokens），
/// 所以这里全部是 `Option`，便于增量合并。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入 token。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// 输出 token。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// 写入缓存的 token。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// 命中缓存的 token。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// 服务端工具用量。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUsage>,
    /// 服务等级。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl Usage {
    /// 输入 + 输出 token（两者都已知时）。
    pub fn total_tokens(&self) -> Option<u64> {
        match (self.input_tokens, self.output_tokens) {
            (Some(input), Some(output)) => Some(input.saturating_add(output)),
            _ => None,
        }
    }

    /// 用更新（更权威）的用量覆盖已知字段：只覆盖对方显式给出的字段。
    pub fn merge(&mut self, other: &Usage) {
        if other.input_tokens.is_some() {
            self.input_tokens = other.input_tokens;
        }
        if other.output_tokens.is_some() {
            self.output_tokens = other.output_tokens;
        }
        if other.cache_creation_input_tokens.is_some() {
            self.cache_creation_input_tokens = other.cache_creation_input_tokens;
        }
        if other.cache_read_input_tokens.is_some() {
            self.cache_read_input_tokens = other.cache_read_input_tokens;
        }
        if other.server_tool_use.is_some() {
            self.server_tool_use = other.server_tool_use.clone();
        }
        if other.service_tier.is_some() {
            self.service_tier = other.service_tier.clone();
        }
        for (key, value) in &other.extra {
            self.extra.insert(key.clone(), value.clone());
        }
    }
}

/// `POST {base}/messages` 响应体。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageResponse {
    /// 消息 id。
    #[serde(default)]
    pub id: String,
    /// 固定为 `message`。
    #[serde(rename = "type", default)]
    pub message_type: String,
    /// 固定为 `assistant`。
    #[serde(default)]
    pub role: String,
    /// 模型 id。
    #[serde(default)]
    pub model: String,
    /// 内容块。
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// 停止原因（`end_turn` / `max_tokens` / `stop_sequence` / `tool_use` / `pause_turn` / `refusal`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// 命中的停止序列。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// 结构化的停止细节（新版本 `stop_reason = "refusal"` 时给出）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<Value>,
    /// token 用量。
    #[serde(default)]
    pub usage: Usage,
    /// 未建模字段（`container` 等）。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl MessageResponse {
    /// 拼接所有 text 块的文本。
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter(|block| block.block_type == "text")
            .filter_map(|block| block.text.as_deref())
            .collect()
    }

    /// 所有 tool_use 块的 (id, name, input)。
    pub fn tool_uses(&self) -> Vec<(&str, &str, &Value)> {
        self.content
            .iter()
            .filter(|block| block.block_type == "tool_use")
            .filter_map(|block| {
                let id = block.id.as_deref()?;
                let name = block.name.as_deref()?;
                let input = block.input.as_ref()?;
                Some((id, name, input))
            })
            .collect()
    }

    /// 所有 thinking 块的思考文本。
    pub fn thinking(&self) -> Option<&str> {
        self.content
            .iter()
            .filter(|block| block.block_type == "thinking")
            .find_map(|block| block.thinking.as_deref())
    }
}

/// `POST {base}/messages/count_tokens` 请求体。
///
/// 不接受 `max_tokens` / `stream` 等生成参数，因此单独建模。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CountTokensRequest {
    /// 模型 id。
    #[serde(default)]
    pub model: String,
    /// 消息列表。
    #[serde(default)]
    pub messages: Vec<Message>,
    /// 系统提示词（string 或 block 数组）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,
    /// 工具列表（工具定义会计入 token）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    /// 工具选择策略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// 扩展思考配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    /// 元数据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// 未建模字段。
    #[serde(default, flatten)]
    pub extra: Extra,
}

impl CountTokensRequest {
    /// 构造最小请求。
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            ..Self::default()
        }
    }

    /// 从一次 `messages` 请求派生：复用 model / messages / system / tools / tool_choice / thinking / metadata。
    pub fn from_message_request(request: &MessageRequest) -> Self {
        Self {
            model: request.model.clone(),
            messages: request.messages.clone(),
            system: request.system.clone(),
            tools: request.tools.clone(),
            tool_choice: request.tool_choice.clone(),
            thinking: request.thinking.clone(),
            metadata: request.metadata.clone(),
            extra: request.extra.clone(),
        }
    }
}

/// `POST {base}/messages/count_tokens` 响应体。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CountTokensResponse {
    /// 输入 token 总数（消息 + system + tools）。
    #[serde(default)]
    pub input_tokens: u64,
    /// 未建模字段（beta 的 context_management 等）。
    #[serde(default, flatten)]
    pub extra: Extra,
}
