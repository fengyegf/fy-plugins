//! OpenAI Chat Completions 的请求 / 响应 / 流式 chunk 类型。
//!
//! 字段以官方 API 参考（POST /chat/completions）为准；没有建模的字段会落到每个类型的
//! extra 字段，序列化时原样写回，避免网关新增字段被丢弃。
//!
//! 约定：
//! - 请求可选字段统一 Option<T>，None 时跳过序列化（布尔字段也是 Option<bool>，
//!   避免把默认 false 发出去）；
//! - 不启用 deny_unknown_fields，响应出现未知字段不会失败；
//! - 多态结构（stop / tool_choice / function_call / 消息内容）用 #[serde(untagged)]
//!   枚举 + Other(Value) 兜底，未知形状原样往返；
//! - 响应中显式为 null 的可选字段反序列化为 None，重新序列化时会省略该键
//!   （请求方向不受影响）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 未建模字段的兜底容器（#[serde(flatten)]）。
pub type ExtraMap = Map<String, Value>;

/// 角色：系统提示。
pub const ROLE_SYSTEM: &str = "system";
/// 角色：开发者提示（o1 / 新模型推荐用 developer 替代 system）。
pub const ROLE_DEVELOPER: &str = "developer";
/// 角色：终端用户。
pub const ROLE_USER: &str = "user";
/// 角色：模型（assistant）。
pub const ROLE_ASSISTANT: &str = "assistant";
/// 角色：工具结果。
pub const ROLE_TOOL: &str = "tool";
/// 角色：旧版 function 结果。
pub const ROLE_FUNCTION: &str = "function";

// ---------------------------------------------------------------------------
// 请求
// ---------------------------------------------------------------------------

/// POST /chat/completions 的请求体。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// 模型 ID，例如 gpt-4o-mini。
    pub model: String,
    /// 对话消息列表（至少一条）。
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// 音频输出配置（modalities 含 audio 时使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioConfig>,
    /// 频率惩罚，-2.0 ~ 2.0。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    /// 旧版函数调用策略（none / auto / {"name": ...}）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCallChoice>,
    /// 旧版函数定义列表。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub functions: Option<Vec<FunctionDefinition>>,
    /// token id 到偏置值（-100 ~ 100）的映射。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<BTreeMap<String, i64>>,
    /// 是否返回输出 token 的 log 概率。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    /// 生成 token 上限（包含推理 token；替代 max_tokens）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    /// 生成 token 上限（旧字段，已弃用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 附加到响应对象的键值对（最多 16 组）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// 输出模态，例如 ["text"] / ["text", "audio"]。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<String>>,
    /// 生成多少个候选。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// 是否允许并行工具调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// 预测输出（用于降低延迟）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prediction: Option<Prediction>,
    /// 存在惩罚，-2.0 ~ 2.0。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    /// 推理强度：minimal / low / medium / high。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// 输出格式（文本 / JSON 对象 / JSON Schema）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// 采样种子（尽力而为的可复现性）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// 服务层级：auto / default / flex / priority 等。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 停止序列：单个字符串或字符串数组。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopSequence>,
    /// 是否存储本次补全（用于后续查询 / 蒸馏）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    /// 是否流式返回；由 streaming() 或客户端 create_stream 设置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 流式选项（是否输出 usage 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    /// 采样温度，0.0 ~ 2.0。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// 工具选择策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// 可用工具列表。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    /// 每个 token 位置返回多少个候选（0 ~ 20）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u32>,
    /// 核采样阈值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// 终端用户标识（滥用监控用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// 输出详略程度：low / medium / high。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
    /// 内置 Web 搜索工具配置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search_options: Option<WebSearchOptions>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl ChatCompletionRequest {
    /// 用模型 ID 构造空请求（需要自己补 messages）。
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    /// 补上消息后返回自身（链式构造）。
    pub fn with_messages(mut self, messages: Vec<ChatMessage>) -> Self {
        self.messages = messages;
        self
    }

    /// 返回 stream = true 的副本。
    pub fn streaming(mut self) -> Self {
        self.stream = Some(true);
        self
    }

    /// 是否启用了流式。
    pub fn is_streaming(&self) -> bool {
        self.stream == Some(true)
    }
}

/// 音频输出配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioConfig {
    /// 音色 ID，例如 alloy。
    pub voice: String,
    /// 音频格式，例如 wav / mp3。
    pub format: String,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 停止序列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopSequence {
    /// 单个停止字符串。
    Single(String),
    /// 多个停止字符串。
    Multiple(Vec<String>),
    /// 未识别的形状原样保留。
    Other(Value),
}

