# ABI v6 审计与 /help 联动规范（本仓 6 个原生插件）

> 依据：FuyuBot 文档站 llms-full（ABI 契约 / ABI v5→v6 / 命令表与 /help / 插件联动）
> + 外部插件 `xiaoqvan/fuyubot-plugins/crates/fuyubot-plugin-help/README.md`（图片版 /help）。
> 这份文件是**唯一口径**：所有改动都按它对齐，别各写各的。

## 一、ABI v6 五条硬契约

### 1. 版本
- `fuyubot-abi = "=0.1.3"`（其中 `ABI_VERSION = 6`），workspace 统一声明，插件别自己写版本号常量。
- `HostApiVTable::version` 必须等于头文件 `#define ABI_VERSION 6`；不匹配宿主直接拒绝加载
  （`plugin_init 返回了失败句柄`）。插件侧不需要手动比：`define_plugin!` 生成的
  `plugin_init` 已经用 `fuyubot_abi::ABI_VERSION` 填 `PluginHandle.abi_version`。
- `plugin.json` 的 `"abi": 6` 只是给人看的建议值，不参与加载判定——但要保持和实际一致。

### 2. 七个导出符号
`define_plugin!(Type, "id")` 一次性导出全部七个，**不要再手写同名 `#[no_mangle]`**：

| 符号 | 必须 | 说明 |
| --- | --- | --- |
| `plugin_init` | 是 | 校验 VTable、构造状态；失败返回全零句柄 |
| `plugin_free_string` | 是 | 释放**插件自己**分配的字符串 |
| `plugin_start` / `plugin_stop` | 可选 | 注册 / 回收 |
| `plugin_free_handle` | 可选（v4） | 版本不匹配时收尾 |
| `fuyubot_abi_version` | 可选（v4） | 返回编译期 ABI_VERSION |
| `plugin_manifest` | 可选（v4） | 依赖自描述，宿主在 init 之前读 |

验收：`cargo build --release` 后对 cdylib 跑 `python scripts/check_abi.py <dll>`（静态读 PE 导出表）。

### 3. 回调边界
- Rust 侧回调类型必须是 `unsafe extern "C-unwind" fn(*mut c_void, *const c_char) -> i32`。
  **不许**写 `extern "C"`：`extern "C"` 下 panic 直接 abort，宿主来不及 `catch_unwind`。
- 回调里不许 panic 逃逸：不用 `unwrap()` / `expect()` / 越界索引处理**外部输入**；
  解析失败就记日志 + 返回 0。
- 回调里不许阻塞（不能 `call_service` / `wait_for_update` / 发网络请求），只投递到工作线程。

### 4. 回命令不要自己拼 sendMessage
- 宿主提供 `fuyubot.host.command` 的 `reply`：入参
  `{"chat_id":…, "message_id":…, "timeout_ms":…, "payload":{"input_message_content": …}}`，
  返回 TDLib 最终 `Message`，失败回 `{"@type":"error",…}`。
  宿主自己决定：bot/白名单 → 发新消息 + `reply_to`；user 账户自己触发 → 编辑原命令消息；
  群里的 ephemeral 命令 → `sendEphemeralMessage`。**插件不再自己拼 `chat_id`/`reply_to`/`sendMessage`。**
- 因此：凡是「回复某条命令消息」的发送，走 `fuyubot.host.command`；相册（`sendMessageAlbum`）
  与主动推送没有命令消息可回，继续用 simplify `send_message`，代码里写清为什么。
- 该服务**同步阻塞**，只能在插件自己的工作线程里调，禁止在更新回调里调。

### 5. 字符串所有权
- 宿主返回的字符串（`call_service` / `simplify_invoke_result` / `get_config` / `list_tools` /
  `list_services` / `WaitHandle::payload_json`）是**线程局部缓冲**：当场拷成 `String`/`Value`，
  **绝不 free、绝不跨调用持有**。
- 插件自己 `CString::into_raw` 给宿主的，只能用 `plugin_free_string` 还。
- `call_service` 的 NULL = 失败，空串 = 成功无输出——不要混。

