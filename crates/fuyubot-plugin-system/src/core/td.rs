//! TDLib 查询：把 ABI 的「发请求 + 等 response」包成几个同步方法。
//!
//! 宿主把所有 TDLib 响应投递到 `"response"` 通道，负载是**原始响应 JSON** 再补一个
//! 数值型的 `request_id`；这里先用 [`HostExt::invoke_value`] 拿到请求号，再
//! [`HostExt::wait_for_update_json`] 等这个请求号的响应。
//!
//! ⚠️ 这些方法会阻塞当前线程，只能在**业务线程**里调——宿主的事件回调里等响应
//! 等于让收件人自己等自己（和 whois / musiccard 的消息网关同一条约束）。
//!
//! 字段路径按 TDLib 1.8.69 的 `td_api.tl`：
//! `user.profile_photo`（`profilePhoto.big`）→ `file.id` / `file.local.path`，
//! `user.usernames.editable_username`。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fuyubot_plugin_api::{HostApi, HostExt, RESPONSE_EVENT};
use serde_json::{Value, json};

use crate::core::model::Identity;

/// 查询失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("调用宿主失败: {0}")]
    Host(String),
    #[error("等待 TDLib 响应超时（{name}）")]
    Timeout { name: String },
    #[error("TDLib 返回错误（{code}）: {message}")]
    Td { code: i64, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// TDLib 查询门面。`HostApi` 是 `Copy`，这里只是给它加几个名字。
#[derive(Debug, Clone, Copy)]
pub struct Td {
    host: HostApi,
}

impl Td {
    pub fn new(host: HostApi) -> Self {
        Self { host }
    }

    /// 发一个 TDLib 方法并等它的响应（原始 JSON，不剥字段）。
    ///
    /// `method` 形如 `getMe` / `getUser` / `getOption`；`params` 是不带 `@type` 的参数对象。
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let params = if params.is_object() {
            params
        } else {
            json!({})
        };
        let request = self
            .host
            .invoke_value(method, params)
            .map_err(|error| Error::Host(error.to_string()))?;
        let response = self
            .wait_response(request.id, timeout, method)
            .ok_or_else(|| Error::Timeout {
                name: method.to_owned(),
            })?;
        if response.get("@type").and_then(Value::as_str) == Some("error") {
            return Err(Error::Td {
                code: response.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: response
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("未提供错误信息")
                    .to_owned(),
            });
        }
        Ok(response)
    }

    /// 等某条请求号的 `response`；超时返回 `None`（而不是把整条命令炸掉）。
    fn wait_response(&self, request_id: u64, timeout: Duration, name: &str) -> Option<Value> {
        let filter = json!({
            "event_type": RESPONSE_EVENT,
            "request_id": request_id,
        });
        let value = self
            .host
            .wait_for_update_json(filter, timeout.as_millis() as u64)?;
        if value.is_null() {
            eprintln!("system: {name} 的响应为空");
        }
        Some(value)
    }

    /// 读一个字符串型 option（`getOption`）；不是 `optionValueString` 就返回 `None`。
    pub fn option_string(&self, name: &str, timeout: Duration) -> Option<String> {
        let response = self
            .call("getOption", json!({ "name": name }), timeout)
            .ok()?;
        if response.get("@type").and_then(Value::as_str) != Some("optionValueString") {
            return None;
        }
        response
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// `getMe`：机器人（或 user 账号）自己的 `User`。
    pub fn get_me(&self, timeout: Duration) -> Result<Value> {
        self.call("getMe", json!({}), timeout)
    }

    /// `getUser`：按 user id 取用户。
    pub fn get_user(&self, user_id: i64, timeout: Duration) -> Result<Value> {
        self.call("getUser", json!({ "user_id": user_id }), timeout)
    }

    /// 取用户头像的原始字节；失败时返回**可读原因**，调用方记日志并用占位图。
    ///
    /// TDLib 的 `profilePhoto.big/small` 只是远端引用（`local.path` 为空、`size = 0`），
    /// 必须 `downloadFile` 才会真正落盘。这里用 `synchronous = true` 一次拿到最终
    /// `File`（含 `local.path`），**不依赖 `updateFile` 通知**——小头像往往在响应
    /// 之前就下载完了，等通知会漏掉那一条更新（宿主自带的 `download_file` 操作就
    /// 掉在这个坑里：它先发请求再等通知，而且漏了 `priority` / `synchronous` 字段）。
    ///
    /// 顺序：本地文件 → `downloadFile(synchronous = true)` → 轮询 `getFile`；
    /// `big` 失败再退到 `small` 缩略图。
    pub fn avatar(&self, user: &Value, timeout: Duration) -> std::result::Result<Vec<u8>, String> {
        let Some(photo) = user.get("profile_photo").filter(|value| !value.is_null()) else {
            return Err("用户没有资料照片".to_owned());
        };
        if photo.get("@type").and_then(Value::as_str) != Some("profilePhoto") {
            return Err("profile_photo 不是 profilePhoto".to_owned());
        }

        let mut reasons = Vec::new();
        for key in ["big", "small"] {
            let Some(file) = photo.get(key).filter(|value| !value.is_null()) else {
                continue;
            };
            match self.avatar_from_file(file, timeout) {
                Ok(bytes) => return Ok(bytes),
                Err(reason) => reasons.push(format!("{key}: {reason}")),
            }
        }
        if reasons.is_empty() {
            Err("资料照片里没有 big / small 文件".to_owned())
        } else {
            Err(reasons.join("；"))
        }
    }

    /// 从一个 `file` 对象取头像字节：本地已有就直接读，否则同步下载后再读。
    fn avatar_from_file(
        &self,
        file: &Value,
        timeout: Duration,
    ) -> std::result::Result<Vec<u8>, String> {
        // ① 已经下好的头像：直接读文件。
        if let Some(path) = local_path(file)
            && let Some(bytes) = read_file(path)
        {
            return Ok(bytes);
        }

        let file_id = file
            .get("id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0)
            .ok_or_else(|| "文件没有有效 id".to_owned())?;

        // ② 同步下载：TDLib 下载完成后才回响应，File 里直接带 local.path。
        let downloaded = self
            .call("downloadFile", download_request(file_id), timeout)
            .map_err(|error| format!("downloadFile 失败: {error}"))?;
        if let Some(path) = local_path(&downloaded)
            && let Some(bytes) = read_file(path)
        {
            return Ok(bytes);
        }

        // ③ 兜底：少数 TDLib 版本即使 synchronous 也可能先回未完成，轮询 getFile。
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            std::thread::sleep(AVATAR_POLL_INTERVAL);
            let polled = self
                .call(
                    "getFile",
                    json!({ "file_id": file_id }),
                    AVATAR_POLL_TIMEOUT,
                )
                .map_err(|error| format!("getFile 失败: {error}"))?;
            if let Some(path) = local_path(&polled)
                && let Some(bytes) = read_file(path)
            {
                return Ok(bytes);
            }
        }
        Err("下载结束但没有可读的本地文件".to_owned())
    }
}

/// `downloadFile` 的完整请求。
///
/// **四个字段一个都不能少**：`file_id` / `priority` / `offset` / `limit` 都是 TDLib 的
/// 必填参数，漏了会被直接拒（宿主自带的 `download_file` 操作就漏了 `priority` 与
/// `synchronous`，实测头像下载会立刻失败）。`synchronous = true` 让 TDLib 下载完才回
/// 响应，头像小、等得起，换来的是**不依赖 `updateFile` 通知**（那种等待会漏掉
/// 「下载早已完成」的那一条更新）。
fn download_request(file_id: i64) -> Value {
    json!({
        "file_id": file_id,
        "priority": AVATAR_DOWNLOAD_PRIORITY,
        "offset": 0,
        "limit": 0,
        "synchronous": true,
    })
}

/// `downloadFile.priority`：头像给小优先级即可，不抢其它下载。
const AVATAR_DOWNLOAD_PRIORITY: i32 = 1;
/// 同步下载没拿到本地文件时，轮询 `getFile` 的间隔。
const AVATAR_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// 单次 `getFile` 的超时。
const AVATAR_POLL_TIMEOUT: Duration = Duration::from_secs(3);

/// 取 `file.local.path`，空串当没有。
fn local_path(file: &Value) -> Option<&str> {
    file.get("local")?
        .get("path")?
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

/// 读本地文件；读不到或空文件返回 `None`。
fn read_file(path: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(PathBuf::from(path)).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/// 从 TDLib 的 `User` JSON 取出卡片需要的身份字段。
pub fn identity_of(user: &Value) -> Identity {
    Identity {
        name: user
            .get("first_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned(),
        username: user
            .get("usernames")
            .and_then(|usernames| usernames.get("editable_username"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned(),
        id: user.get("id").and_then(Value::as_i64).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_read_from_tdlib_user_shape() {
        let user = json!({
            "@type": "user",
            "id": 8007980715i64,
            "first_name": "绅士王",
            "last_name": "",
            "usernames": {
                "@type": "usernames",
                "editable_username": "fengyezzz_bot"
            }
        });
        let identity = identity_of(&user);
        assert_eq!(identity.id, 8007980715);
        assert_eq!(identity.name, "绅士王");
        assert_eq!(identity.username, "fengyezzz_bot");
    }

    #[test]
    fn missing_fields_degrade_to_empty() {
        let identity = identity_of(&json!({ "@type": "user" }));
        assert_eq!(identity.name, "");
        assert_eq!(identity.username, "");
        assert_eq!(identity.id, 0);
    }

    #[test]
    fn download_request_carries_every_mandatory_field() {
        let request = download_request(2);
        assert_eq!(request["file_id"], 2);
        assert_eq!(request["priority"], 1);
        assert_eq!(request["offset"], 0);
        assert_eq!(request["limit"], 0);
        assert_eq!(
            request["synchronous"], true,
            "必须同步下载，别等 updateFile"
        );
    }

    #[test]
    fn local_path_ignores_blank_and_missing() {
        assert_eq!(
            local_path(&json!({ "local": { "path": r" C:\a.jpg " } })),
            Some(r"C:\a.jpg")
        );
        assert_eq!(local_path(&json!({ "local": { "path": "   " } })), None);
        assert_eq!(local_path(&json!({ "local": {} })), None);
        assert_eq!(local_path(&json!({})), None);
    }

    #[test]
    fn read_file_rejects_missing_and_empty_files() {
        assert!(read_file("").is_none());
        assert!(read_file(r"Z:\definitely-missing\avatar.jpg").is_none());

        let path = std::env::temp_dir().join("fuyubot-system-td-empty-avatar-test.jpg");
        std::fs::write(&path, b"").expect("写临时文件");
        assert!(read_file(path.to_str().expect("UTF-8 路径")).is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// 用假宿主跑通「avatar → downloadFile → 读本地文件」整条线，
    /// 并把实际发出去的请求抓下来，防止 `priority` / `synchronous` 再被漏掉。
    #[test]
    fn avatar_downloads_synchronously_and_reads_local_file() {
        let path = std::env::temp_dir().join("fuyubot-system-avatar-wiring-test.jpg");
        std::fs::write(&path, b"\xff\xd8fake-jpeg-bytes").expect("写头像临时文件");
        let payload = json!({
            "@type": "file",
            "id": 2,
            "local": {
                "path": path.to_string_lossy(),
                "is_downloading_completed": true
            }
        });
        fake_host::set_response(&payload.to_string());

        let td = Td::new(fake_host::api());
        let user = json!({
            "@type": "user",
            "profile_photo": {
                "@type": "profilePhoto",
                "big": {
                    "@type": "file",
                    "id": 2,
                    "local": { "path": "" }
                }
            }
        });
        let bytes = td
            .avatar(&user, Duration::from_secs(1))
            .expect("同步下载应当返回本地文件内容");
        assert_eq!(bytes, b"\xff\xd8fake-jpeg-bytes");

        let request = fake_host::last_request().expect("应当发出 downloadFile 请求");
        assert!(request.contains("downloadFile"), "请求方法：{request}");
        assert!(request.contains(r#""synchronous":true"#), "请求：{request}");
        assert!(request.contains(r#""priority":1"#), "请求：{request}");

        let _ = std::fs::remove_file(&path);
    }

    /// 最小假宿主：只实现 `td_invoke_simplified` 与 `wait_for_update`，其余填桩。
    ///
    /// 字段布局必须和 `fuyubot_abi::HostApiVTable` 完全一致（23 个函数指针），
    /// 否则 `HostApi::from_raw` 会以 `VTableTooSmall` 拒绝。
    mod fake_host {
        use std::ffi::{CString, c_char, c_void};
        use std::sync::Mutex;

        use fuyubot_plugin_api::{
            ABI_VERSION, EventCallbackFn, HostApi, HostApiVTable, MiddlewareCallbackFn,
            MiddlewareHandle, RequestId, ServiceHandle, ServiceVTable, SubscriptionHandle,
            ToolHandle, ToolHandlerFn, WaitHandle,
        };

        static RESPONSE: Mutex<Option<CString>> = Mutex::new(None);
        static REQUEST: Mutex<Option<String>> = Mutex::new(None);

        pub fn set_response(text: &str) {
            *RESPONSE.lock().expect("响应锁") = Some(CString::new(text).expect("响应不含 NUL"));
        }

        pub fn last_request() -> Option<String> {
            REQUEST.lock().expect("请求锁").clone()
        }

        unsafe extern "C" fn td_invoke_simplified(
            _host: *mut c_void,
            method: *const c_char,
            params: *const c_char,
        ) -> RequestId {
            let method = unsafe { std::ffi::CStr::from_ptr(method) }.to_string_lossy();
            let params = unsafe { std::ffi::CStr::from_ptr(params) }.to_string_lossy();
            *REQUEST.lock().expect("请求锁") = Some(format!("{method} {params}"));
            RequestId { id: 7 }
        }

        unsafe extern "C" fn wait_for_update(
            _host: *mut c_void,
            _filter: *const c_char,
            _timeout: u64,
        ) -> WaitHandle {
            let response = RESPONSE.lock().expect("响应锁");
            match response.as_ref() {
                Some(text) => WaitHandle {
                    id: 1,
                    matched: 1,
                    _reserved: 0,
                    payload_json: text.as_ptr(),
                },
                None => WaitHandle::INVALID,
            }
        }

        unsafe extern "C" fn on(
            _host: *mut c_void,
            _event: *const c_char,
            _filter: *const c_char,
            _callback: Option<EventCallbackFn>,
            _user_data: *mut c_void,
        ) -> SubscriptionHandle {
            SubscriptionHandle { id: 1 }
        }

        unsafe extern "C" fn off(_host: *mut c_void, _handle: SubscriptionHandle) {}
        unsafe extern "C" fn emit(
            _host: *mut c_void,
            _event: *const c_char,
            _payload: *const c_char,
        ) -> i32 {
            0
        }
        unsafe extern "C" fn register_tool(
            _host: *mut c_void,
            _descriptor: *const c_char,
            _handler: Option<ToolHandlerFn>,
            _user_data: *mut c_void,
        ) -> ToolHandle {
            ToolHandle::INVALID
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
            core::ptr::null()
        }
        unsafe extern "C" fn list_tools(_host: *mut c_void) -> *const c_char {
            core::ptr::null()
        }
        unsafe extern "C" fn list_plugins(_host: *mut c_void) -> *const c_char {
            core::ptr::null()
        }
        unsafe extern "C" fn provide_service(
            _host: *mut c_void,
            _name: *const c_char,
            _vtable: *const ServiceVTable,
        ) -> ServiceHandle {
            ServiceHandle::INVALID
        }
        unsafe extern "C" fn get_service(
            _host: *mut c_void,
            _name: *const c_char,
        ) -> *const ServiceVTable {
            core::ptr::null()
        }
        unsafe extern "C" fn td_invoke(_host: *mut c_void, _request: *const c_char) -> RequestId {
            RequestId::INVALID
        }
        unsafe extern "C" fn td_execute(
            _host: *mut c_void,
            _request: *const c_char,
        ) -> *const c_char {
            core::ptr::null()
        }
        unsafe extern "C" fn use_middleware(
            _host: *mut c_void,
            _stage: u32,
            _callback: Option<MiddlewareCallbackFn>,
            _user_data: *mut c_void,
        ) -> MiddlewareHandle {
            MiddlewareHandle::INVALID
        }
        unsafe extern "C" fn cancel_wait(_host: *mut c_void, _handle: WaitHandle) {}
        unsafe extern "C" fn log(_host: *mut c_void, _level: u32, _message: *const c_char) {}
        unsafe extern "C" fn get_config(
            _host: *mut c_void,
            _section: *const c_char,
            _key: *const c_char,
        ) -> *const c_char {
            core::ptr::null()
        }
        unsafe extern "C" fn set_config(
            _host: *mut c_void,
            _key: *const c_char,
            _value: *const c_char,
        ) -> i32 {
            0
        }
        unsafe extern "C" fn provide_service_shared(
            _host: *mut c_void,
            _name: *const c_char,
            _vtable: *const ServiceVTable,
        ) -> ServiceHandle {
            ServiceHandle::INVALID
        }
        unsafe extern "C" fn simplify_invoke(_host: *mut c_void, _op: *const c_char) -> RequestId {
            RequestId::INVALID
        }
        unsafe extern "C" fn simplify_invoke_result(
            _host: *mut c_void,
            _op: *const c_char,
        ) -> *const c_char {
            core::ptr::null()
        }

        pub fn api() -> HostApi {
            let vtable: &'static HostApiVTable = Box::leak(Box::new(HostApiVTable {
                version: ABI_VERSION,
                size: core::mem::size_of::<HostApiVTable>() as u32,
                host_ctx: core::ptr::null_mut(),
                on: Some(on),
                off: Some(off),
                emit: Some(emit),
                register_tool: Some(register_tool),
                call_tool: Some(call_tool),
                call_tool_result: Some(call_tool_result),
                list_tools: Some(list_tools),
                list_plugins: Some(list_plugins),
                provide_service: Some(provide_service),
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
                provide_service_shared: Some(provide_service_shared),
                simplify_invoke: Some(simplify_invoke),
                simplify_invoke_result: Some(simplify_invoke_result),
            }));
            // SAFETY: vtable 是 Box::leak 出来的，地址稳定、布局正确。
            unsafe { HostApi::from_raw(vtable) }.expect("假宿主应当能解包")
        }
    }
}
