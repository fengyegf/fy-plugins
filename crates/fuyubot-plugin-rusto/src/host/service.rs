//! 跨插件面：共享服务 `rusto.ocr`（v1）+ 工具 `rusto.ocr`。
//!
//! 两条通道的分工（依据 docs/plugin/linkage）：
//!
//! - **服务**是主通道：`provide_service_shared` 注册进进程级根表，兄弟插件用
//!   `call_service("rusto.ocr", "recognize", json)` 同步拿 JSON；
//! - **工具**是发现面：`register_tool` 给宿主 / AI 一张 schema，`call_tool_result` 同步调用。
//!
//! ABI 约束（不遵守会 abort 整个宿主，不是只挂插件）：
//!
//! - 方法跑在**调用方线程**上，不能做重活；同步路径只做「懒加载 + 识别」，
//!   需要长任务时用 `async:true` 拿 `task_id`，结果走 `rusto.ocr.done` 事件；
//! - 方法里 `catch_unwind` 兜底，返回 NULL 表示失败（v6 起 NULL = 失败，空串 = 成功无输出）；
//! - 返回值来自插件自己的线程局部缓冲，宿主当场拷走，插件不负责释放。

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fuyubot_plugin_api::{HostApi, HostExt, ServiceMethod, ServiceVTable, ToolHandle};
use serde_json::{Value, json};

use crate::engine::{EngineError, ImageInput, OcrOutput, OcrRequest};
use crate::host::Ctx;
use crate::host::download;

/// 服务名（反向域名，跨插件共享必须用这个字符串）。
pub const SERVICE: &str = "rusto.ocr";
/// 服务协议版本：字段只增不改，破坏性改动时 +1。
pub const VERSION: u32 = 1;
/// 工具名。
pub const TOOL: &str = "rusto.ocr";
/// 内联图片（`data`）的原始字节上限：ABI 字符串是三次拷贝，再大就不如走 file_id。
const MAX_INLINE_BYTES: usize = 8 * 1024 * 1024;

/// 异步识别的任务号。
static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

// 服务方法返回值的线程局部缓冲；宿主读完即弃。
thread_local! {
    static REPLY: RefCell<CString> = RefCell::new(CString::default());
}

/// 把 JSON 文本放进线程局部缓冲并返回指针。
fn reply(text: &str) -> *const c_char {
    REPLY.with(|slot| {
        *slot.borrow_mut() = CString::new(text).unwrap_or_default();
        slot.borrow().as_ptr()
    })
}

/// 统一失败应答。
fn reply_error(code: &str, message: &str) -> *const c_char {
    reply(&json!({ "ok": false, "code": code, "message": message }).to_string())
}

/// 服务实例：VTable 与它引用的缓冲必须与注册同生共死。
pub struct Service {
    /// 交给宿主的 VTable。
    #[allow(dead_code)]
    vtable: ServiceVTable,
    /// `vtable.methods` 指向的数组；注册后不再扩容。
    #[allow(dead_code)]
    methods: Vec<ServiceMethod>,
    /// `vtable.name` 与每个方法名指向的缓冲。
    #[allow(dead_code)]
    names: Vec<CString>,
    /// 方法回调的 `context` 指向它；本结构保活这份 Arc。
    #[allow(dead_code)]
    ctx: Arc<Ctx>,
}

// SAFETY: 注册之后 Service 的内容不再变化；VTable 里的裸指针都指向本结构自己持有的
// 堆缓冲（names / methods），随结构一起移动不会失效。宿主会从任意线程调用方法，
// 而方法只读 Ctx、只往线程局部缓冲写返回值，所以跨线程持有是安全的。
unsafe impl Send for Service {}

