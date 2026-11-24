//! 对外接口：工具（`fengyegf.agent.*`）与共享服务（`fengyegf.agent`）。
//!
//! 为什么工具和服务共用一份 `execute`：工具给 AI / 宿主调，服务给别的插件调，两边要是
//! 各写一份 JSON 契约，迟早会漂移。这里只写一次，入口只差拿到状态指针的方式
//! （工具的 `user_data` / 服务的 `context` 都指向同一个 [`PluginState`]）。
//!
//! ## 工具在哪
//!
//! 每个工具一个文件，放在 [`tools`] 下：`chat.rs` / `run.rs` / `send.rs` /
//! `session.rs` / `title.rs` / `result.rs`。每个模块自带 `NAME` / `DESCRIPTION` / `schema()` / `handle()`，
//! 本文件只负责注册（把三者交给宿主）、按方法名分发和 FFI 边界。
//!
//! ## 统一契约
//!
//! 成功：`{"ok":true,…}`；失败：`{"ok":false,"code":"…","error":"…"}`
//! （code 来自 [`AgentError::code`]，panic 是 `"panic"`，状态没初始化是 `"state"`）。
//!
//! ## 为什么每个入口都套 catch_unwind
//!
//! 这些函数是 `extern "C"` / `extern "C-unwind"` 的 FFI 边界：Rust 的 panic 一旦穿过去
//! 就是未定义行为（或直接把宿主进程 abort）。工具入参来自外部（AI / 别的插件），任何
//! 一个 `unwrap` 都不该有机会炸掉宿主，所以统一在 `guarded` 里拦下，转成 JSON 错误。
//!
//! ## 阻塞调用
//!
//! `chat` / `run` / `send` 会走 LLM 网关与 TDLib，都是**同步阻塞**的；它们只允许在
//! 宿主调工具 / 服务的线程里执行（ABI 上工具与服务本来就是同步调用），不要拿去订阅回调里跑。

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

use fuyubot_plugin_api::{
    HostApi, HostExt, ServiceMethod, ServiceVTable, ToolHandle, ToolHandlerFn,
};
use serde_json::{Value, json};

use crate::PluginState;
use crate::agent;
use crate::core::{AgentError, AgentResult, consts};

mod tools;

/// 共享服务名。
pub const SERVICE_NAME: &str = "fengyegf.agent";
/// 服务方法名（顺序与 VTable 里的 methods 一致）。
const SERVICE_METHODS: [&str; 9] = [
    "describe",
    "chat",
    "run",
    "send",
    "session",
    "title",
    "register",
    "unregister",
    "registry",
];

/// 注册工具；返回句柄（无效句柄已被宿主拒绝，这里照收）。
pub fn register_tools(host: &HostApi, state: *const PluginState) -> Vec<ToolHandle> {
    let user_data = state.cast_mut().cast::<c_void>();
    // 顺序 = 注册顺序；名字 / 描述 / schema 都在各自的模块里。
    let tools: [(&str, &str, Value, ToolHandlerFn); 6] = [
        (
            tools::chat::NAME,
            tools::chat::DESCRIPTION,
            tools::chat::schema(),
            tool_chat,
        ),
        (
            tools::run::NAME,
            tools::run::DESCRIPTION,
            tools::run::schema(),
            tool_run,
        ),
        (
            tools::send::NAME,
            tools::send::DESCRIPTION,
            tools::send::schema(),
            tool_send,
        ),
        (
            tools::session::NAME,
            tools::session::DESCRIPTION,
            tools::session::schema(),
            tool_session,
        ),
        (
            tools::title::NAME,
            tools::title::DESCRIPTION,
            tools::title::schema(),
            tool_title,
        ),
        (
            tools::result::NAME,
            tools::result::DESCRIPTION,
            tools::result::schema(),
            tool_result,
        ),
    ];

    let mut handles = Vec::with_capacity(tools.len());
    for (name, description, schema, handler) in tools {
        let descriptor = json!({
            "name": name,
            "version": env!("CARGO_PKG_VERSION"),
            "description": description,
            "input_schema": schema,
            "output_schema": {},
            // 网关 + TDLib 都是同步阻塞调用，给足超时。
            "timeout_ms": 180_000,
            "idempotent": false,
        });
        let handle = host.register_tool(&descriptor.to_string(), Some(handler), user_data);
        if handle.is_valid() {
            handles.push(handle);
        } else {
            host.warn(&format!("agent: 注册工具 {name} 失败"));
        }
    }
    handles
}

/// 共享服务：VTable 以及它引用的所有堆缓冲必须同生共死。
///
/// # Safety
///
/// 裸指针只指向自己拥有的 `names` / `methods` 堆缓冲与 `vtable`（Box 固定地址），
/// 创建后不再修改；`context` 指向插件持有的 `PluginState`，生命周期覆盖整个
/// `start` → `stop` 区间。`Plugin` 要求 `Send` 只是允许宿主移动插件状态，
/// 服务方法仍由宿主在调用方线程同步执行。
pub struct AgentService {
    #[allow(dead_code)]
    names: Vec<CString>,
    #[allow(dead_code)]
    methods: Vec<ServiceMethod>,
    vtable: Box<ServiceVTable>,
}