impl From<String> for StopSequence {
    fn from(value: String) -> Self {
        Self::Single(value)
    }
}

impl From<&str> for StopSequence {
    fn from(value: &str) -> Self {
        Self::Single(value.to_owned())
    }
}

impl From<Vec<String>> for StopSequence {
    fn from(value: Vec<String>) -> Self {
        Self::Multiple(value)
    }
}

/// 流式选项。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamOptions {
    /// 结束时额外下发一条只含 usage 的 chunk。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
    /// 是否启用流混淆（默认由服务端决定）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_obfuscation: Option<bool>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 输出格式。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseFormat {
    /// text / json_object / json_schema。
    #[serde(rename = "type")]
    pub format_type: String,
    /// type = json_schema 时的 Schema 定义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<JsonSchemaDefinition>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// JSON Schema 结构化输出定义。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JsonSchemaDefinition {
    /// Schema 名称。
    pub name: String,
    /// Schema 描述。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema 本体。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// 是否严格遵循 Schema。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 预测输出配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// 固定为 content。
    #[serde(rename = "type")]
    pub prediction_type: String,
    /// 预测的文本或内容片段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<PredictionContent>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 预测内容：纯文本或内容片段数组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PredictionContent {
    /// 纯文本。
    Text(String),
    /// 内容片段数组。
    Parts(Vec<ContentPart>),
    /// 未识别的形状原样保留。
    Other(Value),
}

/// 工具选择策略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// none / auto / required。
    Mode(String),
    /// {"type": "function", "function": {"name": ...}}。
    Function(NamedToolChoice),
    /// 旧版形状 {"name": ...}。
    Named(FunctionName),
    /// 未识别的形状（例如未来的 allowed_tools）原样保留。
    Other(Value),
}

/// 指定函数的工具选择。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NamedToolChoice {
    /// 固定为 function。
    #[serde(rename = "type")]
    pub choice_type: String,
    /// 目标函数。
    pub function: FunctionName,
}

/// 函数名引用。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionName {
    /// 函数名。
    pub name: String,
}

/// 旧版函数调用策略（function_call 字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FunctionCallChoice {
    /// none / auto。
    Mode(String),
    /// {"name": ...}。
    Named(FunctionName),
    /// 未识别的形状原样保留。
    Other(Value),
}

/// 工具定义。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// 工具类型，常见为 function。
    #[serde(rename = "type")]
    pub tool_type: String,
    /// type = function 时的函数定义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionDefinition>,
    /// 未建模字段的兜底（例如 type = custom 的 custom 对象）。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl Tool {
    /// 构造一个 function 工具。
    pub fn function(function: FunctionDefinition) -> Self {
        Self {
            tool_type: "function".to_owned(),
            function: Some(function),
            extra: ExtraMap::new(),
        }
    }
}

/// 函数定义（工具声明）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// 函数名。
    pub name: String,
    /// 函数说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema 参数定义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    /// 是否严格遵守 Schema。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// Web 搜索工具配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebSearchOptions {
    /// 搜索上下文规模：low / medium / high。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_context_size: Option<String>,
    /// 用户位置（用于本地化搜索结果）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_location: Option<WebSearchUserLocation>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// Web 搜索的用户位置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebSearchUserLocation {
    /// 固定为 approximate。
    #[serde(rename = "type")]
    pub location_type: String,
    /// 近似位置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approximate: Option<ApproximateLocation>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 近似位置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ApproximateLocation {
    /// 城市。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// 国家/地区。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// 省份/州。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// 时区。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

// ---------------------------------------------------------------------------
// 消息与内容片段
// ---------------------------------------------------------------------------