impl Service {
    /// 注册服务；同名冲突时返回 `None`（宿主只打一条 warn）。
    pub fn register(host: &HostApi, ctx: Arc<Ctx>) -> Option<Box<Self>> {
        let names = vec![
            CString::new(SERVICE).ok()?,
            CString::new("recognize").ok()?,
            CString::new("warm").ok()?,
            CString::new("status").ok()?,
        ];
        let methods = vec![
            ServiceMethod {
                name: names[1].as_ptr(),
                func: Some(method_recognize),
            },
            ServiceMethod {
                name: names[2].as_ptr(),
                func: Some(method_warm),
            },
            ServiceMethod {
                name: names[3].as_ptr(),
                func: Some(method_status),
            },
        ];
        // ⚠️ 宿主持有的是**裸指针**：VTable 必须和它引用的缓冲一起活在堆上。
        // 先装箱拿到稳定地址，再取 &service.vtable 的指针；绝不能把栈上的临时变量地址交出去
        // （那会在 register 返回后变成悬垂指针，宿主读到的是垃圾）。
        let mut service = Box::new(Self {
            vtable: ServiceVTable {
                name: names[0].as_ptr(),
                version: VERSION,
                size: core::mem::size_of::<ServiceVTable>() as u32,
                context: Arc::as_ptr(&ctx) as *mut c_void,
                methods: methods.as_ptr(),
                method_count: methods.len(),
            },
            methods,
            names,
            ctx,
        });
        let handle = host.provide_service_shared(SERVICE, &service.vtable as *const ServiceVTable);
        if !handle.is_valid() {
            host.warn(&format!(
                "rusto: 注册共享服务 {SERVICE} 失败（同名服务已存在？），其它插件将调不到 OCR"
            ));
            return None;
        }
        host.info(&format!("rusto: 已提供共享服务 {SERVICE} v{VERSION}"));
        // 只为了让编译器知道 service 被移动回来；指针在装箱那一刻就已经稳定。
        service.vtable.name = service.names[0].as_ptr();
        Some(service)
    }
}

/// 注册工具 `rusto.ocr`（给宿主 / AI 发现与调用）。
pub fn register_tool(host: &HostApi, ctx: Arc<Ctx>) -> Option<ToolHandle> {
    let descriptor = json!({
        "name": TOOL,
        "version": env!("CARGO_PKG_VERSION"),
        "description": "识别图片中的文字（纯 Rust OCR / PP-OCRv6）：给 file_id、本机路径或 base64 图片，返回文本与行级明细",
        "input_schema": {
            "type": "object",
            "properties": {
                "file_id": { "type": "integer", "description": "TDLib 文件 id（优先）" },
                "path": { "type": "string", "description": "本机图片路径" },
                "data": { "type": "string", "description": "data URL 或 base64，原始 ≤ 8 MiB" },
                "detail": { "type": "boolean", "description": "是否返回行级 text/score/box" },
                "max_side": { "type": "integer", "description": "长边限幅，默认 960（越小越快）" },
                "timeout_ms": { "type": "integer", "description": "预算，默认 20000" }
            },
            "anyOf": [{ "required": ["file_id"] }, { "required": ["path"] }, { "required": ["data"] }]
        },
        "output_schema": {
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" },
                "text": { "type": "string" },
                "elapsed_ms": { "type": "integer" },
                "lines": { "type": "array" },
                "code": { "type": "string" },
                "message": { "type": "string" }
            }
        },
        "idempotent": true,
        "timeout_ms": 30000
    })
    .to_string();
    let handle = host.register_tool(
        &descriptor,
        Some(tool_recognize),
        Arc::as_ptr(&ctx) as *mut c_void,
    );
    if !handle.is_valid() {
        host.warn("rusto: 注册工具 rusto.ocr 失败（重名？），只是少一个发现面，服务仍可用");
        return None;
    }
    host.info("rusto: 已注册工具 rusto.ocr");
    Some(handle)
}

/// 服务方法 `recognize`。
unsafe extern "C" fn method_recognize(context: *mut c_void, input: *const c_char) -> *const c_char {
    dispatch_recognize(context, input, false)
}

