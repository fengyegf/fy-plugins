//! OpenAI Responses 协议的类型定义。
//!
//! 覆盖 POST /responses 的请求体、响应对象、输入项 / 输出项、工具、用量与错误形状。
//! 所有结构体都带 #[serde(flatten)] extra 兜底：未建模字段不会被丢弃，可原样透传；
//! 不启用 deny_unknown_fields，上游新增字段不会导致解析失败。
//!
//! 字段参考 OpenAI 官方 Responses API 参考与 openai-python 的公开类型定义。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::core::{LlmError, LlmResult};

/// 未建模字段的兜底容器。
pub type Extra = Map<String, Value>;

// ===== 请求 =====

/// POST /responses 请求体。
///
/// 可选字段全部是 Option 并跳过序列化，未设置的字段不会出现在请求 JSON 里。
/// stream 一般不用手动设置：客户端的 create_stream / raw_stream 会自动置为 true。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseRequest {
    /// 模型 ID，例如 gpt-5、gpt-4.1。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 输入：一段纯文本，或输入项数组（message / item_reference / function_call_output 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<ResponseInput>,
    /// 系统 / 开发者指令。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// 最大输出 token 数（含推理 token）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// 内置工具调用的最大次数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u64>,
    /// 采样温度，0 到 2。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// 核采样概率质量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// 每个位置返回的 top logprob 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u32>,
    /// 可用工具列表（function / web_search_preview / file_search / code_interpreter / image_generation / mcp 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ResponseTool>>,
    /// 工具选择策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ResponseToolChoice>,
    /// 是否允许并行工具调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// 推理配置（effort / summary 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    /// 文本输出配置（结构化输出 / verbosity）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<ResponseTextConfig>,
    /// 超长输入的截断策略：auto / disabled。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<String>,
    /// 上一轮响应 ID，用于多轮对话。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// 会话标识：字符串 ID 或对象，原样透传。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Value>,
    /// 是否在服务端存储响应。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    /// 附加元数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// 需要额外返回的字段，例如 reasoning.encrypted_content。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    /// 是否后台执行。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// 是否流式返回；create_stream 会自动置为 true。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 流式选项（include_obfuscation）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    /// 终端用户标识（旧字段）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// 安全标识（替代 user）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_identifier: Option<String>,
    /// prompt cache 分桶键。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// 服务等级：auto / default / flex / priority 等。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 提示模板引用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<ResponsePrompt>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 请求输入：纯文本或输入项数组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    /// 直接给一段文本，服务端按 user 消息处理。
    Text(String),
    /// 结构化输入项数组。
    Items(Vec<ResponseInputItem>),
}