// SAFETY: 见 `AgentService` 的安全说明；裸指针指向的缓冲由本结构独占且地址稳定。
unsafe impl Send for AgentService {}

impl AgentService {
    /// 建服务（context = 插件状态指针）。
    pub fn new(state: *const PluginState) -> Self {
        // names[0] 是服务名，后面依次是方法名；VTable 的指针都指向这些堆缓冲。
        let names: Vec<CString> = std::iter::once(SERVICE_NAME)
            .chain(SERVICE_METHODS)
            .map(|name| CString::new(name).expect("静态服务名合法"))
            .collect();
        let funcs: [unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char; 9] = [
            svc_describe,
            svc_chat,
            svc_run,
            svc_send,
            svc_session,
            svc_title,
            svc_register,
            svc_unregister,
            svc_registry,
        ];
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
            context: state.cast_mut().cast::<c_void>(),
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

// ── 方法实现 ────────────────────────────────────────────────────────

/// 工具 / 服务共用的分发：成功给 JSON，失败给 [`AgentError::to_json`]。
fn execute(state: &PluginState, method: &str, input: &Value) -> Value {
    let result = match method {
        "describe" => Ok(describe(state)),
        "chat" => tools::chat::handle(state, input),
        "run" => tools::run::handle(state, input),
        "send" => tools::send::handle(state, input),
        "session" => tools::session::handle(state, input),
        "title" => tools::title::handle(state, input),
        "result" => tools::result::handle(state, input),
        // 能力注册表：提供方在自己的 start() 里调一次 register 即可。
        "register" => register_capability(state, input),
        "unregister" => unregister_capability(state, input),
        "registry" => Ok(registry_snapshot(state)),
        other => Err(AgentError::BadRequest(format!("未知方法：{other}"))),
    };
    match result {
        Ok(value) => value,
        Err(error) => error.to_json(),
    }
}

/// 自描述：给 AI / 面板 / 诊断用的能力与配置快照（不发网络请求、不碰 TDLib）。
/// register：提供方把自己的能力登记进 agent 的能力注册表。
///
/// 入参见 agent::registry 的模块文档；owner 由宿主按调用方判定（ABI v6 的
/// call_service 会带调用者身份），agent 只记录、不替谁站台。
fn register_capability(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let registration = agent::Registration::from_value(input)
        .map_err(AgentError::BadRequest)?
        .with_owner(state.host.current_plugin());
    state
        .registry
        .register(registration.clone())
        .map_err(AgentError::BadRequest)?;
    Ok(json!({
        "ok": true,
        "name": registration.name,
        "service": registration.service,
        "capability": registration.capability,
        "owner": registration.owner,
    }))
}

/// unregister：注销自己登记的能力（只有登记方能注销）。
fn unregister_capability(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let name = input
        .get("name")
        .or_else(|| input.get("capability"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| AgentError::BadRequest("unregister 需要 name".to_owned()))?;
    let removed = state
        .registry
        .unregister(
            name,
            state.host.current_plugin().as_deref(),
            crate::PLUGIN_ID,
        )
        .map_err(AgentError::BadRequest)?;
    Ok(json!({ "ok": true, "name": name, "removed": removed }))
}

/// registry：当前注册表快照（诊断 / 其它插件自检用）。
fn registry_snapshot(state: &PluginState) -> Value {
    let capabilities: Vec<Value> = state
        .registry
        .snapshot()
        .iter()
        .map(agent::Registration::to_json)
        .collect();
    json!({ "ok": true, "count": capabilities.len(), "capabilities": capabilities })
}

fn describe(state: &PluginState) -> Value {
    let config = &state.config;
    let artifacts = state.artifacts.stats();
    json!({
        "ok": true,
        "plugin": crate::PLUGIN_ID,
        "version": env!("CARGO_PKG_VERSION"),
        "gateway": {
            "service": consts::GATEWAY_SERVICE,
            "method": config.gateway_method,
            "available": gateway_available(state),
        },
        "config": {
            // ReplyMode 的 Serialize 就是 plain / markdown / rich。
            "reply_mode": config.reply_mode,
            "model": config.model,
            // 历史默认全量携带；上下文用 /agent compress 压成摘要。
            "history": "full",
            "compression": true,
            "merge_window_ms": consts::MESSAGE_MERGE_MS,
            "allow_bots": config.allow_bots,
            "session_dir": config.session_path().display().to_string(),
            "stream_replies": config.stream_replies,
            // 工具：本次会下发给模型多少个（宿主工具表现查，可能随后续插件变化）。
            "tool_count": state.engine.tool_count(),
            "tool_names": state.engine.tool_names(),
            // 走插件共享服务（describe / invoke）的能力数；0 = 没装提供方或没人登记服务。
            "service_tool_count": state.engine.service_tool_count(),
            // 插件注册进 agent 的能力数 / 名字（工具目录每轮读它）。
            "registry": {
                "count": state.engine.registry_count(),
                "capabilities": state
                    .registry
                    .snapshot()
                    .iter()
                    .map(|item| item.name.clone())
                    .collect::<Vec<_>>(),
            },
            "tool_denylist": config.tool_denylist,
            // 内置屏蔽：永远不下发给模型的通用大接口（不是配置能解除的）。
            "tool_reserved": crate::agent::RESERVED_TOOLS,
        },
        "sessions": state.store.count().unwrap_or(0),
        "artifacts": { "count": artifacts.count, "bytes": artifacts.bytes },
    })
}

/// 网关服务当前是否在场。
///
/// 直接问宿主的服务清单，而不是复用 agent 层的 [`crate::agent::GatewayClient::available`]：
/// `describe` 是诊断入口，答案不能依赖另一层是否已经初始化好。
fn gateway_available(state: &PluginState) -> bool {
    let listing = state.host.list_services();
    let Ok(services) = serde_json::from_str::<Value>(&listing) else {
        return false;
    };
    services.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("name").and_then(Value::as_str) == Some(consts::GATEWAY_SERVICE))
    })
}