/// 工具处理器：与服务同构，只是不支持 `async`。
unsafe extern "C-unwind" fn tool_recognize(
    user: *mut c_void,
    input: *const c_char,
) -> *const c_char {
    dispatch_recognize(user, input, true)
}

/// 把注册时的 `Arc<Ctx>` 指针借成一份 Arc（不夺走原引用）。
///
/// # Safety
/// `context` 必须来自 `Service::register` 里 `Arc::as_ptr`，且对应的 Arc 仍然活着。
unsafe fn borrow_ctx(context: *mut c_void) -> Arc<Ctx> {
    let pointer = context as *const Ctx;
    // SAFETY: 由调用方保证指针有效；先加一份强引用再包装，保证原引用不被吃掉。
    unsafe { Arc::increment_strong_count(pointer) };
    // SAFETY: 与上同一保证。
    unsafe { Arc::from_raw(pointer) }
}

/// `recognize` 的公共实现：解析 → 识别 → JSON。
fn dispatch_recognize(
    context: *mut c_void,
    input: *const c_char,
    from_tool: bool,
) -> *const c_char {
    if context.is_null() || input.is_null() {
        return reply_error("bad_input", "回调上下文或入参为空");
    }
    let result = std::panic::catch_unwind(|| {
        // SAFETY: 由 register 给出的 context；input 是临时 NUL 结尾字符串。
        let ctx = unsafe { borrow_ctx(context) };
        let text = unsafe { CStr::from_ptr(input) }.to_string_lossy();
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                return json!({
                    "ok": false,
                    "code": "bad_input",
                    "message": format!("入参不是 JSON：{error}"),
                });
            }
        };
        run_recognize(&ctx, &value, from_tool)
    });
    match result {
        Ok(value) => reply(&value.to_string()),
        Err(_) => reply_error("internal", "识别过程中发生 panic"),
    }
}

/// 同步 / 异步识别。
fn run_recognize(ctx: &Arc<Ctx>, input: &Value, from_tool: bool) -> Value {
    let timeout = Duration::from_millis(
        input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(ctx.config.timeout.as_millis() as u64)
            .clamp(1_000, 120_000),
    );
    let detail = input
        .get("detail")
        .and_then(Value::as_bool)
        .unwrap_or(ctx.config.detail_default);
    let max_side = input
        .get("max_side")
        .and_then(Value::as_u64)
        .map(|value| value.clamp(320, 4096) as u32)
        .unwrap_or(ctx.config.max_side);

    let request = match build_request(ctx, input, detail, max_side, timeout) {
        Ok(request) => request,
        Err(error) => return error_json(error),
    };

    // async 只在服务通道上支持：工具是同步调用，没有回投通道。
    if !from_tool && input.get("async").and_then(Value::as_bool).unwrap_or(false) {
        let task_id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
        let task_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("rusto-async".to_owned())
            .spawn(move || {
                let payload = match task_ctx.engine.recognize(request) {
                    Ok(output) => result_json(&task_ctx, &output, detail),
                    Err(error) => error_json(error),
                };
                let event = json!({ "task_id": task_id, "result": payload });
                let _ = task_ctx.host.emit("rusto.ocr.done", &event.to_string());
            });
        if spawned.is_err() {
            return error_json(EngineError::Internal("异步线程启动失败".to_owned()));
        }
        return json!({ "ok": true, "task_id": task_id, "pending": true });
    }

    match ctx.engine.recognize(request) {
        Ok(output) => result_json(ctx, &output, detail),
        Err(error) => error_json(error),
    }
}

