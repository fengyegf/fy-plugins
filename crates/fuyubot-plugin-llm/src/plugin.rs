//! 宿主插件壳：把三个协议能力暴露成 **工具** 与 **共享服务**，供宿主 / AI / 其它插件调用。
//!
//! - 工具（`register_tool`）：`fengyegf.llm.chat` / `.responses` / `.messages` / `.models` /
//!   `.test` / `.describe`；
//! - 服务（`provide_service_shared`）：`fengyegf.llm`，方法同上（`chat` / `responses` /
//!   `messages` / `models` / `test` / `describe`）。
//!
//! 插件本身不解释协议字段：入参就是对应协议的原生 JSON body，原样经 [`crate::api::LlmClient`]
//! 发给上游。这样协议模块新增字段时插件壳不用跟着改。
//!
//! ## 入参 / 出参
//!
//! 工具与服务的调用参数都是 JSON 对象：
//!
//! ```json
//! { "model": "deepseek-chat", "messages": [{ "role": "user", "content": "hi" }] }
//! ```
//!
//! 成功：`{"ok":true,"protocol":"openai-chat","data":{…上游原始响应…}}`
//! 失败：`{"ok":false,"kind":"api","status":401,"message":"…"}`
//!
//! 插件配置（`[plugin.fengyegf.llm]`）里没带 `model` / `max_tokens` 时，
//! 会用 `config.default_model` / `default_max_tokens` 补齐。

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};

use fuyubot_plugin_api::{
    HostApi, HostExt, Plugin, ServiceMethod, ServiceVTable, ToolHandle, define_plugin,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{LlmClient, Protocol};
use crate::core::{ClientConfig, LlmError, LlmResult};

/// 插件 id（与 `plugin.json` 一致）。
pub const PLUGIN_ID: &str = "fengyegf.llm";
/// 对外共享服务名。
pub const SERVICE_NAME: &str = "fengyegf.llm";
/// 工具名前缀。
pub const TOOL_PREFIX: &str = "fengyegf.llm";

/// 工具名。
pub const TOOL_CHAT: &str = "fengyegf.llm.chat";
/// 工具名。
pub const TOOL_RESPONSES: &str = "fengyegf.llm.responses";
/// 工具名。
pub const TOOL_MESSAGES: &str = "fengyegf.llm.messages";
/// 工具名。
pub const TOOL_MODELS: &str = "fengyegf.llm.models";
/// 工具名。
pub const TOOL_TEST: &str = "fengyegf.llm.test";
/// 工具名。
pub const TOOL_DESCRIBE: &str = "fengyegf.llm.describe";

/// 插件配置：传输配置 + 插件默认值。
///
/// 传输相关字段直接平铺在 `[plugin.fengyegf.llm]` 下（与 `config.default.toml` 一致）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    /// 三个协议共享的传输配置（`base_url` / `api_key` / 超时 / 重试 / 路径）。
    #[serde(flatten)]
    pub client: ClientConfig,
    /// 请求没带 `model` 时的默认模型。
    pub default_model: String,
    /// 请求没带 max tokens 时的默认上限。
    pub default_max_tokens: u32,
    /// 是否注册工具（供宿主 / AI 调用）。
    pub enable_tools: bool,
    /// 是否注册共享服务（供其它插件调用）。
    pub enable_service: bool,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            client: ClientConfig::default(),
            default_model: "deepseek-chat".to_owned(),
            default_max_tokens: 1024,
            enable_tools: true,
            enable_service: true,
        }
    }
}

impl PluginConfig {
    /// 从宿主配置段读：宿主已经把 `config.default.toml`、运行时落盘和用户配置合并好。
    ///
    /// 注意：`HostExt::config_value` 的**空 section 读的是顶层**（`[bot]` / `[telegram]`），
    /// 插件自己的段要用插件 id 作为 section（与 whois 等插件一致）。
    pub fn load(host: &HostApi) -> Self {
        let mut config = match host.config_value(PLUGIN_ID, "") {
            Some(value) => match serde_json::from_value::<Self>(value) {
                Ok(config) => config,
                Err(error) => {
                    host.warn(&format!("llm: 配置段解析失败，使用默认值：{error}"));
                    Self::default()
                }
            },
            None => Self::default(),
        };

        // 部署现场常用环境变量放 Key：配置里没写才回退，配置优先。
        let env = ClientConfig::from_env();
        if !config.client.has_api_key() && env.has_api_key() {
            config.client.api_key = env.api_key;
        }
        if config.client.base_url.trim().is_empty() {
            config.client.base_url = env.base_url;
        }
        if config.default_model.trim().is_empty() {
            config.default_model = "deepseek-chat".to_owned();
        }
        config
    }

