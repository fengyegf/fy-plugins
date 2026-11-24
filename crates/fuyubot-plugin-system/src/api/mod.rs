//! 对外接口：设备信息（工具 + 共享服务）。
//!
//! 两条通道、同一份 JSON：
//!
//! - 工具 fengyegf.system.info：给宿主 / AI 发现（register_tool，模型可见）；
//! - 服务 fengyegf.system：给别的插件 call_service 当场拿结果
//!   （provide_service_shared，文档「插件联动」里的命名服务通道）。
//!
//! ## 这个文件就是「提供方模板」
//!
//! 照抄下面三块即可给任意插件加一份符合约定的共享服务，不需要任何公共库：
//!
//! 1. 常量：服务名 = 插件 id，方法固定 describe / invoke（老方法只增不删）；
//! 2. SystemService：VTable + 名字缓冲 + 方法数组装进同一个结构体，
//!    注册后不再修改（宿主持有的是裸指针）；
//! 3. FFI 边界：catch_unwind + 线程局部回包 + 失败回 JSON。
//!
//! ## 服务协议（通用形状）
//!
//! - describe：{"ok":true,"service":…,"version":…,"protocol":…,"methods":[…],
//!   "capabilities":[{"name","description","input_schema",…}]}
//! - invoke：{"capability":…,"input":{…}} →
//!   成功 {"ok":true,"capability":…,"data":…}；失败 {"ok":false,"code":…,"error":…}
//!
//! 消费方（当前是 fengyegf.agent）只认服务名 / 方法名 / 这份 JSON，不链接本插件。

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

use fuyubot_plugin_api::{
    HostApi, HostExt, ServiceMethod, ServiceVTable, ToolHandle, ToolHandlerFn,
};
use serde_json::{Value, json};

/// 共享服务名 = 插件 id（反向域名）。
pub const SERVICE_NAME: &str = "fengyegf.system";
/// 工具名（也是能力名：模型看到的和 call_service 用的是同一个字符串）。
pub const TOOL_INFO: &str = "fengyegf.system.info";

/// 协议标识：字段只增不改；真破坏兼容时把协议版本加一。
const PROTOCOL: &str = "fuyubot.service/1";
/// 协议版本（同时写进 ServiceVTable.version）。
const PROTOCOL_VERSION: u32 = 1;
/// 服务方法：describe / invoke 是约定形状，info 是历史方法（只增不删）。
const SERVICE_METHODS: [&str; 3] = ["describe", "invoke", "info"];

/// 能力的说明（工具描述与服务 describe 共用一份，避免两处漂移）。
const INFO_DESCRIPTION: &str =
    "读取宿主机设备信息（操作系统、架构、CPU / 内存占用、磁盘用量），无参数。";

/// agent 的共享服务名：提供方把自己的能力注册进它的注册表。
const AGENT_SERVICE: &str = "fengyegf.agent";

/// 把本插件的能力注册进 agent 的注册表（提供方模板的最后一环）。
///
/// 就是文档「插件联动」的标准姿势：call_service(服务名, 方法名, JSON)。
/// 注册协议见 agent 插件的 agent/registry.rs；agent 没装 / 没开服务时只记一条
/// warn —— 插件单跑仍然可用（工具表通道还在）。
pub fn register_with_agent(host: &HostApi) {
    let payload = registration_payload();
    match host.call_service(AGENT_SERVICE, "register", &payload.to_string()) {
        Ok(reply) => host.info(&format!(
            "system: 已把能力注册进 agent（{AGENT_SERVICE}），回包 {}",
            if reply.trim().is_empty() {
                "空"
            } else {
                &reply
            }
        )),
        Err(error) => host.warn(&format!(
            "system: 注册进 {AGENT_SERVICE} 失败（agent 不在 / 没开服务？继续跑）：{error}"
        )),
    }
}

/// 注册载荷：能力名与工具同名，目标指向本插件服务的方法 invoke。
fn registration_payload() -> Value {
    json!({
        "name": TOOL_INFO,
        "description": INFO_DESCRIPTION,
        "input_schema": info_input_schema(),
        "output_schema": {},
        "timeout_ms": 30_000,
        "idempotent": true,
        "service": SERVICE_NAME,
        "capability": TOOL_INFO,
    })
}

