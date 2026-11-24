//! 宿主工具目录：把 `list_tools()` 的元数据翻译成三种协议的工具定义，
//! 并同步执行模型发起的调用（`call_tool_result`）。
//!
//! 为什么每次都现查：宿主工具表会随插件热插拔变化。缓存住名字等于让模型拿着
//! 已卸载的工具去调，必然 `NoHandler`。
//!
//! 下发的是**全部工具**（可用 `tool_denylist` 屏蔽个别名字；另有 [`RESERVED_TOOLS`]
//! 这类内置屏蔽永远不下发）：名字 / 描述 / 入参 schema 全部来自宿主注册时给的
//! `ToolDescriptor`，本模块只做协议翻译，不另外维护一份清单——否则两份清单迟早对不上。

use fuyubot_plugin_api::HostApi;
use serde_json::{Value, json};

use super::completion::ProtocolKind;

/// 一个可下发给模型的工具。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// 全名（宿主工具表里的键，如 `fengyegf.agent.chat`）。
    pub name: String,
    /// 给模型看的说明。
    pub description: String,
    /// 入参 JSON Schema。
    pub input_schema: Value,
}

/// 面向插件作者、而不是给模型用的通用大接口：`fuyubot.td.functions` 是原始 TDLib
/// 调用入口，入参 schema 覆盖几百个函数（体积按百 KB 计），模型看到就会反复试探，
/// 一两轮就能把上下文撑爆。它属于框架提供给「别的插件」的能力，不是对话能力，
/// 所以这里**内置屏蔽**：无论配置怎么写都不会下发给模型，`tool_denylist` 只能
/// 在此基础上继续追加。
pub const RESERVED_TOOLS: &[&str] = &["fuyubot.td.functions"];

/// 是否属于内置屏蔽（去空白、大小写不敏感，防止宿主注册时大小写漂移）。
pub fn is_reserved(name: &str) -> bool {
    let name = name.trim();
    RESERVED_TOOLS
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(name))
}

/// 宿主工具目录 + 同步执行入口。
pub struct ToolCatalog {
    host: HostApi,
    specs: Vec<ToolSpec>,
    max_result_chars: usize,
}

impl ToolCatalog {
    /// 现查宿主工具表。
    ///
    /// `denylist` 支持精确名与 `前缀*`；`max_result_chars` 至少按 200 生效。
    pub fn load(host: &HostApi, denylist: &[String], max_result_chars: usize) -> Self {
        Self {
            host: *host,
            specs: parse_specs(&host.list_tools(), denylist),
            max_result_chars: max_result_chars.max(200),
        }
    }

    /// 工具数。
    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// 是否为空（空 = 不下发工具）。
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// 本次下发的工具名（日志 / describe 用）。
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.specs.iter().map(|spec| spec.name.as_str())
    }

    /// 协议原生的工具定义；目录为空返回 `None`（不下发空数组）。
    pub fn definitions(&self, protocol: ProtocolKind) -> Option<Value> {
        render_definitions(protocol, &self.specs)
    }

    /// 执行一次工具调用，返回**回填给模型**的文本。
    ///
    /// 失败也是文本（`{"ok":false,"error":…}`）：把错误交给模型，让它自己决定
    /// 换参数重试还是放弃，比整轮失败对用户友好。
    pub fn call(&self, name: &str, arguments: &str) -> String {
        // 双保险：模型凭记忆 / 幻觉直接点了内置屏蔽的工具时，也不真的调宿主。
        if is_reserved(name) {
            return error_text(&format!("工具 {name} 属于内置屏蔽的通用接口，不提供给模型"));
        }
        if !self.specs.iter().any(|spec| spec.name == name) {
            return error_text(&format!("工具 {name} 不在本次下发的清单里，未执行"));
        }
        let input = match parse_arguments(name, arguments) {
            Ok(input) => input,
            Err(message) => return error_text(&message),
        };
        match self.host.call_tool_result(name, &input.to_string()) {
            Ok(output) => truncate(&output, self.max_result_chars),
            Err(error) => error_text(&format!("工具 {name} 调用失败：{error}")),
        }
    }
}

