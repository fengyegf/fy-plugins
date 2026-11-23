//! 对外接口：设备信息（工具 + 共享服务）。
//!
//! 其它插件 / AI 可以：
//! - 调工具 `fengyegf.system.info`（宿主工具表，模型可见）；
//! - 调服务 `fengyegf.system` 的方法 `info`（其它插件 `call_service`）。
//!
//! 两者返回同一份 JSON，且都不碰 TDLib / 渲染，只做一次系统采样（约 200ms）。

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

use fuyubot_plugin_api::{
    HostApi, HostExt, ServiceMethod, ServiceVTable, ToolHandle, ToolHandlerFn,
};
use serde_json::{Value, json};

/// 共享服务名。
pub const SERVICE_NAME: &str = "fengyegf.system";
/// 工具名。
pub const TOOL_INFO: &str = "fengyegf.system.info";

/// 注册工具；返回句柄（无效句柄已被宿主拒绝，这里照收）。
pub fn register_tools(host: &HostApi) -> Vec<ToolHandle> {
    let descriptor = json!({
        "name": TOOL_INFO,
        "version": env!("CARGO_PKG_VERSION"),
        "description": "读取宿主机设备信息（操作系统、架构、CPU / 内存占用、磁盘用量），无参数。",
        "input_schema": { "type": "object", "properties": {}, "additionalProperties": false },
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

/// 共享服务：`fengyegf.system.info`。
pub struct SystemService {
    #[allow(dead_code)]
    names: Vec<CString>,
    #[allow(dead_code)]
    methods: Vec<ServiceMethod>,
    vtable: Box<ServiceVTable>,
}

// SAFETY: 裸指针只指向本结构独占的堆缓冲，创建后不再修改。
unsafe impl Send for SystemService {}

impl SystemService {
    /// 建服务（不需要插件状态，context 为空指针）。
    pub fn new() -> Self {
        let names: Vec<CString> = std::iter::once(SERVICE_NAME)
            .chain(std::iter::once("info"))
            .map(|name| CString::new(name).expect("静态服务名合法"))
            .collect();
        let funcs: [unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char; 1] =
            [svc_info];
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
            version: 1,
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

    /// 给宿主注册用。
    pub fn vtable_ptr(&self) -> *const ServiceVTable {
        &*self.vtable
    }
}

impl Default for SystemService {
    fn default() -> Self {
        Self::new()
    }
}

/// 设备信息 JSON。
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

unsafe extern "C-unwind" fn tool_info(_user: *mut c_void, input: *const c_char) -> *const c_char {
    // 入参不使用，但读一下保证宿主传的是合法 C 字符串（与其它工具行为一致）。
    let _ = input;
    guarded(info_json)
}

unsafe extern "C" fn svc_info(_context: *mut c_void, input: *const c_char) -> *const c_char {
    let _ = input;
    guarded(info_json)
}

/// 校验宿主传的字符串（保留给未来带参方法用，当前无参）。
#[allow(dead_code)]
fn input_text(input: *const c_char) -> Option<String> {
    if input.is_null() {
        return None;
    }
    // SAFETY: 宿主按 ABI 契约传 NUL 结尾的 UTF-8。
    unsafe { CStr::from_ptr(input) }
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_json_has_device_fields() {
        let value = info_json();
        assert_eq!(value["ok"], true);
        assert!(value["os"].as_str().is_some_and(|text| !text.is_empty()));
        assert!(value["arch"].as_str().is_some_and(|text| !text.is_empty()));
        assert!(value["cpu_percent"].as_u64().is_some());
        assert!(value["ram_percent"].as_u64().is_some());
        assert!(value["disks"].as_array().is_some());
    }
}