    /// 从 JSON 解析（测试 / 服务方法复用）。
    pub fn from_json(value: &Value) -> LlmResult<Self> {
        serde_json::from_value(value.clone())
            .map_err(|error| LlmError::Config(format!("插件配置不合法：{error}")))
    }
}

/// 插件运行期状态：配置 + 协议客户端。
///
/// 生命周期由 `Box<PluginState>` 保证；工具 / 服务的 `user_data` 只存指向它的裸指针。
pub struct PluginState {
    /// 解析后的插件配置。
    pub config: PluginConfig,
    /// 统一协议客户端。
    pub client: LlmClient,
}

impl PluginState {
    /// 构造客户端（会校验 `base_url`）。
    pub fn new(config: PluginConfig) -> LlmResult<Self> {
        let client = LlmClient::new(config.client.clone())?;
        Ok(Self { config, client })
    }

    /// 原始 JSON 直通：补齐默认 model / max_tokens 后发给指定协议。
    pub fn dispatch(&self, protocol: Protocol, body: &Value) -> Value {
        let prepared = match self.prepare(protocol, body) {
            Ok(value) => value,
            Err(error) => return error.to_json(),
        };
        match self.client.request_json(protocol, &prepared) {
            Ok(value) => json!({
                "ok": true,
                "protocol": protocol.as_str(),
                "data": value,
            }),
            Err(error) => error.to_json(),
        }
    }

    /// 启动时 / 运行中的自检。
    pub fn test(&self, model: Option<&str>) -> Value {
        let model = model
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or(&self.config.default_model);
        let report = self.client.test(model);
        json!({ "ok": report.ok, "report": report })
    }

    /// 模型列表（OpenAI 风格）。
    pub fn models(&self) -> Value {
        match self.client.models() {
            Ok(value) => json!({ "ok": true, "data": value }),
            Err(error) => error.to_json(),
        }
    }

    /// 自描述：给 AI Agent / 面板看的能力清单（不发网络请求）。
    pub fn describe(&self) -> Value {
        let mut value = self.client.describe();
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "default_model".to_owned(),
                Value::String(self.config.default_model.clone()),
            );
            object.insert(
                "default_max_tokens".to_owned(),
                Value::Number(self.config.default_max_tokens.into()),
            );
            object.insert(
                "tools".to_owned(),
                json!([
                    TOOL_CHAT,
                    TOOL_RESPONSES,
                    TOOL_MESSAGES,
                    TOOL_MODELS,
                    TOOL_TEST,
                    TOOL_DESCRIBE,
                ]),
            );
            object.insert("service".to_owned(), Value::String(SERVICE_NAME.to_owned()));
            object.insert(
                "service_methods".to_owned(),
                json!([
                    "chat",
                    "responses",
                    "messages",
                    "models",
                    "test",
                    "describe"
                ]),
            );
        }
        value
    }

    fn prepare(&self, protocol: Protocol, body: &Value) -> LlmResult<Value> {
        if !self.config.client.has_api_key() {
            return Err(LlmError::Config(
                "未配置 api_key：请在 [plugin.fengyegf.llm] 的 api_key 或环境变量 FUYUBOT_LLM_API_KEY 中设置"
                    .to_owned(),
            ));
        }
        let Some(object) = body.as_object() else {
            return Err(LlmError::Other("请求体必须是 JSON 对象".to_owned()));
        };
        if object.get("stream").and_then(Value::as_bool) == Some(true) {
            return Err(LlmError::Other(
                "工具 / 服务是同步 JSON 调用，不支持 stream:true；流式请用 Rust 库 API 或 probe --stream"
                    .to_owned(),
            ));
        }
        let mut object = object.clone();

        // model：调用方优先，其次插件默认值。
        match object.get("model").and_then(Value::as_str) {
            Some(model) if !model.trim().is_empty() => {}
            _ => {
                if self.config.default_model.trim().is_empty() {
                    return Err(LlmError::Config(
                        "请求缺少 model，且没有默认模型".to_owned(),
                    ));
                }
                object.insert(
                    "model".to_owned(),
                    Value::String(self.config.default_model.clone()),
                );
            }
        }

        // max tokens：不同协议字段名不同；不覆盖调用方显式值。
        let default_max = u64::from(self.config.default_max_tokens.max(1));
        match protocol {
            Protocol::OpenAiChat => {
                if object.get("max_tokens").is_none()
                    && object.get("max_completion_tokens").is_none()
                {
                    object.insert("max_tokens".to_owned(), Value::from(default_max));
                }
            }
            Protocol::OpenAiResponses => {
                if object.get("max_output_tokens").is_none() {
                    object.insert("max_output_tokens".to_owned(), Value::from(default_max));
                }
            }
            Protocol::AnthropicMessages => {
                if object.get("max_tokens").is_none() {
                    object.insert("max_tokens".to_owned(), Value::from(default_max));
                }
            }
        }

        Ok(Value::Object(object))
    }
}

