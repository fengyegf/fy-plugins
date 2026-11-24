//! 能力注册表：别的插件把自己「有什么能力」注册进 agent，agent 拿着这份表与它们交互。
//!
//! 为什么表在 agent 里：agent 是消费方，它需要一个「能调什么」的清单；提供方在
//! 自己的 start() 里调一次 call_service("fengyegf.agent", "register", 载荷) 就完成注册。
//! 不需要额外插件，也不需要中心总线——就是文档「插件联动」的命名服务调用。
//!
//! ## 注册协议（服务 fengyegf.agent 的 register 方法入参）
//!
//! {
//!   "name": "fengyegf.system.info",        // 能力名（建议与工具同名）
//!   "description": "读取宿主机设备信息",
//!   "input_schema": {"type":"object","properties":{}},
//!   "output_schema": {},
//!   "timeout_ms": 30000,
//!   "idempotent": true,
//!   "service": "fengyegf.system",          // 提供方服务名
//!   "capability": "fengyegf.system.info"   // 该服务 invoke 里的能力名
//! }
//!
//! agent 调它时永远走 call_service(service, "invoke", 载荷)。注册是**热**的：
//! agent 每一轮重建工具目录时都读这份表，所以提供方后启动也没关系，起来时注册一次即可。

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{Value, json};

/// 注册表条目上限：防止某个插件无限灌条目把上下文撑爆。
const MAX_ENTRIES: usize = 256;

/// 一条注册项。
#[derive(Debug, Clone, PartialEq)]
pub struct Registration {
    /// 能力名（模型看到的工具名，建议与提供方的工具同名）。
    pub name: String,
    /// 给模型看的说明。
    pub description: String,
    /// 入参 JSON Schema。
    pub input_schema: Value,
    /// 出参 JSON Schema（不校验就空对象）。
    pub output_schema: Value,
    /// 建议超时（毫秒）。
    pub timeout_ms: u64,
    /// 幂等标记。
    pub idempotent: bool,
    /// 提供方服务名（call_service 的第一个参数）。
    pub service: String,
    /// 该服务 invoke 里的能力名。
    pub capability: String,
    /// 谁注册的（宿主按调用方判定，agent 只记录）。
    pub owner: Option<String>,
}

impl Registration {
    /// 从 register 入参解析并做范围校验。
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let name = text_field(value, "name", 160)
            .ok_or_else(|| "register 需要 name（能力名）".to_owned())?;
        let service = text_field(value, "service", 160)
            .ok_or_else(|| "register 需要 service（提供方服务名）".to_owned())?;
        let capability = text_field(value, "capability", 160).unwrap_or_else(|| name.clone());
        Ok(Self {
            name,
            description: text_field(value, "description", 2_000).unwrap_or_default(),
            input_schema: match value.get("input_schema") {
                Some(Value::Object(_)) => value
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                _ => json!({ "type": "object", "properties": {} }),
            },
            output_schema: value.get("output_schema").cloned().unwrap_or(Value::Null),
            timeout_ms: value
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(30_000)
                .clamp(1_000, 600_000),
            idempotent: value
                .get("idempotent")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            service,
            capability,
            owner: None,
        })
    }

    /// 补上注册者（host.current_plugin()）。
    pub fn with_owner(mut self, owner: Option<String>) -> Self {
        self.owner = owner;
        self
    }

    /// 回给 list / 诊断的 JSON。
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
            "output_schema": self.output_schema,
            "timeout_ms": self.timeout_ms,
            "idempotent": self.idempotent,
            "service": self.service,
            "capability": self.capability,
            "owner": self.owner,
        })
    }
}

/// 注册表：一个 Mutex + BTreeMap，读写都是短临界区。
#[derive(Default)]
pub struct Registry {
    entries: Mutex<BTreeMap<String, Registration>>,
}

impl Registry {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册 / 覆盖一条。同名覆盖（后写生效），超过上限且是新名字时拒绝。
    pub fn register(&self, registration: Registration) -> Result<(), String> {
        let mut entries = self.lock();
        if !entries.contains_key(&registration.name) && entries.len() >= MAX_ENTRIES {
            return Err(format!("注册表已满（{MAX_ENTRIES} 条），先注销再注册"));
        }
        entries.insert(registration.name.clone(), registration);
        Ok(())
    }

    /// 注销：只有登记它的插件（或 agent 自己）能注销；不在表里返回 Ok(false)。
    pub fn unregister(
        &self,
        name: &str,
        caller: Option<&str>,
        self_id: &str,
    ) -> Result<bool, String> {
        let mut entries = self.lock();
        let Some(existing) = entries.get(name) else {
            return Ok(false);
        };
        let allowed = existing.owner.is_none()
            || caller == existing.owner.as_deref()
            || caller == Some(self_id);
        if !allowed {
            return Err(format!(
                "能力 {name} 由 {:?} 注册，只有它能注销",
                existing.owner
            ));
        }
        entries.remove(name);
        Ok(true)
    }

    /// 快照（按名字排序）：工具目录每一轮读它。
    pub fn snapshot(&self) -> Vec<Registration> {
        self.lock().values().cloned().collect()
    }

    /// 条目数。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Registration>> {
        self.entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

fn text_field(value: &Value, key: &str, max_chars: usize) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(max_chars).collect())
}