## 二、/help 联动（按 fuyubot-plugin-help README 适配）

命令表的主人是**宿主**，不是 help 插件：

| 旧 | 新 |
| --- | --- |
| `fuyubot.help.commands` | `fuyubot.host.commands`（旧名只是一个版本的兼容别名，新代码禁用） |
| `soft_inject(["fuyubot.help.commands"])` | **删掉**：宿主服务启动时就在，不是插件 |
| 只传 `{name, description}` | 补 `role` / `scopes` / `hidden` |

`register` 入参：

```json
{"name":"whois","description":"…","role":"user","scopes":["private","group"],"hidden":false}
```

- `name`：不带 `/`，只允许 `a-z0-9_`，统一小写；**跨插件重名会被拒**。
- `role`：`user` / `chat_admin` / `admin` / `owner`，默认 `user`。
- `scopes`：`private` / `group` / `channel`，默认 `private + group`；`admin`/`owner` 命令默认只有 `private`。
- `hidden`：`true` 只是不进 `/help`，**不是授权**。
- 返回 `{"ok":true,"name":…,"plugin":…}` 或 `{"error":…}`；`plugin` 由宿主按调用者栈判定，
  插件自报不了，也**不需要**报——图片版 /help 按它分组（组标题用宿主报告的 display_name）。

对齐原则：**只登记与当前代码实际权限相符的 role/scopes**，不要借登记去收紧/放宽执行权限。
登记失败只记 warn，不影响命令本身。

图片版 `/help` 的读取链路（我们只读它，不改它）：`fuyubot.host.commands.list` → 按
`role`/`scopes`/`hidden` 过滤 → 按 `plugin` 分组 → `fuyubot.html2img.render` 出图 →
`fuyubot.host.command.reply` 发回。所以本仓插件只要登记正确、回复走宿主，就能被它正确渲染。

## 三、每个插件要交付的改动

1. `src/help.rs`：服务名改 `fuyubot.host.commands`；payload 补 `role`/`scopes`/`hidden`；
   目录注释、常量名、单测跟着改（单测断言 payload 字段齐全）。
2. `src/lib.rs`：删掉只为 help 写的 `soft_inject`（其它软依赖保留）；相关文档注释同步。
3. `src/host/command.rs`：命令回复通道 `call_service("fuyubot.host.command", "reply", …)`；
   `send_text` / `send_photo` 带 `reply_to` 的调用改走它**并且不再自己拼 `reply_to`**；
   相册 / 主动推送保持 simplify `send_message`，注释说明原因。
4. 更新本 crate `README.md` 里提到 `fuyubot.help.commands` / `soft_inject` 的段落。
5. 验收：`cargo check -p <crate> --all-targets --offline`、`cargo test -p <crate> --offline` 全绿；
   `python scripts/check_abi.py target/debug/<lib>.dll` 七个符号齐全。

## 四、禁止事项

- 不要改 `crates/fuyubot-plugin-api`（共享门面，6 个插件都用它）。
- 不要跨 crate 改动；只碰自己那一个 `crates/fuyubot-plugin-<id>/`。
- 不要为了「兼容旧宿主」保留 `fuyubot.help.commands` 双写——这是要淘汰的别名。
- 不要把 `unwrap()` 加进回调路径；不要在回调里做阻塞调用。

## 五、落地结构（6 个 crate 统一）

- `src/help.rs`：命令表登记（服务名 `fuyubot.host.commands`，条目带 role/scopes/hidden）；
- `src/host/command.rs`：`fuyubot.host.command.reply` 客户端（`SERVICE` / `input` / `reply` / `parse_reply` / `reply_code` + 单测）；
- `src/host/messenger.rs`：`reply_to = Some(id)` → `command::reply`；`None` → simplify `send_message`；
- `scripts/check_abi.py`：静态读 PE 导出表，核对七个 ABI 符号（不 dlopen、不执行插件）；
- `scripts/replay-smoke.sh`：用一份 fuyubot 安装目录跑离线回放，验命令表登记与「回命令走宿主」。