// ── FFI 边界 ────────────────────────────────────────────────────────

thread_local! {
    /// 工具 / 服务返回值：宿主调完立刻拷走，下一次调用前有效。
    static REPLY: RefCell<CString> = RefCell::new(CString::new("{}").expect("静态 JSON 合法"));
}

fn reply_json(value: &Value) -> *const c_char {
    let text = serde_json::to_string(value)
        .unwrap_or_else(|_| r#"{"ok":false,"code":"json","error":"返回值序列化失败"}"#.to_owned());
    // 交给 C 的字符串不能含内嵌 NUL。
    let text = text.replace('\0', "");
    let cstring = CString::new(text).unwrap_or_else(|_| {
        CString::new(r#"{"ok":false,"code":"json","error":"返回值含 NUL"}"#)
            .expect("静态 JSON 合法")
    });
    REPLY.with(|slot| {
        *slot.borrow_mut() = cstring;
        slot.borrow().as_ptr()
    })
}

/// 任何 panic 都在这里变成 `{"ok":false,"code":"panic"}`，绝不穿出 FFI。
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

fn input_value(input: *const c_char) -> AgentResult<Value> {
    if input.is_null() {
        return Ok(json!({}));
    }
    // SAFETY: 宿主按 ABI 契约传 NUL 结尾的 UTF-8 JSON。
    let text = unsafe { CStr::from_ptr(input) }
        .to_str()
        .map_err(|error| AgentError::BadRequest(format!("入参不是 UTF-8：{error}")))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(text)
        .map_err(|error| AgentError::BadRequest(format!("入参不是 JSON：{error}")))
}

/// 工具 / 服务共用入口：`user_data` 与 `context` 都指向 [`PluginState`]。
fn entry(state: *mut c_void, input: *const c_char, method: &'static str) -> *const c_char {
    guarded(move || {
        // SAFETY: 指针是插件自己 Box 持有的 PluginState；AgentPlugin 的字段顺序保证
        // 工具 / 服务先于 state 释放。
        let Some(state) = (unsafe { (state as *const PluginState).as_ref() }) else {
            return json!({ "ok": false, "code": "state", "error": "插件状态未初始化" });
        };
        match input_value(input) {
            Ok(value) => execute(state, method, &value),
            Err(error) => error.to_json(),
        }
    })
}

unsafe extern "C-unwind" fn tool_chat(user: *mut c_void, input: *const c_char) -> *const c_char {
    entry(user, input, "chat")
}

unsafe extern "C-unwind" fn tool_run(user: *mut c_void, input: *const c_char) -> *const c_char {
    entry(user, input, "run")
}

unsafe extern "C-unwind" fn tool_send(user: *mut c_void, input: *const c_char) -> *const c_char {
    entry(user, input, "send")
}

unsafe extern "C-unwind" fn tool_session(user: *mut c_void, input: *const c_char) -> *const c_char {
    entry(user, input, "session")
}

unsafe extern "C-unwind" fn tool_title(user: *mut c_void, input: *const c_char) -> *const c_char {
    entry(user, input, "title")
}

unsafe extern "C-unwind" fn tool_result(user: *mut c_void, input: *const c_char) -> *const c_char {
    entry(user, input, "result")
}

unsafe extern "C" fn svc_describe(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "describe")
}

unsafe extern "C" fn svc_chat(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "chat")
}

unsafe extern "C" fn svc_run(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "run")
}

unsafe extern "C" fn svc_send(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "send")
}

unsafe extern "C" fn svc_session(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "session")
}

unsafe extern "C" fn svc_title(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "title")
}

unsafe extern "C" fn svc_register(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "register")
}

unsafe extern "C" fn svc_unregister(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "unregister")
}

unsafe extern "C" fn svc_registry(context: *mut c_void, input: *const c_char) -> *const c_char {
    entry(context, input, "registry")
}
