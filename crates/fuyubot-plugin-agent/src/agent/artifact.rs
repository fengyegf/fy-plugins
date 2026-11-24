//! 长工具结果的短期缓存与字段筛选。
//!
//! 工具结果超过阈值时不再整段回填：原文按 id 存在内存里（TTL / 条数 / 字节数都有上限），
//! 回给模型第一页 + artifact id；模型可以用 [`RESULT_TOOL`] 翻页，或按 JSON 路径只取
//! 自己关心的字段（例如只取排行榜的 data.items[*].name），不必吃下整段 JSON。

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::core::consts;

/// 读取长结果的工具名（agent 自己注册，模型通过工具表调用）。
pub const RESULT_TOOL: &str = "fengyegf.agent.result";

/// 缓存统计（describe / 诊断用）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ArtifactStats {
    /// 当前条数。
    pub count: usize,
    /// 当前占用字节数（UTF-8）。
    pub bytes: usize,
}

/// 长结果缓存：内存、带 TTL 与容量上限，进程重启即清空。
pub struct ArtifactStore {
    inner: Mutex<Inner>,
    ttl: Duration,
    max_entries: usize,
    max_bytes: usize,
    max_item_bytes: usize,
    next_id: AtomicU64,
}

struct Inner {
    entries: HashMap<String, Entry>,
    /// 插入顺序：淘汰最旧的用。
    order: VecDeque<String>,
    bytes: usize,
}

struct Entry {
    text: Arc<str>,
    created: Instant,
}

impl ArtifactStore {
    /// 按内置配额建。
    pub fn new() -> Self {
        Self::with_limits(
            Duration::from_secs(consts::ARTIFACT_TTL_SECS),
            consts::ARTIFACT_MAX_ENTRIES,
            consts::ARTIFACT_MAX_BYTES,
            consts::ARTIFACT_MAX_ITEM_BYTES,
        )
    }

    /// 指定配额（测试 / 诊断用）。
    pub fn with_limits(
        ttl: Duration,
        max_entries: usize,
        max_bytes: usize,
        max_item_bytes: usize,
    ) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                order: VecDeque::new(),
                bytes: 0,
            }),
            ttl,
            max_entries,
            max_bytes,
            max_item_bytes: max_item_bytes.min(max_bytes),
            next_id: AtomicU64::new(1),
        }
    }

    /// 存一段结果；返回 artifact id。超过单条上限（太大不适合放内存）返回 None。
    pub fn put(&self, text: &str) -> Option<String> {
        if self.max_entries == 0
            || self.max_bytes == 0
            || text.is_empty()
            || text.len() > self.max_item_bytes
        {
            return None;
        }
        let id = self.next_id();
        let mut inner = self.lock();
        prune_locked(&mut inner, self.ttl);
        while inner.entries.len() >= self.max_entries || inner.bytes + text.len() > self.max_bytes {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            if let Some(entry) = inner.entries.remove(&oldest) {
                inner.bytes = inner.bytes.saturating_sub(entry.text.len());
            }
        }
        inner.bytes += text.len();
        inner.order.push_back(id.clone());
        inner.entries.insert(
            id.clone(),
            Entry {
                text: Arc::from(text),
                created: Instant::now(),
            },
        );
        Some(id)
    }

    /// 取一段结果（过期 / 不存在返回 None）。
    pub fn get(&self, id: &str) -> Option<Arc<str>> {
        let mut inner = self.lock();
        prune_locked(&mut inner, self.ttl);
        inner.entries.get(id).map(|entry| Arc::clone(&entry.text))
    }

    /// 当前条数与占用字节。
    pub fn stats(&self) -> ArtifactStats {
        let mut inner = self.lock();
        prune_locked(&mut inner, self.ttl);
        ArtifactStats {
            count: inner.entries.len(),
            bytes: inner.bytes,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// 生成不可推测的 id：RandomState 的随机键 + 进程内计数 + 当前时间。
    ///
    /// 长结果里可能有别的会话 / 其它工具的数据；id 不能是连续数字，否则模型
    /// （被用户诱导时）可以顺序枚举出别人的 artifact。
    fn next_id(&self) -> String {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(self.next_id.fetch_add(1, Ordering::Relaxed));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        hasher.write_u128(nanos);
        format!("r{:016x}", hasher.finish())
    }
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 清掉过期的条目（只在队首判断；插入顺序 = 创建时间顺序）。
fn prune_locked(inner: &mut Inner, ttl: Duration) {
    let now = Instant::now();
    while let Some(id) = inner.order.front().cloned() {
        let expired = inner
            .entries
            .get(&id)
            .is_none_or(|entry| now.duration_since(entry.created) >= ttl);
        if !expired {
            break;
        }
        inner.order.pop_front();
        if let Some(entry) = inner.entries.remove(&id) {
            inner.bytes = inner.bytes.saturating_sub(entry.text.len());
        }
    }
}

/// 一段 JSON 路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// 对象字段；当前是数组时对每个元素取同名字段（省掉 [*]）。
    Key(String),
    /// 数组下标。
    Index(usize),
    /// [*]：数组元素 / 对象的值。
    Wildcard,
}

/// 解析路径：data.items[*].name、items[0].title、fields.name，可带 $ 前缀。
pub fn parse_path(raw: &str) -> Result<Vec<PathSegment>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("路径为空".to_owned());
    }
    let path = trimmed
        .strip_prefix('$')
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let chars: Vec<char> = path.chars().collect();
    let mut segments = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        match chars[index] {
            '.' => index += 1,
            '[' => {
                let start = index + 1;
                let mut end = start;
                while end < chars.len() && chars[end] != ']' {
                    end += 1;
                }
                if end >= chars.len() {
                    return Err("路径里的 [ 没有闭合".to_owned());
                }
                let inside: String = chars[start..end].iter().collect();
                let inside = inside.trim();
                if inside == "*" {
                    segments.push(PathSegment::Wildcard);
                } else {
                    let position: usize = inside
                        .parse()
                        .map_err(|_| format!("数组下标不是数字：{inside}"))?;
                    segments.push(PathSegment::Index(position));
                }
                index = end + 1;
            }
            _ => {
                let start = index;
                while index < chars.len() && chars[index] != '.' && chars[index] != '[' {
                    index += 1;
                }
                let key: String = chars[start..index].iter().collect();
                if key.is_empty() {
                    return Err("路径里有空字段名".to_owned());
                }
                segments.push(PathSegment::Key(key));
            }
        }
    }
    if segments.is_empty() {
        return Err("路径为空".to_owned());
    }
    Ok(segments)
}