/// 工具 / 服务共用的方法分发。
fn execute(state: &PluginState, method: &str, input: &Value) -> Value {
    match method {
        "chat" => state.dispatch(Protocol::OpenAiChat, input),
        "responses" => state.dispatch(Protocol::OpenAiResponses, input),
        "messages" => state.dispatch(Protocol::AnthropicMessages, input),
        "models" => state.models(),
        "test" => {
            let model = input.get("model").and_then(Value::as_str);
            state.test(model)
        }
        "describe" => state.describe(),
        other => LlmError::Other(format!("未知方法：{other}")).to_json(),
    }
}

// ── 工具处理器 ──────────────────────────────────────────────────────

thread_local! {
    /// 工具 / 服务返回值：宿主调完立刻拷走，下一次调用前有效。
    static REPLY: RefCell<CString> = RefCell::new(CString::new("{}").expect("静态 JSON 合法"));
}

fn reply_json(value: &Value) -> *const c_char {
    let text = serde_json::to_string(value).unwrap_or_else(|_| {
        r#"{"ok":false,"kind":"json","message":"返回值序列化失败"}"#.to_owned()
    });
    let text = text.replace('\0', "");
    let cstring = CString::new(text).unwrap_or_else(|_| CString::new("{}").expect("合法"));
    REPLY.with(|slot| {
        *slot.borrow_mut() = cstring;
        slot.borrow().as_ptr()
    })
}

fn input_value(input: *const c_char) -> LlmResult<Value> {
    if input.is_null() {
        return Ok(json!({}));
    }
    // SAFETY: 宿主按 ABI 契约传 NUL 结尾的 UTF-8 JSON。
    let text = unsafe { CStr::from_ptr(input) }
        .to_str()
        .map_err(|error| LlmError::Json(format!("入参不是 UTF-8：{error}")))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(text).map_err(|error| LlmError::Json(format!("入参不是 JSON：{error}")))
}

fn guarded(handler: impl FnOnce() -> Value) -> *const c_char {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(handler)) {
        Ok(value) => reply_json(&value),
        Err(_) => reply_json(&json!({
            "ok": false,
            "kind": "panic",
            "message": "工具 / 服务处理器 panic（已拦截，未穿出 ABI）",
        })),
    }
}

fn tool_entry(user_data: *mut c_void, input: *const c_char, method: &'static str) -> *const c_char {
    guarded(move || {
        let Some(state) = (unsafe { (user_data as *const PluginState).as_ref() }) else {
            return json!({ "ok": false, "kind": "state", "message": "插件状态未初始化" });
        };
        match input_value(input) {
            Ok(value) => execute(state, method, &value),
            Err(error) => error.to_json(),
        }
    })
}

unsafe extern "C-unwind" fn tool_chat(user: *mut c_void, input: *const c_char) -> *const c_char {
    tool_entry(user, input, "chat")
}

unsafe extern "C-unwind" fn tool_responses(
    user: *mut c_void,
    input: *const c_char,
) -> *const c_char {
    tool_entry(user, input, "responses")
}

unsafe extern "C-unwind" fn tool_messages(
    user: *mut c_void,
    input: *const c_char,
) -> *const c_char {
    tool_entry(user, input, "messages")
}

unsafe extern "C-unwind" fn tool_models(user: *mut c_void, input: *const c_char) -> *const c_char {
    tool_entry(user, input, "models")
}

unsafe extern "C-unwind" fn tool_test(user: *mut c_void, input: *const c_char) -> *const c_char {
    tool_entry(user, input, "test")
}

unsafe extern "C-unwind" fn tool_describe(
    user: *mut c_void,
    input: *const c_char,
) -> *const c_char {
    tool_entry(user, input, "describe")
}

// ── 共享服务 ────────────────────────────────────────────────────────

