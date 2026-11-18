//! 测试替身宿主：不接 TDLib、不接真宿主，只验证 ABI 接线。
//!
//! 覆盖的是最容易出事、又最难在真机上复现的部分：
//! 订阅注册、命令表登记、共享服务 VTable 打包、工具登记、服务方法回调。
//!
//! 只在 `cfg(test)` 下编译，不进发布产物。

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};

use fuyubot_plugin_api::{
    ABI_VERSION, EventCallbackFn, HostApi, HostApiVTable, LogLevel, MiddlewareCallbackFn,
    MiddlewareHandle, RequestId, ServiceHandle, ServiceMethodFn, ServiceVTable, SubscriptionHandle,
    ToolHandle, ToolHandlerFn, WaitHandle,
};
use serde_json::{Value, json};

/// 一条记录下来的宿主调用。
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// `on(event, filter)`
    Subscribe { event: String },
    /// `call_service(service, method, input)`
    Service {
        service: String,
        method: String,
        input: String,
    },
    /// `register_tool(descriptor)`
    Tool { descriptor: String },
    /// `provide_service_shared(vtable)`（记录指针，方法名在断言时再读）
    Provide { name: String, vtable: usize },
    /// `log(level, message)`
    Log { level: u32, message: String },
    /// `emit(event, payload)`
    Emit { event: String, payload: String },
    /// 其它未归一化的调用。
    Other(String),
}

thread_local! {
    static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
    /// 服务方法返回用的线程局部缓冲（和真宿主一样：读完即弃）。
    static REPLY: RefCell<CString> = RefCell::new(CString::default());
    /// 最近一次 `provide_service_shared` 的 VTable 副本。
    static SERVICE: RefCell<Option<&'static ServiceVTable>> = const { RefCell::new(None) };
}

/// 取出所有记录（按顺序）。
pub fn calls() -> Vec<Call> {
    CALLS.with(|calls| calls.borrow().clone())
}

/// 清空记录。
pub fn reset() {
    CALLS.with(|calls| calls.borrow_mut().clear());
    SERVICE.with(|service| *service.borrow_mut() = None);
}

/// 最近一次注册的共享服务 VTable。
pub fn service_vtable() -> Option<&'static ServiceVTable> {
    SERVICE.with(|service| *service.borrow())
}

/// 按名字找服务方法。
pub fn service_method(vtable: &'static ServiceVTable, name: &str) -> Option<ServiceMethodFn> {
    // from_raw_parts 的空指针 + 0 长度是 UB；没注册方法时直接当成找不到。
    if vtable.methods.is_null() || vtable.method_count == 0 {
        return None;
    }
    // SAFETY: VTable 由插件注册且活着；测试里只读。
    let methods = unsafe { std::slice::from_raw_parts(vtable.methods, vtable.method_count) };
    methods.iter().find_map(|method| {
        // SAFETY: 方法名是插件持有的 C 字符串。
        let method_name = unsafe { CStr::from_ptr(method.name) }.to_string_lossy();
        (method_name == name).then_some(method.func).flatten()
    })
}

fn record(call: Call) {
    CALLS.with(|calls| calls.borrow_mut().push(call));
}

fn reply_text(text: &str) -> *const c_char {
    REPLY.with(|slot| {
        *slot.borrow_mut() = CString::new(text).unwrap_or_default();
        slot.borrow().as_ptr()
    })
}

/// 把 C 字符串读成 Rust 字符串（测试断言用）。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn cstr(value: *const c_char) -> String {
    if value.is_null() {
        return String::new();
    }
    // SAFETY: ABI 契约保证是 NUL 结尾字符串。
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
}

// ---- VTable 桩 -------------------------------------------------------------

unsafe extern "C" fn on(
    _host: *mut c_void,
    event: *const c_char,
    _filter: *const c_char,
    _cb: Option<EventCallbackFn>,
    _ud: *mut c_void,
) -> SubscriptionHandle {
    record(Call::Subscribe { event: cstr(event) });
    SubscriptionHandle { id: 1 }
}