/// 一条对话消息。
///
/// 用一个结构体覆盖 system / developer / user / assistant / tool / function 六种角色：
/// 不同角色只用到其中一部分字段，未用到的字段为 None。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 角色，见 ROLE_SYSTEM 等常量。
    pub role: String,
    /// 消息内容：纯文本或内容片段数组。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// 参与者名称（可选）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// assistant 的工具调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// tool 消息对应的调用 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 旧版函数调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
    /// assistant 的拒答文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// assistant 的音频输出信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<MessageAudio>,
    /// 推理内容（DeepSeek / 部分兼容网关的非标准字段）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// 未建模字段的兜底（响应里的 annotations 等）。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl ChatMessage {
    /// 构造指定角色的文本消息。
    pub fn new(role: impl Into<String>, content: impl Into<MessageContent>) -> Self {
        Self {
            role: role.into(),
            content: Some(content.into()),
            ..Self::default()
        }
    }

    /// system 文本消息。
    pub fn system(content: impl Into<MessageContent>) -> Self {
        Self::new(ROLE_SYSTEM, content)
    }

    /// developer 文本消息。
    pub fn developer(content: impl Into<MessageContent>) -> Self {
        Self::new(ROLE_DEVELOPER, content)
    }

    /// user 文本消息。
    pub fn user(content: impl Into<MessageContent>) -> Self {
        Self::new(ROLE_USER, content)
    }

    /// assistant 文本消息。
    pub fn assistant(content: impl Into<MessageContent>) -> Self {
        Self::new(ROLE_ASSISTANT, content)
    }

    /// tool 结果消息。
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<MessageContent>) -> Self {
        Self {
            role: ROLE_TOOL.to_owned(),
            content: Some(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            ..Self::default()
        }
    }

    /// 取纯文本内容（内容片段数组时只取第一个 text 片段）。
    pub fn text(&self) -> Option<&str> {
        match self.content.as_ref()? {
            MessageContent::Text(text) => Some(text.as_str()),
            MessageContent::Parts(parts) => parts.iter().find_map(|part| part.text.as_deref()),
        }
    }
}

/// 消息内容：纯文本或内容片段数组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// 纯文本。
    Text(String),
    /// 内容片段数组。
    Parts(Vec<ContentPart>),
}

impl From<String> for MessageContent {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for MessageContent {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<Vec<ContentPart>> for MessageContent {
    fn from(value: Vec<ContentPart>) -> Self {
        Self::Parts(value)
    }
}

impl MessageContent {
    /// 取纯文本；内容片段数组时返回第一个 text 片段。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text.as_str()),
            Self::Parts(parts) => parts.iter().find_map(|part| part.text.as_deref()),
        }
    }
}

/// 一个消息内容片段。
///
/// 用「结构体 + type 字段 + 兜底 extra」而不是带 tag 的 enum，这样
/// text / image_url / input_audio / file 之外的未来片段类型也能原样往返。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContentPart {
    /// 片段类型：text / image_url / input_audio / file / refusal。
    #[serde(rename = "type")]
    pub part_type: String,
    /// type = text 时的文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// type = image_url 时的图片地址。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<ImageUrl>,
    /// type = input_audio 时的音频输入。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio: Option<InputAudio>,
    /// type = file 时的文件输入。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FilePart>,
    /// type = refusal 时的拒答文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl ContentPart {
    /// 文本片段。
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            part_type: "text".to_owned(),
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// 图片片段。
    pub fn image_url(image_url: ImageUrl) -> Self {
        Self {
            part_type: "image_url".to_owned(),
            image_url: Some(image_url),
            ..Self::default()
        }
    }

    /// 音频输入片段。
    pub fn input_audio(input_audio: InputAudio) -> Self {
        Self {
            part_type: "input_audio".to_owned(),
            input_audio: Some(input_audio),
            ..Self::default()
        }
    }

    /// 文件输入片段。
    pub fn file(file: FilePart) -> Self {
        Self {
            part_type: "file".to_owned(),
            file: Some(file),
            ..Self::default()
        }
    }

    /// 拒答片段。
    pub fn refusal(refusal: impl Into<String>) -> Self {
        Self {
            part_type: "refusal".to_owned(),
            refusal: Some(refusal.into()),
            ..Self::default()
        }
    }
}

/// 图片地址片段。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    /// 图片 URL 或 data: base64。
    pub url: String,
    /// 细节级别：auto / low / high。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl ImageUrl {
    /// 构造图片地址。
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    /// 设置细节级别。
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// 音频输入片段。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InputAudio {
    /// base64 编码的音频数据。
    pub data: String,
    /// 音频格式：wav / mp3。
    pub format: String,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl InputAudio {
    /// 构造音频输入。
    pub fn new(data: impl Into<String>, format: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            format: format.into(),
            extra: ExtraMap::new(),
        }
    }
}