/// 服务注册持有者：VTable 以及它引用的所有堆缓冲必须同生共死。
///
/// # Safety
///
/// 结构里的裸指针只指向自己拥有的 `names` / `methods` 堆缓冲，创建后不再修改；
/// `Plugin` 要求 `Send` 只是允许宿主在加载 / 卸载时移动插件状态，服务方法本身
/// 仍由宿主在调用方线程同步执行。`context` 指向同样由插件拥有的 `PluginState`，
/// 生命周期覆盖整个 `start` → `stop` 区间，因此跨线程移动是安全的。
pub struct GatewayService {
    // 服务名、方法名；VTable 的裸指针指向这些 CString 的堆缓冲。
    #[allow(dead_code)]
    names: Vec<CString>,
    // 方法数组；VTable 的 `methods` 指向它。
    #[allow(dead_code)]
    methods: Vec<ServiceMethod>,
    // 用 Box 固定地址，provide_service_shared 存的是这个指针。
    vtable: Box<ServiceVTable>,
}

// SAFETY: 见 `GatewayService` 的安全说明；裸指针指向的缓冲由本结构独占并保持地址稳定。
unsafe impl Send for GatewayService {}

impl GatewayService {
    /// 构造服务表；`state` 会作为每个方法的 `context` 原样传回。
    pub fn new(state: *mut c_void) -> Self {
        let names: Vec<CString> = [
            SERVICE_NAME,
            "chat",
            "responses",
            "messages",
            "models",
            "test",
            "describe",
        ]
        .iter()
        .map(|name| CString::new(*name).expect("静态服务名合法"))
        .collect();

        let methods = vec![
            ServiceMethod {
                name: names[1].as_ptr(),
                func: Some(svc_chat),
            },
            ServiceMethod {
                name: names[2].as_ptr(),
                func: Some(svc_responses),
            },
            ServiceMethod {
                name: names[3].as_ptr(),
                func: Some(svc_messages),
            },
            ServiceMethod {
                name: names[4].as_ptr(),
                func: Some(svc_models),
            },
            ServiceMethod {
                name: names[5].as_ptr(),
                func: Some(svc_test),
            },
            ServiceMethod {
                name: names[6].as_ptr(),
                func: Some(svc_describe),
            },
        ];

        let vtable = Box::new(ServiceVTable {
            name: names[0].as_ptr(),
            version: 1,
            size: std::mem::size_of::<ServiceVTable>() as u32,
            context: state,
            methods: methods.as_ptr(),
            method_count: methods.len(),
        });

        Self {
            names,
            methods,
            vtable,
        }
    }

    /// 传给宿主的 VTable 指针。
    pub fn vtable_ptr(&self) -> *const ServiceVTable {
        &*self.vtable
    }
}

fn service_entry(
    context: *mut c_void,
    input: *const c_char,
    method: &'static str,
) -> *const c_char {
    guarded(move || {
        let Some(state) = (unsafe { (context as *const PluginState).as_ref() }) else {
            return json!({ "ok": false, "kind": "state", "message": "插件状态未初始化" });
        };
        match input_value(input) {
            Ok(value) => execute(state, method, &value),
            Err(error) => error.to_json(),
        }
    })
}

unsafe extern "C" fn svc_chat(context: *mut c_void, input: *const c_char) -> *const c_char {
    service_entry(context, input, "chat")
}

unsafe extern "C" fn svc_responses(context: *mut c_void, input: *const c_char) -> *const c_char {
    service_entry(context, input, "responses")
}

unsafe extern "C" fn svc_messages(context: *mut c_void, input: *const c_char) -> *const c_char {
    service_entry(context, input, "messages")
}

unsafe extern "C" fn svc_models(context: *mut c_void, input: *const c_char) -> *const c_char {
    service_entry(context, input, "models")
}

unsafe extern "C" fn svc_test(context: *mut c_void, input: *const c_char) -> *const c_char {
    service_entry(context, input, "test")
}

unsafe extern "C" fn svc_describe(context: *mut c_void, input: *const c_char) -> *const c_char {
    service_entry(context, input, "describe")
}

// ── 插件主体 ────────────────────────────────────────────────────────

struct LlmPlugin {
    host: HostApi,
    state: Option<Box<PluginState>>,
    service: Option<GatewayService>,
    tools: Vec<ToolHandle>,
}