impl Default for ResponseInput {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

/// 单条输入项。
///
/// 用「结构体 + type 字段 + extra」而不是封闭 enum，这样任何带 type 的输入项
/// （message / item_reference / function_call / function_call_output / reasoning 等）
/// 都能解析、透传，不会因为新类型导致失败。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseInputItem {
    /// 输入项类型，例如 message、item_reference、function_call_output。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    /// 服务端返回的项 ID（item_reference 必填）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 消息角色：user / system / developer。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 消息内容块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ResponseInputContent>>,
    /// 项状态（服务端回传）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 工具调用 ID（function_call / function_call_output）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// 工具输出，可以是字符串或内容块数组。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// 函数调用参数（JSON 字符串）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// 函数名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 推理摘要。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Vec<ReasoningSummaryPart>>,
    /// 推理项的加密内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 输入内容块：input_text / input_image / input_file。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseInputContent {
    /// 内容类型。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// 文本内容（input_text）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 图片 URL 或 data URL（input_image）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// 文件 ID（input_image / input_file）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    /// 内联文件数据（base64）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_data: Option<String>,
    /// 文件 URL。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_url: Option<String>,
    /// 文件名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// 图片细节级别：auto / low / high。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 工具定义。
///
/// 覆盖 function / web_search_preview / file_search / computer_use_preview /
/// code_interpreter / image_generation / mcp 的常用字段；其余字段由 extra 透传。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseTool {
    /// 工具类型。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub tool_type: Option<String>,
    /// 函数名（function）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 函数描述。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 函数参数的 JSON Schema。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    /// 是否严格按 schema 生成参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// file_search 的向量库 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_store_ids: Option<Vec<String>>,
    /// file_search 过滤器。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<Value>,
    /// file_search 最大结果数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_num_results: Option<u64>,
    /// file_search 排序选项。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ranking_options: Option<Value>,
    /// web_search 上下文大小：low / medium / high。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_context_size: Option<String>,
    /// 搜索工具的用户位置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_location: Option<Value>,
    /// mcp 服务标签。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_label: Option<String>,
    /// mcp 服务地址。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// mcp 服务描述。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_description: Option<String>,
    /// mcp 允许的工具：字符串数组或过滤对象。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Value>,
    /// mcp 审批策略：always / never 或过滤对象。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_approval: Option<Value>,
    /// mcp 自定义请求头。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Map<String, Value>>,
    /// mcp 连接器 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<String>,
    /// mcp OAuth 令牌。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization: Option<String>,
    /// code_interpreter 容器：ID 或对象。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<Value>,
    /// computer_use_preview 显示宽度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_width: Option<u64>,
    /// computer_use_preview 显示高度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_height: Option<u64>,
    /// computer_use_preview 环境：browser / mac / windows / ubuntu。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// image_generation 尺寸。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// image_generation 质量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// image_generation 背景。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// image_generation 输出格式。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    /// image_generation 内容审核级别。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moderation: Option<String>,
    /// image_generation 输出压缩率。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_compression: Option<u64>,
    /// 是否延迟加载（tool search）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
    /// 允许的调用上下文：direct / programmatic。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_callers: Option<Vec<String>>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 工具选择策略：字符串（none / auto / required）或对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseToolChoice {
    /// none / auto / required。
    Mode(String),
    /// 指定函数、MCP 服务或允许的工具集合。
    Object(ResponseToolChoiceObject),
}

/// 对象形态的工具选择。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseToolChoiceObject {
    /// 对象类型：function / mcp / allowed_tools 等。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub choice_type: Option<String>,
    /// 函数名（function）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// MCP 服务标签（mcp）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_label: Option<String>,
    /// allowed_tools 的选择模式：auto / required。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// allowed_tools 允许的工具定义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Value>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 推理配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReasoningConfig {
    /// 推理强度：minimal / low / medium / high 等。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// 推理摘要粒度：auto / concise / detailed。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 旧版摘要开关：auto / concise / detailed。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_summary: Option<String>,
    /// 推理模式：standard / pro。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 上下文策略：auto / current_turn / all_turns。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 文本输出配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseTextConfig {
    /// 输出格式（text / json_schema / json_object）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
    /// 输出详略：low / medium / high。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 结构化输出格式。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextFormat {
    /// 格式类型：text / json_schema / json_object。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub format_type: Option<String>,
    /// 格式名（json_schema）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 格式描述。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// 是否严格模式。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 提示模板引用。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponsePrompt {
    /// 模板 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 模板版本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// 模板变量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<Map<String, Value>>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 流式选项。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamOptions {
    /// 是否在流里插入混淆事件。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_obfuscation: Option<bool>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

// ===== 响应 =====

/// 响应对象（POST /responses、GET /responses/{id}、response.completed 事件里的 response）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseObject {
    /// 响应 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 对象类型，固定 response。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// 创建时间（Unix 秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<f64>,
    /// 完成时间（Unix 秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<f64>,
    /// 状态：completed / failed / in_progress / cancelled / queued / incomplete。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 失败错误。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
    /// 未完成原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete_details: Option<IncompleteDetails>,
    /// 系统 / 开发者指令（字符串或输入项数组）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// 元数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// 实际使用的模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 输出项数组。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Vec<ResponseOutputItem>>,
    /// 是否允许并行工具调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// 采样温度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// 工具选择策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ResponseToolChoice>,
    /// 工具列表。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ResponseTool>>,
    /// 核采样概率质量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// 最大输出 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// 内置工具调用上限。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u64>,
    /// 上一轮响应 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// 所属会话。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Conversation>,
    /// 是否后台执行。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// 截断策略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<String>,
    /// top logprob 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u32>,
    /// 推理配置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    /// 文本输出配置。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<ResponseTextConfig>,
    /// 服务等级。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 终端用户标识（旧字段）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// 安全标识。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_identifier: Option<String>,
    /// prompt cache 分桶键。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// token 用量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
    /// 便捷字段：部分兼容网关直接给出拼接后的输出文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_text: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