unsafe extern "C" fn off(_host: *mut c_void, _handle: SubscriptionHandle) {}

unsafe extern "C" fn emit(_host: *mut c_void, event: *const c_char, payload: *const c_char) -> i32 {
    record(Call::Emit {
        event: cstr(event),
        payload: cstr(payload),
    });
    0
}

unsafe extern "C" fn register_tool(
    _host: *mut c_void,
    descriptor: *const c_char,
    _handler: Option<ToolHandlerFn>,
    _ud: *mut c_void,
) -> ToolHandle {
    record(Call::Tool {
        descriptor: cstr(descriptor),
    });
    ToolHandle { id: 1 }
}

unsafe extern "C" fn call_tool(
    _host: *mut c_void,
    _name: *const c_char,
    _input: *const c_char,
) -> RequestId {
    RequestId::INVALID
}

unsafe extern "C" fn call_tool_result(
    _host: *mut c_void,
    _name: *const c_char,
    _input: *const c_char,
) -> *const c_char {
    reply_text("{}")
}

unsafe extern "C" fn list_tools(_host: *mut c_void) -> *const c_char {
    reply_text("[]")
}

unsafe extern "C" fn list_plugins(_host: *mut c_void) -> *const c_char {
    reply_text("[]")
}

unsafe extern "C" fn provide(
    _host: *mut c_void,
    name: *const c_char,
    vtable: *const ServiceVTable,
) -> ServiceHandle {
    // SAFETY: 插件把 &'static VTable 交给宿主；测试里只读并记录。
    let vtable: &'static ServiceVTable = unsafe { &*vtable };
    SERVICE.with(|service| *service.borrow_mut() = Some(vtable));
    record(Call::Provide {
        name: cstr(name),
        vtable: vtable as *const ServiceVTable as usize,
    });
    ServiceHandle { id: 1 }
}

unsafe extern "C" fn get_service(_host: *mut c_void, _name: *const c_char) -> *const ServiceVTable {
    std::ptr::null()
}

unsafe extern "C" fn td_invoke(_host: *mut c_void, _method: *const c_char) -> RequestId {
    RequestId::INVALID
}

unsafe extern "C" fn td_invoke_simplified(
    _host: *mut c_void,
    _method: *const c_char,
    _params: *const c_char,
) -> RequestId {
    RequestId { id: 7 }
}

unsafe extern "C" fn td_execute(_host: *mut c_void, _request: *const c_char) -> *const c_char {
    std::ptr::null()
}

unsafe extern "C" fn use_middleware(
    _host: *mut c_void,
    _stage: u32,
    _callback: Option<MiddlewareCallbackFn>,
    _ud: *mut c_void,
) -> MiddlewareHandle {
    MiddlewareHandle::INVALID
}

unsafe extern "C" fn wait_for_update(
    _host: *mut c_void,
    _filter: *const c_char,
    _timeout_ms: u64,
) -> WaitHandle {
    WaitHandle::INVALID
}

unsafe extern "C" fn cancel_wait(_host: *mut c_void, _handle: WaitHandle) {}

unsafe extern "C" fn log(_host: *mut c_void, level: u32, message: *const c_char) {
    record(Call::Log {
        level,
        message: cstr(message),
    });
    let _ = LogLevel::Info;
}

unsafe extern "C" fn get_config(
    _host: *mut c_void,
    _section: *const c_char,
    _key: *const c_char,
) -> *const c_char {
    // 测试里一律读不到配置 → 插件用缺省值。
    std::ptr::null()
}

unsafe extern "C" fn set_config(
    _host: *mut c_void,
    _key: *const c_char,
    _value: *const c_char,
) -> i32 {
    0
}

