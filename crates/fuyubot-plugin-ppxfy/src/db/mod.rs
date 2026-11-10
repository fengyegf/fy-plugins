//! galgame 作品记录存储。
//!
//! `/p` 核对完成后把成品记录留档；读写都经 mongo-plugin 的**进程级共享服务**
//! `storage.mongo`（见 [`service`]），ppxfy 自己不持有任何数据库连接。
//! 服务不在线（没装 mongo-plugin / 没起来）时，所有操作只返回错误，不影响
//! 下载、打包等主流程——调用方按「失败只警告」处理即可。
//!
//! 涉及的配置项（都在 `[plugin.ppxfy]` 段）：
//!
//! - `mongo_collection`：集合名，缺省 `ppxfy_galgame`；
//! - `mongodb_db`：覆盖 mongo-plugin 的默认库；留空 = 用服务端的默认库。
//!
//! 匹配键是 `title + brand`：
//! 查不到返回 `Ok(None)`，写入走 `update` + `upsert: true`，命中则
//! [`SaveOutcome::Updated`]，否则 [`SaveOutcome::Inserted`]。
//!
//! 边界：
//!
//! - 服务调用是**同步阻塞**的（mongo-plugin 内部自己 `block_on`），调用点必须在
//!   业务线程，不能放到宿主回调线程上（见 `host` 模块的线程约束）；
//! - 返回的 `Err` 都是能直接写日志的中文串，不 panic、不 unwrap；
//! - 读取宽容：未知字段忽略、缺字段用默认值、`updated_at` 的
//!   relaxed Extended JSON 日期原样保留（见 [`record`]）；
//! - `downloadLinks` 作为兼容字段保留，恒为 `""`。

mod record;
mod service;

use serde_json::Value;

pub use record::{Record, SaveOutcome};

/// 服务是否可用。只做方法表探测，不发起任何 IO，可以在每次流程开始时调用。
///
/// `/p` 的 persist 用它决定要不要尝试入库：没装 mongo-plugin 时省掉一次必然失败
/// 的调用，也不会把「服务没装」当成写库错误反复打日志。
pub fn available(ctx: &crate::host::Ctx) -> bool {
    service::available(&ctx.host)
}

/// 按 title + brand 查一条记录；没有返回 `Ok(None)`。
///
/// 预留给后续下载 / 管理流程（`/p` 已不再预查，直接靠 upsert 落库）。
///
/// `title` 为空直接报错：没有标题的查询只会扫全表。
#[allow(dead_code)]
pub fn fetch(ctx: &crate::host::Ctx, title: &str, brand: &str) -> Result<Option<Record>, String> {
    if title.trim().is_empty() {
        return Err("缺少 title，无法查询游戏记录".to_owned());
    }
    let collection = &ctx.config.mongo_collection;
    let db = ctx.config.mongodb_db.as_deref();
    let doc = service::find_one(&ctx.host, collection, db, title, brand)?;
    // `Option<Value> -> Result<Option<Record>>`：有文档才反序列化。
    doc.map(Record::from_doc).transpose()
}

/// 写入一条记录：按 title + brand 命中则更新，否则插入。
///
/// `title` 为空直接报错——空标题配合 upsert 会把一批记录都当成同一条。
pub fn save(ctx: &crate::host::Ctx, record: &Record) -> Result<SaveOutcome, String> {
    if record.title.trim().is_empty() {
        return Err("缺少 title，无法匹配游戏记录".to_owned());
    }
    let collection = &ctx.config.mongo_collection;
    let db = ctx.config.mongodb_db.as_deref();
    let response = service::update(&ctx.host, collection, db, record)?;
    Ok(outcome(&response))
}

/// `update` 的响应 → [`SaveOutcome`]：upsert 插入新文档时 `upserted` 是新 `_id`，
/// 命中已有文档时为 `null`（甚至字段缺失）。
fn outcome(response: &Value) -> SaveOutcome {
    match response.get("upserted") {
        Some(value) if !value.is_null() => SaveOutcome::Inserted,
        _ => SaveOutcome::Updated,
    }
}