/// 按路径取值；返回顺序与 JSON 里的顺序一致（数组按下标）。
pub fn select<'a>(root: &'a Value, path: &str) -> Result<Vec<&'a Value>, String> {
    let segments = parse_path(path)?;
    let mut current: Vec<&Value> = vec![root];
    for segment in segments {
        let mut next: Vec<&Value> = Vec::new();
        for value in current {
            match &segment {
                PathSegment::Key(key) => match value {
                    Value::Object(map) => {
                        if let Some(child) = map.get(key) {
                            next.push(child);
                        }
                    }
                    Value::Array(items) => {
                        for item in items {
                            if let Value::Object(map) = item
                                && let Some(child) = map.get(key)
                            {
                                next.push(child);
                            }
                        }
                    }
                    _ => {}
                },
                PathSegment::Index(position) => {
                    if let Value::Array(items) = value
                        && let Some(child) = items.get(*position)
                    {
                        next.push(child);
                    }
                }
                PathSegment::Wildcard => match value {
                    Value::Array(items) => next.extend(items.iter()),
                    Value::Object(map) => next.extend(map.values()),
                    _ => {}
                },
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    Ok(current)
}

/// 给模型看的 JSON 形状摘要：顶层类型 + 一级键 / 数组长度与元素键。
/// 完整结构不必塞进上下文，但它得知道能按哪些字段过滤。
pub fn json_shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let keys: Vec<&String> = map.keys().take(24).collect();
            json!({ "type": "object", "keys": keys })
        }
        Value::Array(items) => {
            let mut shape = json!({ "type": "array", "length": items.len() });
            if let Some(Value::Object(first)) = items.first() {
                let keys: Vec<&String> = first.keys().take(24).collect();
                shape["item_keys"] = json!(keys);
            }
            shape
        }
        Value::String(_) => json!({ "type": "string" }),
        Value::Number(_) => json!({ "type": "number" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Null => json!({ "type": "null" }),
    }
}

/// 回给模型的长结果信封：第一页 + artifact id + 翻页 / 过滤提示。
pub fn envelope(tool: &str, id: &str, text: &str, page_chars: usize) -> String {
    let page_chars = page_chars.max(200);
    let total = text.chars().count();
    let pages = total.div_ceil(page_chars);
    let content: String = text.chars().take(page_chars).collect();
    let hint = format!(
        "结果过长，只回填第 1 页。用 {RESULT_TOOL} 翻页（{{\"artifact\":\"{id}\",\"page\":2}}），或按 JSON 字段过滤（{{\"artifact\":\"{id}\",\"path\":\"data.items[*].name\"}}）。"
    );
    let mut payload = json!({
        "ok": true,
        "truncated": true,
        "tool": tool,
        "artifact": id,
        "total_chars": total,
        "page": 1,
        "page_count": pages,
        "page_chars": page_chars,
        "content": content,
        "hint": hint,
    });
    if let Ok(root) = serde_json::from_str::<Value>(text) {
        payload["shape"] = json_shape(&root);
    }
    payload.to_string()
}

/// 单个过滤值的字符上限：名字 / 标题这类字段够用，避免一个超长值把一页撑满。
pub fn clip_value(value: &Value, max_chars: usize) -> Value {
    match value {
        Value::String(text) if text.chars().count() > max_chars => {
            let cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
            Value::String(format!("{cut}…"))
        }
        // 对象 / 数组也按序列化后的长度截断成字符串：保证单值一定放得进一页，
        // 否则过滤一个巨大对象时会「返回 0 条 + 同一个 next_offset」原地打转。
        Value::Array(_) | Value::Object(_) => {
            let rendered = value.to_string();
            if rendered.chars().count() > max_chars {
                let cut: String = rendered.chars().take(max_chars.saturating_sub(1)).collect();
                Value::String(format!("{cut}…"))
            } else {
                value.clone()
            }
        }
        other => other.clone(),
    }
}