unsafe extern "C" fn simplify_invoke(_host: *mut c_void, _op: *const c_char) -> RequestId {
    RequestId { id: 8 }
}

unsafe extern "C" fn simplify_invoke_result(
    _host: *mut c_void,
    _op: *const c_char,
) -> *const c_char {
    reply_text(r#"{"@type":"simplifyResult","ok":true}"#)
}

unsafe extern "C" fn current_plugin(_host: *mut c_void) -> *const c_char {
    reply_text("rusto")
}

unsafe extern "C" fn call_service(
    _host: *mut c_void,
    service: *const c_char,
    method: *const c_char,
    input: *const c_char,
) -> *const c_char {
    let service_name = cstr(service);
    let method_name = cstr(method);
    let payload = cstr(input);
    record(Call::Service {
        service: service_name.clone(),
        method: method_name.clone(),
        input: payload,
    });
    match (service_name.as_str(), method_name.as_str()) {
        ("fuyubot.host.commands", "register") => {
            reply_text(r#"{"ok":true,"name":"ocr","plugin":"rusto"}"#)
        }
        ("fuyubot.host.command", "reply") => reply_text(r#"{"id":42,"chat_id":7770001}"#),
        _ => {
            // 未预期的服务调用：回一个带 error 的 JSON，方便断言直接失败。
            reply_text(r#"{"error":"unexpected service call"}"#)
        }
    }
}

unsafe extern "C" fn last_error(_host: *mut c_void) -> *const c_char {
    std::ptr::null()
}

unsafe extern "C" fn list_services(_host: *mut c_void) -> *const c_char {
    reply_text("[]")
}

unsafe extern "C" fn set_config_section(
    _host: *mut c_void,
    _section: *const c_char,
    _key: *const c_char,
    _value: *const c_char,
) -> i32 {
    0
}

/// 造一个假宿主。
pub fn api() -> HostApi {
    let vtable: &'static HostApiVTable = Box::leak(Box::new(HostApiVTable {
        version: ABI_VERSION,
        size: std::mem::size_of::<HostApiVTable>() as u32,
        host_ctx: std::ptr::null_mut(),
        on: Some(on),
        off: Some(off),
        emit: Some(emit),
        register_tool: Some(register_tool),
        call_tool: Some(call_tool),
        call_tool_result: Some(call_tool_result),
        list_tools: Some(list_tools),
        list_plugins: Some(list_plugins),
        provide_service: Some(provide),
        get_service: Some(get_service),
        td_invoke: Some(td_invoke),
        td_invoke_simplified: Some(td_invoke_simplified),
        td_execute: Some(td_execute),
        use_middleware: Some(use_middleware),
        wait_for_update: Some(wait_for_update),
        cancel_wait: Some(cancel_wait),
        log: Some(log),
        get_config: Some(get_config),
        set_config: Some(set_config),
        provide_service_shared: Some(provide),
        simplify_invoke: Some(simplify_invoke),
        simplify_invoke_result: Some(simplify_invoke_result),
        current_plugin: Some(current_plugin),
        call_service: Some(call_service),
        last_error: Some(last_error),
        list_services: Some(list_services),
        set_config_section: Some(set_config_section),
    }));
    // SAFETY: vtable 由 Box::leak 保活，布局与 ABI 一致。
    unsafe { HostApi::from_raw(vtable) }.expect("假宿主必须能被解包")
}

/// 断言辅助：取出某类调用。
pub fn service_calls() -> Vec<(String, String, Value)> {
    calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Service {
                service,
                method,
                input,
            } => Some((
                service,
                method,
                serde_json::from_str(&input).unwrap_or(Value::Null),
            )),
            _ => None,
        })
        .collect()
}

/// 断言辅助：工具描述 JSON。
pub fn tool_descriptor() -> Option<Value> {
    calls().into_iter().find_map(|call| match call {
        Call::Tool { descriptor } => serde_json::from_str(&descriptor).ok(),
        _ => None,
    })
}