/// 注册工具；返回句柄（无效句柄已被宿主拒绝，这里照收）。
pub fn register_tools(host: &HostApi) -> Vec<ToolHandle> {
    let descriptor = json!({
        "name": TOOL_INFO,
        "version": env!("CARGO_PKG_VERSION"),
        "description": INFO_DESCRIPTION,
        "input_schema": info_input_schema(),
        "output_schema": {},
        "timeout_ms": 30_000,
        "idempotent": true,
    });
    let handler: ToolHandlerFn = tool_info;
    let handle = host.register_tool(&descriptor.to_string(), Some(handler), std::ptr::null_mut());
    if handle.is_valid() {
        vec![handle]
    } else {
        host.warn("system: 注册工具 fengyegf.system.info 失败");
        Vec::new()
    }
}

/// 共享服务：VTable 与它引用的所有堆缓冲必须同生共死。
///
/// # Safety
///
/// 裸指针只指向自己拥有的 names / methods 堆缓冲与 Box 固定的 vtable；
/// 创建后不再修改。context 为空（system 的服务方法不需要实例状态）。
pub struct SystemService {
    #[allow(dead_code)]
    names: Vec<CString>,
    #[allow(dead_code)]
    methods: Vec<ServiceMethod>,
    vtable: Box<ServiceVTable>,
}

// SAFETY: 见 SystemService 的安全说明；裸指针指向的缓冲由本结构独占且地址稳定。
unsafe impl Send for SystemService {}

impl SystemService {
    /// 建服务并注册到宿主根表。
    pub fn register(host: &HostApi) -> Option<Self> {
        let names: Vec<CString> = std::iter::once(SERVICE_NAME)
            .chain(SERVICE_METHODS)
            .map(|name| CString::new(name).expect("静态服务名合法"))
            .collect();
        let funcs: [unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char; 3] =
            [svc_describe, svc_invoke, svc_info];
        let methods: Vec<ServiceMethod> = funcs
            .iter()
            .enumerate()
            .map(|(index, func)| ServiceMethod {
                name: names[index + 1].as_ptr(),
                func: Some(*func),
            })
            .collect();
        let vtable = Box::new(ServiceVTable {
            name: names[0].as_ptr(),
            version: PROTOCOL_VERSION,
            size: core::mem::size_of::<ServiceVTable>() as u32,
            context: std::ptr::null_mut(),
            methods: methods.as_ptr(),
            method_count: methods.len(),
        });
        let handle = host.provide_service_shared(SERVICE_NAME, &*vtable);
        if !handle.is_valid() {
            host.warn(&format!(
                "system: 注册共享服务 {SERVICE_NAME} 失败（同名服务已存在？），其它插件将只能走工具表"
            ));
            return None;
        }
        host.info(&format!(
            "system: 已提供共享服务 {SERVICE_NAME} v{PROTOCOL_VERSION}（describe / invoke / info）"
        ));
        Some(Self {
            names,
            methods,
            vtable,
        })
    }

    /// 给宿主注册用。
    pub fn vtable_ptr(&self) -> *const ServiceVTable {
        &*self.vtable
    }
}

impl Default for SystemService {
    fn default() -> Self {
        // 只有测试会走默认构造：不注册，只用来读 VTable 形状。
        let names: Vec<CString> = std::iter::once(SERVICE_NAME)
            .chain(SERVICE_METHODS)
            .map(|name| CString::new(name).expect("静态服务名合法"))
            .collect();
        let methods: Vec<ServiceMethod> = SERVICE_METHODS
            .iter()
            .enumerate()
            .map(|(index, _)| ServiceMethod {
                name: names[index + 1].as_ptr(),
                func: None,
            })
            .collect();
        let vtable = Box::new(ServiceVTable {
            name: names[0].as_ptr(),
            version: PROTOCOL_VERSION,
            size: core::mem::size_of::<ServiceVTable>() as u32,
            context: std::ptr::null_mut(),
            methods: methods.as_ptr(),
            method_count: methods.len(),
        });
        Self {
            names,
            methods,
            vtable,
        }
    }
}

