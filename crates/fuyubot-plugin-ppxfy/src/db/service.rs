//! `storage.mongo` 服务的客户端。
//!
//! 不直连 MongoDB：连接串、默认库、超时都归 mongo-plugin 管，它把能力注册成
//! **进程级共享服务** `storage.mongo`，暴露 `find_one` / `update` 等方法。
//! ppxfy 只按名字取方法、把 JSON 塞进去、再把 `{"ok": ...}` 拆开。
//!
//! 协议（对照 mongo-plugin 的 `service.rs` / `lib.rs`）：
//!
//! ```text
//! find_one 入参 {"collection": str, "filter": object, "db"?: str}
//! find_one 成功 {"ok": true, "doc": object | null}
//! update   入参 {"collection": str, "filter": object,
//!               "update": {"$set": object, "$currentDate": {...}},
//!               "upsert": bool, "db"?: str}
//! update   成功 {"ok": true, "matched": u64, "modified": u64,
//!               "upserted": <id> | null}
//! 失败（任意方法）{"ok": false, "error": str}
//! ```
//!
//! 三条 ABI 约束决定了这里的写法：
//!
//! 1. 服务表由提供方持有，插件卸载 / 服务重注册都可能换表，**方法指针不缓存**，
//!    每次调用都重新 [`HostApi::get_service`]；
//! 2. 返回的字符串归提供方所有（线程局部缓冲，读完即弃），这里立刻拷成
//!    [`Value`]，绝不 free 别人的指针；
//! 3. 方法函数是同步的，mongo-plugin 内部自己 `block_on`；调用点都在业务线程，
//!    不在宿主回调线程上（见 `host` 模块的线程约束）。
//!
//! 入参构造抽成了纯函数 [`find_payload`] / [`update_payload`]，单测不碰宿主。

use std::ffi::{CStr, CString};

use fuyubot_plugin_api::{HostApi, ServiceMethodFn};
use serde_json::{Map, Value, json};

use super::record::Record;

/// 提供方注册的服务名（Cordis 契约名，不是插件名）。
pub(crate) const SERVICE: &str = "storage.mongo";

/// 服务缺失时的统一文案：装没装、起没起都归这一句，日志里一看就懂。
const UNAVAILABLE: &str = "storage.mongo 服务不可用（mongo-plugin 未安装或未启动）";

/// 服务是否在线：`find_one` 与 `update` 两个方法都得有。
///
/// 只看服务名不够——插件在但版本过老、方法表里没有我们需要的方法时，
/// 和完全没装一样不能干活。
pub(crate) fn available(host: &HostApi) -> bool {
    method(host, "find_one").is_some() && method(host, "update").is_some()
}

/// 按方法名从服务表里取函数指针；服务没装、方法不存在都返回 `None`。
fn method(host: &HostApi, name: &str) -> Option<ServiceMethodFn> {
    let vtable = host.get_service(SERVICE);
    if vtable.is_null() {
        return None;
    }
    // SAFETY: `get_service` 只返回宿主校验过的服务表或 null（上面已挡）；
    // 注册方保证表在进程存活期间有效，`methods` 指向 `method_count` 个连续项。
    let table = unsafe { &*vtable };
    if table.methods.is_null() {
        return None;
    }
    // SAFETY: 同上，`methods` 是连续的方法表。
    let methods = unsafe { std::slice::from_raw_parts(table.methods, table.method_count) };

    methods.iter().find_map(|entry| {
        if entry.name.is_null() {
            return None;
        }
        // SAFETY: 方法名由注册方保证是 NUL 结尾 UTF-8（注册时泄漏成 'static）。
        let current = unsafe { CStr::from_ptr(entry.name) }.to_str().ok()?;
        (current == name).then_some(entry.func).flatten()
    })
}