/// 把服务入参翻译成引擎请求。
fn build_request(
    ctx: &Ctx,
    input: &Value,
    detail: bool,
    max_side: u32,
    timeout: Duration,
) -> Result<OcrRequest, EngineError> {
    let image = if let Some(file_id) = input.get("file_id").and_then(Value::as_i64) {
        let path = download::file_path(&ctx.host, file_id, timeout).map_err(|error| {
            EngineError::BadInput(format!("下载 file_id={file_id} 失败：{error}"))
        })?;
        ImageInput::Path(path)
    } else if let Some(path) = input.get("path").and_then(Value::as_str) {
        ImageInput::Path(std::path::PathBuf::from(path))
    } else if let Some(data) = input.get("data").and_then(Value::as_str) {
        let bytes = decode_inline(data)?;
        ImageInput::Bytes(Arc::new(bytes))
    } else {
        return Err(EngineError::BadInput(
            "需要 file_id / path / data 三者之一".to_owned(),
        ));
    };
    Ok(OcrRequest {
        image,
        detail,
        max_side,
        timeout,
    })
}

/// 解 base64 / data URL，并做大小上限检查。
fn decode_inline(data: &str) -> Result<Vec<u8>, EngineError> {
    let payload = match data.split_once(',') {
        Some((head, rest)) if head.starts_with("data:") => rest,
        _ => data,
    };
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload.trim())
        .map_err(|error| EngineError::BadInput(format!("data 不是合法 base64：{error}")))?;
    if bytes.len() > MAX_INLINE_BYTES {
        return Err(EngineError::TooLarge);
    }
    Ok(bytes)
}

/// 成功应答。
fn result_json(ctx: &Ctx, output: &OcrOutput, detail: bool) -> Value {
    let engine = ctx.engine.status();
    let lines: Vec<Value> = if detail {
        output
            .lines
            .iter()
            .map(|line| {
                json!({
                    "text": line.text,
                    "score": line.score,
                    "box": line.quad,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    json!({
        "ok": true,
        "text": output.text,
        "elapsed_ms": output.elapsed_ms,
        "truncated": output.truncated,
        "lines": lines,
        "engine": {
            "backend": engine.get("backend").and_then(Value::as_str).unwrap_or("rusto"),
            "model": engine.get("model").and_then(Value::as_str).unwrap_or_default(),
            "rec_workers": ctx.config.rec_workers,
        },
    })
}

/// 失败应答（`{ok:false, code, message}`，字段只增不改）。
fn error_json(error: EngineError) -> Value {
    json!({
        "ok": false,
        "code": error.code(),
        "message": error.to_string(),
    })
}

/// 服务方法 `warm`：预加载模型。
unsafe extern "C" fn method_warm(context: *mut c_void, _input: *const c_char) -> *const c_char {
    if context.is_null() {
        return reply_error("bad_input", "回调上下文为空");
    }
    let result = std::panic::catch_unwind(|| {
        // SAFETY: 由 register 给出的 context。
        let ctx = unsafe { borrow_ctx(context) };
        match ctx.engine.warm() {
            Ok(()) => json!({ "ok": true, "loaded": true }),
            Err(error) => error_json(error),
        }
    });
    match result {
        Ok(value) => reply(&value.to_string()),
        Err(_) => reply_error("internal", "预热过程中发生 panic"),
    }
}

/// 服务方法 `status`：引擎状态。
unsafe extern "C" fn method_status(context: *mut c_void, _input: *const c_char) -> *const c_char {
    if context.is_null() {
        return reply_error("bad_input", "回调上下文为空");
    }
    let result = std::panic::catch_unwind(|| {
        // SAFETY: 由 register 给出的 context。
        let ctx = unsafe { borrow_ctx(context) };
        ctx.engine.status()
    });
    match result {
        Ok(value) => reply(&value.to_string()),
        Err(_) => reply_error("internal", "读取状态时发生 panic"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_plain_and_data_url() {
        let plain = decode_inline("aGVsbG8=").unwrap();
        assert_eq!(plain, b"hello");
        let url = decode_inline("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(url, b"hello");
    }

    #[test]
    fn rejects_bad_base64() {
        assert!(matches!(
            decode_inline("not base64 !!"),
            Err(EngineError::BadInput(_))
        ));
    }

    #[test]
    fn error_payload_has_code() {
        let value = error_json(EngineError::Busy);
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], "engine_busy");
        assert!(value["message"].as_str().is_some());
    }
}