impl ResponseObject {
    /// 输出项切片；没有输出时返回空切片。
    pub fn output_items(&self) -> &[ResponseOutputItem] {
        self.output.as_deref().unwrap_or(&[])
    }

    /// 拼接所有 output_text 内容块（与官方 SDK 的 output_text 便捷属性一致）。
    pub fn output_text(&self) -> String {
        let mut text = String::new();
        for item in self.output_items() {
            text.push_str(&item.text());
        }
        text
    }

    /// 所有函数调用输出项。
    pub fn function_calls(&self) -> Vec<&ResponseOutputItem> {
        self.output_items()
            .iter()
            .filter(|item| item.item_type.as_deref() == Some("function_call"))
            .collect()
    }
}

/// 单个输出项。
///
/// 与输入项一样用「结构体 + type + extra」，覆盖 message / function_call / reasoning /
/// web_search_call / file_search_call / computer_call / image_generation_call /
/// code_interpreter_call / mcp_call / mcp_list_tools / mcp_approval_request 等。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseOutputItem {
    /// 输出项类型。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    /// 输出项 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 项状态：in_progress / completed / incomplete。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 消息角色，通常是 assistant。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 消息内容块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ResponseContentPart>>,
    /// 工具调用 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// 函数 / 工具名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 函数调用参数（JSON 字符串）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    /// 推理摘要。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Vec<ReasoningSummaryPart>>,
    /// 推理项加密内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
    /// 工具动作（computer_call / shell_call 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Value>,
    /// 工具结果（image_generation_call 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// 工具输出（function_call_output 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// MCP 服务标签。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_label: Option<String>,
    /// 搜索查询。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// 工具列表（mcp_list_tools）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    /// 工具输出集合。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Value>,
    /// 错误详情。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

impl ResponseOutputItem {
    /// 拼接本项所有 output_text 内容块。
    pub fn text(&self) -> String {
        let mut text = String::new();
        for part in self.content.as_deref().unwrap_or(&[]) {
            if part.part_type.as_deref() == Some("output_text") {
                if let Some(value) = &part.text {
                    text.push_str(value);
                }
            }
        }
        text
    }

    /// 把函数调用参数解析成 JSON；不是合法 JSON 时返回 LlmError::Json。
    pub fn arguments_json(&self) -> LlmResult<Value> {
        match &self.arguments {
            Some(arguments) if !arguments.trim().is_empty() => serde_json::from_str(arguments)
                .map_err(|error| LlmError::Json(format!("函数调用参数不是合法 JSON：{error}"))),
            _ => Ok(Value::Null),
        }
    }
}

/// 输出内容块：output_text / refusal / reasoning_text / summary_text。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseContentPart {
    /// 内容类型。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub part_type: Option<String>,
    /// 文本内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 拒答内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// 文本注解（url_citation / file_citation 等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<Value>>,
    /// logprob 明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Value>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 推理摘要片段（summary_text）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReasoningSummaryPart {
    /// 片段类型，通常是 summary_text。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub part_type: Option<String>,
    /// 摘要文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 响应错误。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseError {
    /// 错误码。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 错误信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 出错参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// 错误类型。
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 未完成原因。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IncompleteDetails {
    /// 原因：max_output_tokens / content_filter 等。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 响应所属会话。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    /// 会话 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// token 用量。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseUsage {
    /// 输入 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// 输入 token 明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens_details: Option<InputTokensDetails>,
    /// 输出 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// 输出 token 明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<OutputTokensDetails>,
    /// 总 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 输入 token 明细。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InputTokensDetails {
    /// 命中缓存的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    /// 写入缓存的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// 缓存写入的保留期明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens_details: Option<CacheWriteTokensDetails>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 缓存写入保留期明细。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheWriteTokensDetails {
    /// 12 小时保留的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_12h_tokens: Option<u64>,
    /// 30 分钟保留的 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_30m_tokens: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 输出 token 明细。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    /// 推理 token 数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// DELETE /responses/{id} 的返回。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseDeletion {
    /// 响应 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 对象类型，固定 response.deleted。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// 是否删除成功。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
    /// 未建模字段兜底。
    #[serde(flatten, default)]
    pub extra: Extra,
}