/// 解析 `list_tools()` 的 JSON 数组；坏数据一律当空表（不让诊断信息拖垮一轮对话）。
fn parse_specs(raw: &str, denylist: &[String]) -> Vec<ToolSpec> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    let mut specs = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        if is_reserved(name)
            || blocked(name, denylist)
            || specs.iter().any(|spec: &ToolSpec| spec.name == name)
        {
            continue;
        }
        specs.push(ToolSpec {
            name: name.to_owned(),
            description: item
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned(),
            input_schema: normalize_schema(
                item.get("input_schema")
                    .or_else(|| item.get("parameters"))
                    .or_else(|| item.get("schema")),
            ),
        });
    }
    specs
}

/// 是否被 denylist 挡住：精确名，或以 `*` 结尾的前缀。
fn blocked(name: &str, denylist: &[String]) -> bool {
    denylist.iter().any(|rule| {
        let rule = rule.trim();
        if rule.is_empty() {
            return false;
        }
        match rule.strip_suffix('*') {
            Some(prefix) => !prefix.is_empty() && name.starts_with(prefix),
            None => name == rule,
        }
    })
}

/// 入参 schema 兜底：非对象 / 空对象补成 `{"type":"object","properties":{}}`，
/// 否则 OpenAI 与 Anthropic 都会拒；带 `oneOf` 这类组合子的不硬塞 `type`。
fn normalize_schema(value: Option<&Value>) -> Value {
    let mut schema = match value {
        Some(Value::Object(_)) => value.cloned().unwrap_or_else(|| json!({})),
        _ => json!({}),
    };
    let combinator = ["oneOf", "anyOf", "allOf", "$ref"]
        .iter()
        .any(|key| schema.get(*key).is_some());
    if !combinator && schema.get("type").is_none() {
        schema["type"] = json!("object");
    }
    if !combinator && schema.get("properties").is_none() {
        schema["properties"] = json!({});
    }
    schema
}

/// 协议原生工具定义。
fn render_definitions(protocol: ProtocolKind, specs: &[ToolSpec]) -> Option<Value> {
    if specs.is_empty() {
        return None;
    }
    let tools: Vec<Value> = match protocol {
        ProtocolKind::OpenAiChat => specs
            .iter()
            .map(|spec| {
                json!({
                    "type": "function",
                    "function": {
                        "name": spec.name,
                        "description": spec.description,
                        "parameters": spec.input_schema,
                    }
                })
            })
            .collect(),
        ProtocolKind::OpenAiResponses => specs
            .iter()
            .map(|spec| {
                json!({
                    "type": "function",
                    "name": spec.name,
                    "description": spec.description,
                    "parameters": spec.input_schema,
                })
            })
            .collect(),
        ProtocolKind::AnthropicMessages => specs
            .iter()
            .map(|spec| {
                json!({
                    "name": spec.name,
                    "description": spec.description,
                    "input_schema": spec.input_schema,
                })
            })
            .collect(),
    };
    Some(Value::Array(tools))
}

/// 入参必须是 JSON 对象（工具都是 `{...}` 形状）。
fn parse_arguments(name: &str, arguments: &str) -> Result<Value, String> {
    let text = arguments.trim();
    if text.is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("工具 {name} 的入参不是 JSON：{error}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(format!("工具 {name} 的入参必须是 JSON 对象"))
    }
}

/// 统一失败文本（模型看到的是内容，不是 HTTP 错误）。
fn error_text(message: &str) -> String {
    json!({ "ok": false, "error": message }).to_string()
}

/// 按字符截断，截断时给模型留一句说明。
fn truncate(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push_str("…（结果已截断）");
    }
    out
}