/// 调一次方法，返回拆掉 `ok` 外壳后的响应对象。
fn call(host: &HostApi, name: &str, payload: &Value) -> Result<Value, String> {
    let Some(func) = method(host, name) else {
        return Err(UNAVAILABLE.to_owned());
    };
    let input = CString::new(payload.to_string())
        .map_err(|_| format!("storage.mongo.{name} 入参含 NUL"))?;

    // SAFETY: `func` 取自刚才查到的活服务表，调用约定是「NUL 结尾 JSON 进，
    // 提供方持有的 NUL 结尾 JSON 出」；返回指针在下次同线程调用前有效，
    // 下面立刻用 CStr 拷走，不再碰原指针。context 传 null 与 mongo-plugin
    // 的调用范例一致（该服务不读 context）。
    let raw = unsafe { func(std::ptr::null_mut(), input.as_ptr()) };
    if raw.is_null() {
        return Err(format!("storage.mongo.{name} 返回空指针"));
    }
    // SAFETY: 同上，`raw` 是提供方持有的 NUL 结尾 UTF-8 字符串。
    let text = unsafe { CStr::from_ptr(raw) }
        .to_str()
        .map_err(|err| format!("storage.mongo.{name} 响应不是 UTF-8：{err}"))?;
    parse_response(name, text)
}

/// 拆分 `{"ok": true, ...}` / `{"ok": false, "error": ...}` 外壳。
///
/// 抽成纯函数：这段解析最容易出协议偏差，单测直接喂字符串就能覆盖，
/// 不需要造一个 `HostApi`（它的字段在 SDK 里是私有的）。
pub(crate) fn parse_response(name: &str, text: &str) -> Result<Value, String> {
    let response: Value = serde_json::from_str(text)
        .map_err(|err| format!("storage.mongo.{name} 响应不是 JSON：{err}（原文：{text}）"))?;

    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        let message = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(format!("storage.mongo.{name} 失败：{message}"));
    }
    Ok(response)
}

/// `find_one` 的入参。`db` 为 `None` 时不带该键，由提供方用自己配置的默认库。
pub(crate) fn find_payload(collection: &str, db: Option<&str>, title: &str, brand: &str) -> Value {
    let mut payload = Map::new();
    payload.insert(
        "collection".to_owned(),
        Value::String(collection.to_owned()),
    );
    payload.insert(
        "filter".to_owned(),
        json!({ "title": title, "brand": brand }),
    );
    if let Some(db) = db {
        payload.insert("db".to_owned(), Value::String(db.to_owned()));
    }
    Value::Object(payload)
}

/// `update` 的入参：按 title + brand 匹配，`$set` 铺开记录，`upsert: true`。
///
/// `updated_at` 不放进 `$set`，而是用 `$currentDate` 让数据库服务器生成
/// BSON Date，不受本机时钟 / 时区影响。
pub(crate) fn update_payload(
    collection: &str,
    db: Option<&str>,
    record: &Record,
) -> Result<Value, String> {
    let update = json!({
        "$set": record.to_set()?,
        "$currentDate": { "updated_at": true },
    });

    let mut payload = Map::new();
    payload.insert(
        "collection".to_owned(),
        Value::String(collection.to_owned()),
    );
    payload.insert(
        "filter".to_owned(),
        json!({ "title": record.title, "brand": record.brand }),
    );
    payload.insert("update".to_owned(), update);
    payload.insert("upsert".to_owned(), Value::Bool(true));
    if let Some(db) = db {
        payload.insert("db".to_owned(), Value::String(db.to_owned()));
    }
    Ok(Value::Object(payload))
}

/// 查询一条记录；`doc: null` 表示库里没有。
pub(crate) fn find_one(
    host: &HostApi,
    collection: &str,
    db: Option<&str>,
    title: &str,
    brand: &str,
) -> Result<Option<Value>, String> {
    let response = call(
        host,
        "find_one",
        &find_payload(collection, db, title, brand),
    )?;
    doc_from_response(response)
}

/// 从 `find_one` 响应里取文档对象。抽成纯函数便于单测响应解析。
pub(crate) fn doc_from_response(response: Value) -> Result<Option<Value>, String> {
    match response.get("doc") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(_)) => Ok(response.get("doc").cloned()),
        Some(other) => Err(format!(
            "storage.mongo.find_one 返回的 doc 不是对象：{other}"
        )),
    }
}

/// 写入一条记录（已存在则更新、不存在则插入），返回原始响应供调用方判断结果。
pub(crate) fn update(
    host: &HostApi,
    collection: &str,
    db: Option<&str>,
    record: &Record,
) -> Result<Value, String> {
    call(host, "update", &update_payload(collection, db, record)?)
}
