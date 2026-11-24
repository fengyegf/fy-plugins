//! 工具目录：把宿主工具表与「注册到 agent 的能力」翻译成三种协议的工具定义，
//! 并同步执行模型发起的调用。
//!
//! 两条通道：
//!
//! - 工具（宿主工具表）：call_tool_result，给 AI / 宿主发现；
//! - 已注册能力（其它插件通过 call_service("fengyegf.agent", "register", 载荷) 注册）：
//!   模型点中时走 call_service(服务, "invoke", 载荷) 当场拿结果。
//!
//! ## 这个文件就是「消费方模板」
//!
//! 注册协议见 [crate::agent::registry]：提供方在自己的 start() 里调一次 register 即可，
//! agent 不需要去发现、也不需要逐个服务 describe；每轮重建目录时读注册表。
//!
//! 调用三步（本文件都实现了）：
//!
//! 1. register 记下 {name, description, input_schema, service, capability}；
//! 2. 同名能力若也在宿主工具表里，**优先走服务通道**（这正是
//!    「agent 调外部工具也走 provide_service」的落点）；
//! 3. 服务通道失败（服务不在 / 不认识能力）-> 退回 call_tool_result（同名工具还在）。
//!
//! 为什么每次都现查：宿主工具表随插件热插拔变化。缓存住名字等于让模型拿着已卸载的
//! 工具去调，必然 NoHandler；注册表是内存里的，读一次开销很小。
//!
//! 下发的是**全部能力**（可用 tool_denylist 屏蔽个别名字；另有 RESERVED_TOOLS 这类
//! 内置屏蔽永远不下发）：名字 / 描述 / 入参 schema 全部来自宿主注册的元数据或注册表，
//! 本模块只做协议翻译，不另外维护一份清单——否则两份清单迟早对不上。

use fuyubot_plugin_api::{HostApi, HostExt};
use serde_json::{Value, json};

use super::artifact::{ArtifactStore, envelope};
use super::completion::ProtocolKind;
use crate::agent::registry::Registry;
use crate::core::consts;

use std::sync::Arc;

/// 服务协议方法名（与提供方约定一致）。
const METHOD_INVOKE: &str = "invoke";

/// 一个可下发给模型的能力。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// 全名（宿主工具表里的键 / 注册表里的能力名，如 fengyegf.system.info）。
    pub name: String,
    /// 给模型看的说明。
    pub description: String,
    /// 入参 JSON Schema。
    pub input_schema: Value,
    /// 模型点了之后走哪条通道。
    channel: Channel,
}

/// 调用通道。
#[derive(Debug, Clone, PartialEq)]
enum Channel {
    /// 宿主工具表：call_tool_result。
    Tool,
    /// 已注册能力的服务通道：call_service(service, "invoke", …)。
    Service { service: String, capability: String },
}

/// 面向插件作者、而不是给模型用的通用大接口：fuyubot.td.functions 是原始 TDLib
/// 调用入口，入参 schema 覆盖几百个函数（体积按百 KB 计），模型看到就会反复试探，
/// 一两轮就能把上下文撑爆。它属于框架提供给「别的插件」的能力，不是对话能力，
/// 所以这里**内置屏蔽**：无论配置怎么写都不会下发给模型，tool_denylist 只能
/// 在此基础上继续追加。
pub const RESERVED_TOOLS: &[&str] = &["fuyubot.td.functions"];

/// 是否属于内置屏蔽（去空白、大小写不敏感，防止宿主注册时大小写漂移）。
pub fn is_reserved(name: &str) -> bool {
    let name = name.trim();
    RESERVED_TOOLS
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(name))
}

/// 工具目录 + 同步执行入口。
pub struct ToolCatalog {
    host: HostApi,
    specs: Vec<ToolSpec>,
    max_result_chars: usize,
    /// 长结果缓存（分页 / 字段过滤的数据源）。
    artifacts: Arc<ArtifactStore>,
    /// 读取工具是否在本次下发的清单里；被 denylist 屏蔽时退回普通截断。
    has_reader: bool,
    /// 走服务通道的能力数（describe / 诊断用）。
    service_count: usize,
    /// 服务通道失败时是否退回工具表。
    fallback_to_tool: bool,
}

impl ToolCatalog {
    /// 现查宿主工具表，并把注册表里的能力并进来。
    ///
    /// denylist 支持精确名与「前缀*」；max_result_chars 至少按 200 生效。
    pub fn load(
        host: &HostApi,
        denylist: &[String],
        max_result_chars: usize,
        use_registry: bool,
        fallback_to_tool: bool,
        registry: &Registry,
        artifacts: Arc<ArtifactStore>,
    ) -> Self {
        let mut specs = parse_specs(&host.list_tools(), denylist);
        if use_registry {
            merge_registrations(&mut specs, registry, denylist);
        }
        let has_reader = specs.iter().any(|spec| spec.name == super::RESULT_TOOL);
        let service_count = specs
            .iter()
            .filter(|spec| matches!(spec.channel, Channel::Service { .. }))
            .count();
        Self {
            host: *host,
            specs,
            max_result_chars: max_result_chars.max(200),
            artifacts,
            has_reader,
            service_count,
            fallback_to_tool,
        }
    }

    /// 能力数。
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

    /// 走服务通道（已注册能力）的数量。
    pub fn service_count(&self) -> usize {
        self.service_count
    }

    /// 协议原生的工具定义；目录为空返回 None（不下发空数组）。
    pub fn definitions(&self, protocol: ProtocolKind) -> Option<Value> {
        render_definitions(protocol, &self.specs)
    }

