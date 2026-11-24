//! `fengyegf.agent.result`：读取过长工具结果的指定页或 JSON 字段。
//!
//! 长结果不会整段回填（见 [crate::agent::tools] 的信封），原文按 artifact id 存在
//! [crate::agent::ArtifactStore] 里；本工具让模型按页翻阅，或只取自己关心的字段。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::PluginState;
use crate::agent::{self, RESULT_TOOL};
use crate::core::{AgentError, AgentResult, consts};

use super::parse_args;

/// 工具名。
pub const NAME: &str = RESULT_TOOL;

/// 工具说明（注册到宿主工具表）。
pub const DESCRIPTION: &str = "读取过长工具结果的分页 / 字段：长结果只回填第一页并给 artifact id；用本工具翻页（page / offset / limit），或按 JSON 路径过滤（path / fields，如 data.items[*].name）。结果只在内存保留约 10 分钟。";

/// 入参 schema。
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "artifact": {
                "type": "string",
                "description": "长结果信封里的 artifact id，例如 r3f8a1c2d4e5f6070"
            },
            "page": {
                "type": "integer",
                "minimum": 1,
                "description": "第几页（1 起）；与 offset 二选一"
            },
            "offset": {
                "type": "integer",
                "minimum": 0,
                "description": "按字符偏移读取；给了它就以它为准，忽略 page"
            },
            "limit": {
                "type": "integer",
                "minimum": 100,
                "maximum": consts::ARTIFACT_READ_MAX_CHARS,
                "description": "本页最多返回多少字符"
            },
            "path": {
                "type": "string",
                "description": "JSON 路径过滤，例如 data.items[*].name / items[0].title"
            },
            "fields": {
                "type": "array",
                "items": { "type": "string" },
                "description": "多个 JSON 路径，一起返回 路径 -> 值列表"
            },
            "item_offset": {
                "type": "integer",
                "minimum": 0,
                "description": "过滤时从第几个值开始（默认 0）"
            },
            "item_limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": consts::ARTIFACT_VALUE_MAX_LIMIT,
                "description": "过滤时最多返回多少个值（默认 200）"
            }
        },
        "required": ["artifact"],
        "additionalProperties": false
    })
}

/// 入参。
#[derive(Debug, Clone, Default, Deserialize)]
struct ResultArgs {
    #[serde(default)]
    artifact: Option<String>,
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    item_offset: Option<usize>,
    #[serde(default)]
    item_limit: Option<usize>,
}

impl ResultArgs {
    /// 必填的 artifact id（去空白后不能为空）。
    fn artifact(&self) -> AgentResult<&str> {
        self.artifact
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AgentError::BadRequest("缺少 artifact（长结果信封里的 id）".to_owned()))
    }
}

/// 处理：有 path / fields 就走字段过滤，否则按页翻阅。
pub fn handle(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let args: ResultArgs = parse_args(input)?;
    let id = args.artifact()?;
    let text = state.artifacts.get(id).ok_or_else(|| {
        AgentError::BadRequest(format!(
            "artifact {id} 不存在或已过期（长结果只在内存保留约 {} 分钟）",
            consts::ARTIFACT_TTL_SECS / 60
        ))
    })?;

    let paths: Vec<String> = if args.fields.is_empty() {
        args.path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(|path| vec![path.to_owned()])
            .unwrap_or_default()
    } else {
        args.fields
            .iter()
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty())
            .collect()
    };

    if paths.is_empty() {
        page_text(text.as_ref(), id, &args)
    } else {
        filter_json(text.as_ref(), id, &paths, &args)
    }
}

/// 按字符偏移 / 页读原文。
fn page_text(text: &str, id: &str, args: &ResultArgs) -> AgentResult<Value> {
    let total = text.chars().count();
    let limit = args
        .limit
        .unwrap_or(consts::ARTIFACT_READ_MAX_CHARS)
        .clamp(100, consts::ARTIFACT_READ_MAX_CHARS);
    let offset = match args.offset {
        Some(offset) => offset.min(total),
        None => {
            let page = args.page.unwrap_or(1).max(1) as usize;
            page.saturating_sub(1).saturating_mul(limit).min(total)
        }
    };
    let content: String = text.chars().skip(offset).take(limit).collect();
    let page = offset / limit + 1;
    let page_count = total.div_ceil(limit).max(1);
    let mut payload = json!({
        "ok": true,
        "artifact": id,
        "offset": offset,
        "page": page,
        "page_count": page_count,
        "total_chars": total,
        "content": content,
    });
    if content.is_empty() {
        payload["note"] = json!("offset 超出结果长度");
    }
    if let Ok(root) = serde_json::from_str::<Value>(text) {
        payload["json"] = json!(true);
        if offset == 0 {
            payload["shape"] = agent::json_shape(&root);
        }
    }
    Ok(payload)
}

/// 按 JSON 路径只取关心的字段：保持 JSON 里的顺序，按 item_offset / item_limit 分批。
fn filter_json(text: &str, id: &str, paths: &[String], args: &ResultArgs) -> AgentResult<Value> {
    let root: Value = serde_json::from_str(text).map_err(|_| {
        AgentError::BadRequest("该 artifact 不是 JSON，只能按 page / offset 翻阅".to_owned())
    })?;
    let mut selections: Vec<(String, Vec<&Value>)> = Vec::with_capacity(paths.len());
    for path in paths {
        let values = agent::select(&root, path).map_err(AgentError::BadRequest)?;
        selections.push((path.clone(), values));
    }
    let matched = selections
        .iter()
        .map(|(_, values)| values.len())
        .max()
        .unwrap_or(0);
    let item_limit = args
        .item_limit
        .unwrap_or(consts::ARTIFACT_VALUE_DEFAULT_LIMIT)
        .clamp(1, consts::ARTIFACT_VALUE_MAX_LIMIT);
    let start = args.item_offset.unwrap_or(0).min(matched);
    let mut returned = (matched - start).min(item_limit);

    loop {
        let response = if selections.len() == 1 {
            let (path, values) = &selections[0];
            let list: Vec<Value> = values
                .iter()
                .skip(start)
                .take(returned)
                .map(|value| agent::clip_value(value, consts::ARTIFACT_VALUE_MAX_CHARS))
                .collect();
            json!({
                "ok": true,
                "artifact": id,
                "path": path,
                "matched": matched,
                "start": start,
                "returned": returned,
                "values": list,
                "next_item_offset": next_offset(start, returned, matched),
            })
        } else {
            let mut map = serde_json::Map::new();
            for (path, values) in &selections {
                let list: Vec<Value> = values
                    .iter()
                    .skip(start)
                    .take(returned)
                    .map(|value| agent::clip_value(value, consts::ARTIFACT_VALUE_MAX_CHARS))
                    .collect();
                map.insert(path.clone(), Value::Array(list));
            }
            json!({
                "ok": true,
                "artifact": id,
                "matched": matched,
                "start": start,
                "returned": returned,
                "values": map,
                "next_item_offset": next_offset(start, returned, matched),
            })
        };
        if response.to_string().chars().count() <= consts::ARTIFACT_READ_MAX_CHARS || returned == 0
        {
            return Ok(response);
        }
        returned -= 1;
    }
}

/// 下一批的起点；没有下一批返回 null。
fn next_offset(start: usize, returned: usize, matched: usize) -> Value {
    if start + returned < matched {
        json!(start + returned)
    } else {
        Value::Null
    }
}