/// 能力入参 schema（无参）。
fn info_input_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// describe：自描述 + 能力清单。
fn describe_json() -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "plugin": SERVICE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "protocol_version": PROTOCOL_VERSION,
        "methods": SERVICE_METHODS,
        "capabilities": [{
            "name": TOOL_INFO,
            "description": INFO_DESCRIPTION,
            "input_schema": info_input_schema(),
            "output_schema": {},
            "timeout_ms": 30_000,
            "idempotent": true,
            "tags": ["system"],
        }],
    })
}

/// invoke：按能力名分发，结果包在 data 里。
fn invoke_json(request: &Value) -> Value {
    let capability = request
        .get("capability")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if capability.is_empty() {
        return error_json("invalid", "invoke 需要 capability 字段");
    }
    match capability {
        TOOL_INFO => json!({
            "ok": true,
            "service": SERVICE_NAME,
            "capability": TOOL_INFO,
            "data": info_json(),
        }),
        other => error_json("not_found", &format!("{SERVICE_NAME} 没有能力 {other}")),
    }
}

/// 统一失败信封。
fn error_json(code: &str, message: &str) -> Value {
    json!({ "ok": false, "code": code, "error": message })
}

/// 设备信息 JSON（工具、invoke、历史 info 方法共用一份）。
fn info_json() -> Value {
    let snapshot = crate::core::sysinfo::collect();
    let gauge = |label: &str| {
        snapshot
            .gauges
            .iter()
            .find(|item| item.label == label)
            .map(|item| item.usage)
            .unwrap_or(0)
    };
    let disks: Vec<Value> = snapshot
        .disks
        .iter()
        .map(|disk| {
            json!({
                "label": disk.label,
                "used": disk.used,
                "total": disk.total,
                "usage_percent": disk.usage_percent,
            })
        })
        .collect();
    json!({
        "ok": true,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu_percent": gauge("CPU"),
        "ram_percent": gauge("RAM"),
        "disks": disks,
    })
}

// ── FFI 边界 ────────────────────────────────────────────────────────

thread_local! {
    /// 工具 / 服务返回值：宿主调完立刻拷走，下一次调用前有效。
    static REPLY: RefCell<CString> = RefCell::new(CString::new("{}").expect("静态 JSON 合法"));
}

fn reply_json(value: &Value) -> *const c_char {
    let text = serde_json::to_string(value)
        .unwrap_or_else(|_| r#"{"ok":false,"code":"json","error":"返回值序列化失败"}"#.to_owned())
        .replace('\0', "");
    let cstring = CString::new(text).unwrap_or_else(|_| {
        CString::new(r#"{"ok":false,"code":"json","error":"返回值含 NUL"}"#)
            .expect("静态 JSON 合法")
    });
    REPLY.with(|slot| {
        *slot.borrow_mut() = cstring;
        slot.borrow().as_ptr()
    })
}

/// 任何 panic 都在这里变成 JSON，绝不穿出 FFI。
fn guarded(handler: impl FnOnce() -> Value) -> *const c_char {
    match catch_unwind(AssertUnwindSafe(handler)) {
        Ok(value) => reply_json(&value),
        Err(_) => reply_json(&json!({
            "ok": false,
            "code": "panic",
            "error": "工具 / 服务处理器 panic（已拦截，未穿出 ABI）",
        })),
    }
}

/// 读入参：NULL / 空串当空对象。
fn input_value(input: *const c_char) -> Result<Value, String> {
    if input.is_null() {
        return Ok(json!({}));
    }
    // SAFETY: 宿主按 ABI 契约传 NUL 结尾的 UTF-8 JSON。
    let text = unsafe { CStr::from_ptr(input) }
        .to_str()
        .map_err(|error| format!("入参不是 UTF-8：{error}"))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(text).map_err(|error| format!("入参不是 JSON：{error}"))
}

unsafe extern "C-unwind" fn tool_info(_user: *mut c_void, _input: *const c_char) -> *const c_char {
    guarded(info_json)
}

unsafe extern "C" fn svc_describe(_context: *mut c_void, _input: *const c_char) -> *const c_char {
    guarded(describe_json)
}

unsafe extern "C" fn svc_invoke(_context: *mut c_void, input: *const c_char) -> *const c_char {
    match input_value(input) {
        Ok(request) => guarded(|| invoke_json(&request)),
        Err(message) => guarded(|| error_json("invalid", &message)),
    }
}

unsafe extern "C" fn svc_info(_context: *mut c_void, _input: *const c_char) -> *const c_char {
    guarded(info_json)
}