    /// 执行一次工具调用，返回**回填给模型**的文本。
    ///
    /// 失败也是文本（{"ok":false,"error":…}）：把错误交给模型，让它自己决定
    /// 换参数重试还是放弃，比整轮失败对用户友好。
    pub fn call(&self, name: &str, arguments: &str) -> String {
        // 双保险：模型凭记忆 / 幻觉直接点了内置屏蔽的工具时，也不真的调宿主。
        if is_reserved(name) {
            return error_text(&format!("工具 {name} 属于内置屏蔽的通用接口，不提供给模型"));
        }
        let Some(spec) = self.specs.iter().find(|spec| spec.name == name) else {
            return error_text(&format!("工具 {name} 不在本次下发的清单里，未执行"));
        };
        let input = match parse_arguments(name, arguments) {
            Ok(input) => input,
            Err(message) => return error_text(&message),
        };
        match &spec.channel {
            Channel::Service {
                service,
                capability,
            } => match self.invoke_service(service, capability, name, &input) {
                Ok(output) => self.finish(name, output),
                Err(reason) if !self.fallback_to_tool => {
                    error_text(&format!("服务 {service} 调用 {name} 失败：{reason}"))
                }
                Err(reason) => {
                    // 服务不在 / 不认识这个能力：退回工具表（同名工具通常还在）。
                    self.host.warn(&format!(
                        "agent: 服务 {service} 调用 {name} 失败（{reason}），退回工具表"
                    ));
                    match self.host.call_tool_result(name, &input.to_string()) {
                        Ok(output) => self.finish(name, output),
                        Err(error) => error_text(&format!("工具 {name} 调用失败：{error}")),
                    }
                }
            },
            Channel::Tool => match self.host.call_tool_result(name, &input.to_string()) {
                Ok(output) => self.finish(name, output),
                Err(error) => error_text(&format!("工具 {name} 调用失败：{error}")),
            },
        }
    }

    /// 收口一次工具结果：短结果原样回填；长结果存 artifact，回第一页 + 读取提示。
    ///
    /// 读取工具自己被 denylist 挡掉时（模型拿不到翻页工具），退回普通截断，
    /// 避免留下一个「有 id 但读不了」的 artifact。
    fn finish(&self, name: &str, output: String) -> String {
        if name == super::RESULT_TOOL {
            // 读取工具自己已经按页 / 值限长（逻辑内容 3500 字）；JSON 转义可能让最终
            // 字符串略超 4000，这里不再截断，否则会把合法 JSON 切成半截。
            return output;
        }
        let chars = output.chars().count();
        if chars <= self.max_result_chars {
            return output;
        }
        if self.has_reader {
            if let Some(id) = self.artifacts.put(&output) {
                self.host.info(&format!(
                    "agent: 工具 {name} 结果 {chars} 字符，已分页为 {id}"
                ));
                return envelope(name, &id, &output, consts::TOOL_RESULT_PAGE_CHARS);
            }
            self.host.warn(&format!(
                "agent: 工具 {name} 结果过大（{chars} 字符），未能缓存，退回截断"
            ));
        }
        truncate(&output, self.max_result_chars)
    }

    /// 走一次提供方的服务（消费方模板的核心 30 行）：
    /// call_service(service, "invoke", {"capability":…,"input":…}) -> data。
    fn invoke_service(
        &self,
        service: &str,
        capability: &str,
        name: &str,
        input: &Value,
    ) -> Result<String, String> {
        let request = json!({ "capability": capability, "input": input });
        let text = self
            .host
            .call_service(service, METHOD_INVOKE, &request.to_string())
            .map_err(|error| format!("call_service({service}, {METHOD_INVOKE}) 失败：{error}"))?;
        let reply: Value =
            serde_json::from_str(&text).map_err(|error| format!("服务回包不是 JSON：{error}"))?;
        if reply.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(reply.get("error").and_then(Value::as_str).map_or_else(
                || format!("服务 {service} 执行 {capability} 失败"),
                str::to_owned,
            ));
        }
        let _ = name;
        Ok(match reply.get("data") {
            Some(Value::String(text)) => text.clone(),
            Some(data) => data.to_string(),
            None => String::new(),
        })
    }
}

/// 把注册表里的能力并进目录。
///
/// 同名能力已经以工具形式存在时**升级成服务通道**（这正是「agent 调外部工具也走
/// provide_service」的落点）；注册表是内存快照，读一次很便宜。
fn merge_registrations(specs: &mut Vec<ToolSpec>, registry: &Registry, denylist: &[String]) {
    for registration in registry.snapshot() {
        if is_reserved(&registration.name) || blocked(&registration.name, denylist) {
            continue;
        }
        let channel = Channel::Service {
            service: registration.service.clone(),
            capability: registration.capability.clone(),
        };
        match specs.iter_mut().find(|spec| spec.name == registration.name) {
            Some(existing) => {
                if matches!(existing.channel, Channel::Tool) {
                    existing.channel = channel;
                }
            }
            None => specs.push(ToolSpec {
                name: registration.name.clone(),
                description: registration.description.clone(),
                input_schema: normalize_schema(Some(&registration.input_schema)),
                channel,
            }),
        }
    }
}

/// 解析 list_tools() 的 JSON 数组；坏数据一律当空表（不让诊断信息拖垮一轮对话）。
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
            channel: Channel::Tool,
        });
    }
    specs
}

/// 是否被 denylist 挡住：精确名，或以「*」结尾的前缀。
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

/// 入参 schema 兜底：非对象 / 空对象补成 {"type":"object","properties":{}}，
/// 否则 OpenAI 与 Anthropic 都会拒；带 oneOf 这类组合子的不硬塞 type。
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

/// 入参必须是 JSON 对象（工具都是 {...} 形状）。
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