/// 文件输入片段。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FilePart {
    /// 文件名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// base64 文件内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_data: Option<String>,
    /// 已上传文件的 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl FilePart {
    /// 用 base64 数据构造。
    pub fn data(filename: impl Into<String>, file_data: impl Into<String>) -> Self {
        Self {
            filename: Some(filename.into()),
            file_data: Some(file_data.into()),
            file_id: None,
            extra: ExtraMap::new(),
        }
    }

    /// 用文件 ID 构造。
    pub fn id(file_id: impl Into<String>) -> Self {
        Self {
            file_id: Some(file_id.into()),
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// 工具调用
// ---------------------------------------------------------------------------

/// 一次工具调用。
///
/// 同时用于完整响应（id / type 必填）与流式增量（index 标识、id 只在首块出现）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// 流式增量里的序号（响应里通常没有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// 调用 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 调用类型，通常为 function。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    /// 函数名与参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCall>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 函数调用（名称 + JSON 字符串参数）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// 函数名（流式增量里只有首块有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 参数 JSON 字符串（流式增量里是分片拼接）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// assistant 音频输出信息。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageAudio {
    /// 音频 ID。
    pub id: String,
    /// base64 音频数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// 过期时间戳。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// 音频转写文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

// ---------------------------------------------------------------------------
// 响应
// ---------------------------------------------------------------------------

/// POST /chat/completions 的同步响应。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// 补全 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 对象类型，通常为 chat.completion。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// 创建时间（Unix 秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
    /// 实际使用的模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 候选列表。
    #[serde(default)]
    pub choices: Vec<ChatChoice>,
    /// token 用量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// 服务端配置指纹。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    /// 服务层级。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 请求里 metadata 的原样回显。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl ChatCompletionResponse {
    /// 取第一个候选的文本。
    pub fn text(&self) -> Option<&str> {
        self.choices.first().and_then(ChatChoice::text)
    }
}

/// 一个候选。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatChoice {
    /// 候选序号。
    #[serde(default)]
    pub index: u32,
    /// 模型消息。
    pub message: ChatMessage,
    /// 结束原因：stop / length / tool_calls / content_filter 等。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// token log 概率。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<ChatLogprobs>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

impl ChatChoice {
    /// 取该候选的文本。
    pub fn text(&self) -> Option<&str> {
        self.message.text()
    }
}

/// 一个候选的 log 概率信息。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatLogprobs {
    /// 内容 token 的 log 概率。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ChatTokenLogprob>>,
    /// 拒答 token 的 log 概率。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Vec<ChatTokenLogprob>>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 单个 token 的 log 概率。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatTokenLogprob {
    /// token 文本。
    pub token: String,
    /// log 概率。
    pub logprob: f64,
    /// token 的 UTF-8 字节（可能为负，表示非法 UTF-8 字节）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<i32>>,
    /// 该位置最可能的若干个 token。
    #[serde(default)]
    pub top_logprobs: Vec<TopLogprob>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 某个位置上的候选 token。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TopLogprob {
    /// token 文本。
    pub token: String,
    /// log 概率。
    pub logprob: f64,
    /// token 的 UTF-8 字节。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<i32>>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// token 用量。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入 token 数。
    #[serde(default)]
    pub prompt_tokens: u32,
    /// 输出 token 数。
    #[serde(default)]
    pub completion_tokens: u32,
    /// 总 token 数。
    #[serde(default)]
    pub total_tokens: u32,
    /// 输入明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    /// 输出明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 输入 token 明细。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptTokensDetails {
    /// 命中缓存的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    /// 音频 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
    /// 文本 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_tokens: Option<u32>,
    /// 图片 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tokens: Option<u32>,
    /// 写缓存的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u32>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 输出 token 明细。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompletionTokensDetails {
    /// 推理 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    /// 音频 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
    /// 预测被接受的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_prediction_tokens: Option<u32>,
    /// 预测被拒绝的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_prediction_tokens: Option<u32>,
    /// 文本 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_tokens: Option<u32>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

// ---------------------------------------------------------------------------
// 流式 chunk
// ---------------------------------------------------------------------------

/// SSE 流里的一条 data chunk。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    /// 补全 ID（同一流内不变）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 对象类型，通常为 chat.completion.chunk。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// 创建时间（Unix 秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
    /// 实际使用的模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 增量候选。
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// 用量（stream_options.include_usage 打开时，最后一条 chunk 才有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// 服务端配置指纹。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    /// 服务层级。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 一个增量候选。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// 候选序号。
    #[serde(default)]
    pub index: u32,
    /// 本块增量。
    #[serde(default)]
    pub delta: Delta,
    /// 结束原因（只在最后一块出现）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// token log 概率增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<ChatLogprobs>,
    /// 未建模字段的兜底。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}

/// 流式增量：角色、文本、拒答、推理内容、工具调用等。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// 角色（通常只在首块出现）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 文本增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 拒答增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// 推理内容增量（DeepSeek / 兼容网关）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// 工具调用增量（按 index 分片）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 旧版函数调用增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
    /// 音频输出增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<MessageAudio>,
    /// 未建模字段的兜底（例如流混淆的 obfuscation）。
    #[serde(flatten, default)]
    pub extra: ExtraMap,
}