impl Plugin for LlmPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        Ok(Self {
            host,
            state: None,
            service: None,
            tools: Vec::new(),
        })
    }

    fn start(&mut self, _config_json: &str) -> Result<(), String> {
        let config = PluginConfig::load(&self.host);
        if !config.client.has_api_key() {
            self.host.warn(
                "llm: 尚未配置 api_key，工具会返回配置错误；请在 [plugin.fengyegf.llm] 或环境变量里设置",
            );
        }

        let state = Box::new(PluginState::new(config).map_err(|error| error.to_string())?);
        let state_ptr = (&*state as *const PluginState).cast_mut().cast::<c_void>();

        if state.config.enable_tools {
            self.register_tools(state_ptr);
        }
        if state.config.enable_service {
            let service = GatewayService::new(state_ptr);
            let handle = self
                .host
                .provide_service_shared(SERVICE_NAME, service.vtable_ptr());
            if handle.is_valid() {
                self.service = Some(service);
            } else {
                self.host.warn("llm: 注册共享服务失败（同名服务已存在？）");
            }
        }

        self.state = Some(state);
        self.host.info(&format!(
            "llm: 已启动（{}，{} 个工具，服务 {}）",
            self.host_config_summary(),
            self.tools.len(),
            if self.service.is_some() {
                "已注册"
            } else {
                "未注册"
            },
        ));
        Ok(())
    }

    fn stop(&mut self) {
        // 先撤服务（VTable 指向 state），再释放 state。
        self.service = None;
        self.tools.clear();
        self.state = None;
        self.host.info("llm: 已停止");
    }

    fn display_name() -> &'static str {
        "LLM Gateway"
    }

    fn description() -> &'static str {
        "AI Agent 大模型协议网关：OpenAI Chat Completions / Responses / Anthropic Messages"
    }
}

impl LlmPlugin {
    fn host_config_summary(&self) -> String {
        match &self.state {
            Some(state) => format!(
                "{} · model={}",
                state.config.client.base_url, state.config.default_model
            ),
            None => "未初始化".to_owned(),
        }
    }

    fn register_tools(&mut self, state_ptr: *mut c_void) {
        let tools: [(&str, &str, Value, Option<fuyubot_plugin_api::ToolHandlerFn>); 6] = [
            (
                TOOL_CHAT,
                "OpenAI Chat Completions 原始调用：入参是 /chat/completions 的原生 JSON body。",
                json!({
                    "type": "object",
                    "properties": {
                        "model": { "type": "string" },
                        "messages": { "type": "array" }
                    },
                    "additionalProperties": true
                }),
                Some(tool_chat),
            ),
            (
                TOOL_RESPONSES,
                "OpenAI Responses 原始调用：入参是 /responses 的原生 JSON body。",
                json!({
                    "type": "object",
                    "properties": {
                        "model": { "type": "string" },
                        "input": {}
                    },
                    "additionalProperties": true
                }),
                Some(tool_responses),
            ),
            (
                TOOL_MESSAGES,
                "Anthropic Messages 原始调用：入参是 /messages 的原生 JSON body（max_tokens 会按默认值补齐）。",
                json!({
                    "type": "object",
                    "properties": {
                        "model": { "type": "string" },
                        "messages": { "type": "array" },
                        "max_tokens": { "type": "integer" }
                    },
                    "additionalProperties": true
                }),
                Some(tool_messages),
            ),
            (
                TOOL_MODELS,
                "列出上游可用模型（GET /models）。",
                json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                Some(tool_models),
            ),
            (
                TOOL_TEST,
                "自检：用最小请求依次测试三个协议，返回每个协议的 ok / 延迟 / 文本。",
                json!({
                    "type": "object",
                    "properties": { "model": { "type": "string" } },
                    "additionalProperties": false
                }),
                Some(tool_test),
            ),
            (
                TOOL_DESCRIBE,
                "不联网的自描述：协议、端点、认证方式、默认模型与工具清单。",
                json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                Some(tool_describe),
            ),
        ];

        for (name, description, schema, handler) in tools {
            let descriptor = json!({
                "name": name,
                "version": env!("CARGO_PKG_VERSION"),
                "description": description,
                "input_schema": schema,
                "output_schema": {},
                "timeout_ms": 180_000,
                "idempotent": false,
            });
            let handle = self
                .host
                .register_tool(&descriptor.to_string(), handler, state_ptr);
            if handle.id == 0 {
                self.host.warn(&format!("llm: 注册工具 {name} 失败"));
            } else {
                self.tools.push(handle);
            }
        }
    }
}

define_plugin!(LlmPlugin, "fengyegf.llm");