#[allow(dead_code)]
fn _unused(_: Value) {
    let _ = json!({});
}

#[cfg(test)]
mod smoke {
    //! ABI 级冒烟：验证 start() 接线与服务回调，不需要真宿主。

    use std::ffi::CString;

    use fuyubot_plugin_api::Plugin;
    use serde_json::Value;

    use super::{Call, cstr, service_calls, service_method, service_vtable, tool_descriptor};
    use crate::Rusto;

    fn invoke(
        method: fuyubot_plugin_api::ServiceMethodFn,
        context: *mut std::ffi::c_void,
        input: &str,
    ) -> Value {
        let payload = CString::new(input).unwrap();
        // SAFETY: 回调签名与 ABI 一致；context 来自插件注册的 VTable。
        let raw = unsafe { method(context, payload.as_ptr()) };
        serde_json::from_str(&cstr(raw)).unwrap_or(Value::Null)
    }

    #[test]
    fn start_wires_subscription_command_service_and_tool() {
        super::reset();
        let mut plugin = Rusto::init(super::api()).expect("init 不该失败");
        plugin.start("{}").expect("start 不该失败");

        let calls = super::calls();
        assert!(
            calls.iter().any(
                |call| matches!(call, Call::Subscribe { event } if event == "updateNewMessage")
            ),
            "必须订阅 updateNewMessage: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|call| matches!(call, Call::Provide { .. })),
            "必须注册共享服务: {calls:?}"
        );
        assert!(tool_descriptor().is_some(), "必须注册工具: {calls:?}");

        let registered = service_calls().iter().any(|(service, method, input)| {
            service == "fuyubot.host.commands"
                && method == "register"
                && input.get("name").and_then(Value::as_str) == Some("ocr")
        });
        assert!(registered, "/ocr 必须登记进宿主命令表");

        let vtable = service_vtable().expect("VTable 必须被登记");
        assert_eq!(vtable.version, 1);
        // SAFETY: name 是插件持有的 C 字符串。
        let name = unsafe { std::ffi::CStr::from_ptr(vtable.name) };
        assert_eq!(name.to_string_lossy(), "rusto.ocr");

        plugin.stop();
    }

    #[test]
    fn service_methods_answer_json_and_never_panic() {
        super::reset();
        let mut plugin = Rusto::init(super::api()).expect("init");
        plugin.start("{}").expect("start");
        let vtable = service_vtable().expect("VTable");

        let status = invoke(
            service_method(vtable, "status").expect("status"),
            vtable.context,
            "{}",
        );
        assert_eq!(status["ok"], true, "status 必须是成功应答: {status}");

        let recognize = service_method(vtable, "recognize").expect("recognize");
        let missing = invoke(
            recognize,
            vtable.context,
            "{\"path\":\"definitely-missing.png\"}",
        );
        assert_eq!(missing["ok"], false, "缺模型/缺图必须是失败应答");
        assert!(
            missing["code"].as_str().is_some(),
            "失败必须带错误码: {missing}"
        );

        let bad = invoke(recognize, vtable.context, "not json");
        assert_eq!(bad["code"], "bad_input");

        // 不在冒烟里调 warm：模型缺失时它会触发联网下载，冒烟测试必须保持离线。
        // warm 的行为由 engine::models / rusto_backend 的单测覆盖。

        plugin.stop();
    }

    #[test]
    fn tool_descriptor_is_a_valid_schema() {
        super::reset();
        let mut plugin = Rusto::init(super::api()).expect("init");
        plugin.start("{}").expect("start");
        let descriptor = tool_descriptor().expect("工具描述");
        assert_eq!(descriptor["name"], "rusto.ocr");
        assert_eq!(descriptor["input_schema"]["type"], "object");
        assert!(descriptor["timeout_ms"].as_u64().unwrap_or(0) >= 10_000);
        plugin.stop();
    }
}
