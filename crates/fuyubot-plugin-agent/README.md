# AI Agent 运行时（`fengyegf.agent`）

Telegram 侧的 **AI Agent 运行时**：把私聊 / 群话题接上大模型，把回复发回 Telegram，
并把「对话能力」暴露成工具与共享服务。它**不是** LLM 网关——网关是 `fengyegf.llm`，
本插件只通过共享服务调它，自己不建 HTTP 客户端、不存 API Key。

- 插件 id：`fengyegf.agent`
- crate：`fuyubot-plugin-agent`（`cdylib` + `rlib`，lib name `agent`）
- ABI：v6
- 版本：`0.1.2`
- 硬依赖：`fengyegf.llm`（`Plugin::inject`；没装网关时插件停在 Pending，不会启动后每条消息都失败）

> **安全提示**：本插件会把宿主上各插件注册的工具交给大模型调用，模型可能在日常对话中
> 无意带出配置 / 调试信息（例如网关地址、模型名）。安装插件、开放工具前请先读
> [安全提示](#安全提示务必阅读)，并按需用 `tool_denylist` 收紧。

## 它负责什么、不负责什么

| 负责 | 不负责 |
| --- | --- |
| 监听 `updateNewMessage`、解析消息 / 话题 / 发送者 | 大模型协议（OpenAI / Anthropic / Responses） |
| 按 `chat + topic` 归档会话，JSON 落盘、重启不丢 | HTTP / SSE / 认证 / 重试 / Key 管理 |
| 提示词组装、单次 / 循环推理、历史压缩 | 大模型协议 / HTTP / 鉴权 / Key（全归网关；本插件只拼会话） |
| 工具调用：宿主工具表 + 插件共享服务（describe / invoke）按协议下发给模型，同步执行并回填 | 工具 / 服务的实现（由宿主与各插件提供，本插件只转发 JSON） |
| 防环：去重、Bot-to-Bot 频率与深度、白名单（人类消息不做限速） | 图片 / 语音等媒体理解（只把类型占位交给模型） |
| `/agent` 命令、6 个工具、共享服务 `fengyegf.agent` | 跨插件编排（只做被动调用方给出的 JSON） |

## 目录

```text
crates/fuyubot-plugin-agent/
├── Cargo.toml               # rlib + cdylib：既能被宿主加载，也能被 Rust 侧单测
├── plugin.json              # 分发包清单（id / entry / abi / provides / depends）
├── config.default.toml      # 默认配置（最低优先级）
├── README.md
└── src/
    ├── lib.rs               # 插件壳：生命周期、state 指针、订阅 / 工具 / 服务装配
    ├── core/                # 纯数据与配置：不依赖宿主，方便单测
    │   ├── types.rs         # ChatKey / IncomingMessage / ChatMessage / TurnRequest…
    │   ├── quote.rs         # 引用回复的上下文块（纯渲染，可单测）
    │   ├── consts.rs        # 内置常量：流式 / 超时 / 工具轮数 / 截断长度 / 压缩额度
    │   ├── config.rs        # Config：解析 [plugin.fengyegf.agent]，给全量默认值
    │   └── error.rs         # AgentError：稳定 code，工具 / 服务回包复用
    ├── td/                  # 唯一碰 TDLib 的地方（同步阻塞；message.rs = 取被引用消息）
    ├── agent/               # Agent 运行时：GatewayClient / PromptBuilder / Guard / AgentEngine
    │   ├── compact.rs       # 历史压缩：会话转录 + 压缩器提示词（纯文本处理）
    │   ├── artifact.rs      # 长工具结果缓存：分页信封 + JSON 路径选择
    │   ├── tools.rs         # 工具表 + 插件服务 → 协议工具定义 + 执行（消费方模板）
    │   └── toolcall.rs      # 工具调用 / 结果回填的协议形状（纯函数）
    ├── session/             # 会话与 JSON 落盘（原子写，按 chat + topic 一文件）
    ├── api/                 # 对外接口：6 个工具 + 共享服务 fengyegf.agent
    │   ├── mod.rs           # 注册 / 按方法名分发 / FFI 边界 / describe
    │   └── tools/           # 一个工具一个文件：chat / run / send / session / title / result
    ├── cmd.rs               # /agent 命令表：解析 → 会话管理动作 → 文案
    └── host/
        ├── mod.rs
        └── worker.rs        # 订阅回调（只投递）+ agent-worker 线程（解析 / 准入 / 推理）
```

安装到宿主后的标准分发包布局（ZIP 根就是插件目录）：

```text
plugins/fengyegf.agent/
├── plugin.json
├── config.default.toml
└── bin/
    └── agent.dll            # Linux: libagent.so / macOS: libagent.dylib
```

## 消息链路

```text
updateNewMessage
  → host::worker::on_update        回调只做一件事：把 JSON 丢进 channel
  → agent-worker 线程
      → 600ms 内同一会话的连发消息合并（只回答最后一条；命令不参与）
      → td::parse_incoming         纯函数解析成 IncomingMessage（含引用关系）
      → is_outgoing / 服务消息过滤 / 命中 /agent → cmd::dispatch → 直接回复，结束（不建话题、不推理）
      → 会话类型准入
      → td::quoted_message         点「回复」时取回被引用消息（群里「回复我也算点名」也用它）
      → 群内 @ / bot 身份 / 防环 Guard
      → 私聊 + 无话题 + topic_auto_create → td::create_topic（新会话）
      → td::send_text_draft        空草稿 = 客户端自带的 Thinking… 占位
      → 网关支持 stream → stream + stream_poll：每块增量直接更新草稿（首字延迟真实下降）
        网关不支持 / 群聊 → AgentEngine::run 同步拿终稿，再按块回放草稿
      → 模型要调工具 → 同步执行宿主工具 → 结果回填再问一次（内置最多 8 轮）
      → td::send_text / send_rich  正式消息落地，草稿自动消失；不引用原消息
      → session 追加落盘
```

三个设计点：

- **回调里绝不阻塞**：TDLib 调用全是同步等 response，放在订阅回调里会卡死宿主的接收循环，
  所以回调只投递，解析 / 准入 / 推理 / 发送都在 `agent-worker` 线程里做；
- **命令短路**：`/agent` 在准入之后、推理之前就返回，命令不会触发网关调用，也不会建会话话题
  （`/agent new` 除外，它显式建一个）。
- **连发合并**：正常的连续消息按会话合并成一次请求（见下一节），命令不参与合并。

## 引用回复（用户点「回复」）

Telegram 的 `updateNewMessage` 里，`reply_to`（`messageReplyToMessage`）**只带被引用消息的 id**
和用户手动选中的片段，没有正文。只把这条消息交给模型，它看到「这个呢？」根本不知道指哪条，
所以 worker 在准入阶段先取一次被引用消息：

1. `getRepliedMessage`（回复链，跨话题 / 跨群都能解析）→ 失败退回 `getMessage`
   （按 `reply_to.chat_id` + 消息 id，chat_id 缺省用当前 chat）；
2. 正文按内容类型拍平：`messageText` / 媒体 caption 直取；`messageRichMessage`（本插件自己的
   rich 回复）把 `richMessage.blocks` 的块树拍成纯文本，行内富文本（粗体 / `richTextConcat` /
   公式）拼成一串；纯媒体退化成 `[messagePhoto]` 这样的占位；
3. 拼成引用块放在本轮输入最前面（同时进会话历史，后续轮次也看得到）：

   ```text
   [引用消息 #101｜发送者：你（助手）]
   刚才那条是 42
   [引用结束]

   那这个是什么意思？
   ```

   手动选中片段会单独列一行「用户选中的片段：…」；发送者查不到时退化成 user id，
   匿名管理员记成「匿名管理员 / 频道」。

被引用消息取不到（已删除 / 回放 / 无权限）**不会**变成没有引用：仍然保留
`[引用消息 #id]` 标记和一句说明，模型至少知道「他在回复某条消息」，不用瞎猜。

群里 `group_require_mention = true` 时，「回复我的消息」与 `@我` 等效（Telegram privacy mode
投递的本来就是这两类），判断依据就是这次取回的发送者是不是自己。

引用上下文始终开启（群里「回复我 = 点名」也依赖它）；引用正文 / 选中片段按 800 字截断，
不会因为一条长消息把上下文顶满。

## 连续消息合并（一次对话）

用户连着发几句话时，不要每句都请求一次模型。worker 收到第一条正常聊天消息后等一个
**600ms 静默窗口**；窗口内同一会话的新消息继续并入，直到静默 / 达到 20 条上限 / 遇到命令，
然后把这几句按顺序拼成**一次**用户输入，只回答最后一条。

- 只合并正常聊天消息；`/agent` 命令、服务消息、outgoing 更新不参与（命令立刻执行）；
- 按 `chat + topic` 分组：不同会话互不干扰，每个会话各产生一次请求；
- 窗口内任意一条 @ 我（或回复我）就算点名；引用上下文只取最后一条；
- 会话落盘里也只有一条 user 消息（合并后的正文），不会「一句话一条历史」；
- 代价是单条消息也会多等最多 600ms；连发越多越划算（N 句 → 1 次请求、1 条回复）。

窗口与上限是内置常量（`core/consts.rs` 的 `MESSAGE_MERGE_MS` / `MESSAGE_MERGE_MAX`），
不开放配置。

## 配置

默认值在 `config.default.toml`，用户在自己的 `fuyubot.toml` 里覆盖；
三层优先级是「包默认 < `plugin-config/` 运行时落盘 < 用户配置」，插件只用 `HostExt::config_value` 读合并后的结果。

**设计原则**：Agent 的行为尽量确定，配置只留「部署环境真的会不同」的项。会话历史默认
**全量携带**（没有「每次带几条」），上下文太长用 `/agent compress` 让模型压成摘要；
流式节奏、工具轮数、截断长度、重试次数、连发合并窗口等都在 `src/core/consts.rs` 里写成常量，不开放配置。

```toml
[plugin.fengyegf.agent]
model = ""
reply_mode = "rich"
topic_auto_create = true
```

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `true` | 总开关。关掉 = 不订阅、不注册工具 / 服务，插件仍在进程里可见 |
| `gateway_method` | `"chat"` | 走网关的哪个方法（`chat` / `messages` / `responses`） |
| `model` | `""` | 空 = 用网关的默认模型 |
| `system_prompt` | 内置一句 | 系统提示词；留空则不注入 `system` 消息（富消息语法说明始终附加） |
| `reply_mode` | `"rich"` | 终稿形态：`plain` / `markdown` / `rich`（失败逐级降级）。rich = `inputMessageRichMessage` |
| `stream_replies` | `true` | 私聊用 TDLib 草稿消息做官方流式；关掉 = 直接发终稿 |
| `allow_private` | `true` | 是否接私聊 |
| `allow_groups` | `true` | 是否接群 / 超级群 |
| `group_require_mention` | `true` | 群里必须 @ 我（或回复我的消息）才接 |
| `allow_bots` | `false` | Bot-to-Bot 总开关；默认关 |
| `bot_allowlist` | `[]` | 允许对话的对端 bot 用户名（不带 @）；空 = 只按 `allow_bots` |
| `tool_denylist` | `[]` | 额外不下发的工具名：精确名或以 `*` 结尾的前缀（如 `fengyegf.agent.run`、`my.plugin.*`）；只能追加，`fuyubot.td.functions` 是内置屏蔽、删不掉 |
| `topic_auto_create` | `true` | 私聊首条消息自动开话题（新会话） |
| `session_dir` | `""` | 会话落盘目录；空 = `data/plugins/fengyegf.agent/sessions` |
| `session_ttl_days` | `7` | 会话自动清理天数（启动时 + 每小时检查）；`0` = 永久保留，不自动删 |

> 旧版本里的 `max_history` / `model_tools` / `max_tool_rounds` / `tool_result_max_chars` /
> `stream_*` / `retry_*` / `failure_text` / `quote_*` / `bot_max_depth` / `bot_rate_limit_secs` /
> `dedupe_window_secs` / `topic_title_*` / `topic_icon_color` / `session_max_*` / `commands` /
> `tools` / `service` / `loop_max_turns` / `max_tokens` / `temperature` / `gateway_service` 等键
> 已移除：留着不会报错，但不再生效；需要改这些行为就改代码里的常量。

### 上下文与压缩

Agent 默认**记得整个会话**：每轮请求都带这个会话的全部历史（落盘上限 2000 条，只是文件体积闸门）。

上下文太长时用 `/agent compress`（或 API `session` 的 `action = "compress"`）：

1. agent 把当前历史拼成转录，调一次网关，让模型输出一份「后续可以直接接着聊」的摘要；
2. 摘要落进会话（`summary`），消息清空；`/agent reset` 会连摘要一起清；
3. 之后每轮请求在系统提示后注入这份摘要（system 消息），再带压缩之后产生的新消息；
4. **压缩失败不落盘**：网关报错时原历史一条不动；摘要超过 4000 字会截断。

**不自动压缩**：什么时候压由用户 / 调用方决定——想省额度就手动压，想保留原文就不压。

## 插件联动（能力注册表 / 消费方模板）

本插件对外既是「能力的注册表」也是「调用方」：提供方在自己的 `start()` 里把能力注册进来，
模型点中时 agent 用 `call_service` 调回去。没有中心服务，也没有额外插件。

**提供方**（完整实现见 `system` 插件的 `src/api/mod.rs`）：

1. `provide_service_shared(自己的插件 id, &vtable)`，方法 `describe` / `invoke`；
2. 调一次 `host.call_service("fengyegf.agent", "register", 载荷)`：

```json
{
  "name": "fengyegf.system.info",
  "description": "读取宿主机设备信息",
  "input_schema": {"type": "object", "properties": {}},
  "service": "fengyegf.system",
  "capability": "fengyegf.system.info",
  "timeout_ms": 30000,
  "idempotent": true
}
```

3. agent 不需要被通知：每一轮重建工具目录时读注册表（`agent/registry.rs`）。

**调用**：模型点中能力 → `call_service(service, "invoke", {"capability":…,"input":…})`，
取回包里 `data`；失败（服务不在 / 不认识能力 / 调用报错）→ 退回 `call_tool_result(能力名, …)`
（同名工具还在），由 `service_tool_fallback` 控制。

同名能力若既在工具表里、又被注册过，**优先服务通道**——这就是「agent 调外部工具也走
provide_service」的落点。条目数见 `describe` 回包的 `registry.count`。

## 对外契约

### 共享服务 `fengyegf.agent`

其它插件按名字调用（`host.call_service("fengyegf.agent", "chat", input)`），
入参 / 出参都是 JSON 对象。服务名 `fengyegf.agent`，一共 9 个方法：

| 方法 | 用途 |
| --- | --- |
| `chat` | 单次完成：带 `chat_id` 时按会话取历史并落盘（`RunMode::Single`、1 轮），不带则直通网关、不落历史 |
| `run` | 在某个会话上跑一轮（带历史、可循环迭代） |
| `send` | 主动往指定 chat / topic 发一条消息（markdown 优先富消息，失败逐级降级） |
| `session` | 会话管理：`list` / `get` / `reset` / `delete` / `create` / `compress`（`get` 优先用 `id`，否则 `chat_id + topic_id`） |
| `describe` | 能力 / 配置自检（不联网、不发请求；含 `registry.count`） |
| `register` | 注册一条能力（提供方调它）：`{name, description, input_schema, service, capability, …}` |
| `unregister` | 注销自己注册的能力（只有登记方能删） |
| `registry` | 当前注册表快照（诊断 / 自检） |

成功与失败统一形状：

```json
{"ok": true,  "...": "具体结果"}
```

```json
{"ok": false, "code": "bad_request", "error": "参数错误：缺少 prompt（非空字符串）"}
```

`code` 取 `host` / `tdlib` / `gateway` / `store` / `config` / `bad_request` / `disabled`；
FFI 边界还有两个兜底码：`panic`（处理器 panic 被 `catch_unwind` 拦下）、`state`（插件状态未初始化）。

#### `chat`：单次完成

入参：

```json
{
  "prompt": "你好，介绍一下你自己",
  "chat_id": 7770001,
  "topic_id": 12,
  "session": "可选：显式会话 id",
  "system": "可选：覆盖系统提示词",
  "model": "可选：覆盖模型",
  "max_tokens": 512
}
```

`prompt` 必填（去空白后不能为空）。带 `chat_id` 时按 `chat + topic` 取历史、落盘；
不带时**直通网关、不落历史**（调用方自己管上下文）。

出参（带会话）：

```json
{"ok": true, "session": "7770001__12", "reply": "……", "model": "deepseek-chat", "usage": {"prompt_tokens": 12, "completion_tokens": 33}}
```

`model` 只在网关返回时出现；`usage` 是网关原始 usage 对象。

出参（无会话，直通）：

```json
{"ok": true, "session": null, "reply": "……", "model": "deepseek-chat", "usage": {}}
```

#### `run`：会话内一轮（可循环）

入参与 `chat` 相同，额外两个可选键：

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `mode` | `loop` | `single` 只跑一轮；`loop` 迭代到收敛或达到 `max_turns` |
| `max_turns` | `3` | 循环上限；`single` 时固定 1 |

```json
// 入参
{"prompt": "把这句改成正式语气", "chat_id": 7770001, "topic_id": 12, "mode": "loop", "max_turns": 3}
// 出参
{"ok": true, "session": "7770001__12", "reply": "……", "turns": 2, "usage": {}}
```

没有 `chat_id` 时退化成一次直通，出参是 `turns: 1`、`session: null`。

#### `send`：主动发送

入参：

```json
{
  "chat_id": 7770001,
  "topic_id": 12,
  "text": "纯文本兜底",
  "markdown": "**富文本**（优先走富消息 / markdown）",
  "reply_to": 101
}
```

`chat_id` 必填；`text` / `markdown` 至少要有一个非空字符串（都有时 `text` 当纯文本兜底，
`markdown` 走富消息 → markdown → 纯文本逐级降级）。

出参：

```json
{"ok": true, "chat_id": 7770001, "message_id": 12345, "offline": false}
```

回放里 `offline` 为 `true`、`message_id` 为 0（请求只留档，没有真实消息号）。

#### `session`：会话管理

入参：

```json
{"action": "list", "chat_id": 7770001, "topic_id": 12, "id": "7770001__12", "title": "旅行计划"}
```

`action` 必填且只认 `list` / `get` / `reset` / `delete` / `create` / `compress`（大小写宽松）。

| action | 必要定位 | 出参 |
| --- | --- | --- |
| `list` | 可选 `chat_id` | `{"ok":true,"sessions":[{"id":"7770001__12","chat_id":7770001,"topic_id":12,"title":"旅行计划","message_count":3,"updated_at":1760000000}]}`；不带 `chat_id` 列全部会话 |
| `get` | `chat_id`（+ `topic_id`）或 `id` | `{"ok":true,"session":{"id":"…","chat_id":…,"topic_id":…,"title":"…","created_at":…,"updated_at":…,"messages":[{"role":"user","content":"…","ts":…}],"summary":"…","compacted_at":…,"compacted_messages":…,"meta":{}}}` |
| `reset` | `chat_id`（+ `topic_id`）或 `id` | `{"ok":true,"removed":3}`：清掉了多少条消息（会话本身保留） |
| `delete` | 同上 | `{"ok":true,"removed":1}`；`0` = 本来就不存在 |
| `create` | `chat_id`（+ `topic_id`） | `{"ok":true,"session":{…}}`；`title` 空时用「AI 会话」 |
| `compress` | `chat_id`（+ `topic_id`）或 `id` | `{"ok":true,"session":"7770001__12","removed":12,"summary":"…"}`：把历史压成摘要；没有历史返回 `bad_request` |

#### `describe`：自检

```json
{
  "ok": true,
  "plugin": "fengyegf.agent",
  "version": "0.1.2",
  "gateway": {"service": "fengyegf.llm", "method": "chat", "available": true},
  "config": {"reply_mode": "rich", "model": "", "history": "full", "compression": true, "merge_window_ms": 600, "allow_bots": false, "session_dir": "data/plugins/fengyegf.agent/sessions", "stream_replies": true, "tool_count": 9, "tool_names": ["fengyegf.agent.chat", "…"], "service_tool_count": 0, "registry": {"count": 0, "capabilities": []}, "tool_denylist": [], "tool_reserved": ["fuyubot.td.functions"]},
  "sessions": 7,
  "artifacts": {"count": 0, "bytes": 0}
}
```

不发网络请求、不碰 TDLib：网关可用性来自宿主的服务清单，会话数来自本地目录，适合面板 / 诊断用。

### 工具（宿主 / AI / 其它插件）

| 工具 | 入参 | 说明 |
| --- | --- | --- |
| `fengyegf.agent.chat` | 同服务 `chat`；required：`prompt` | 单次完成 |
| `fengyegf.agent.run` | 同服务 `run`；required：`prompt` | 会话内一轮 |
| `fengyegf.agent.send` | 同服务 `send`；required：`chat_id`（`text` / `markdown` 至少一个） | 主动发送 |
| `fengyegf.agent.session` | 同服务 `session`；required：`action` | 会话查询 / 管理（`action=compress` 压历史） |
| `fengyegf.agent.title` | `chat_id` / `topic_id` / `title`（全部必填） | 重命名话题标题，并回写会话落盘标题 |
| `fengyegf.agent.result` | `artifact`（必填）+ `page` / `offset` / `limit` / `path` / `fields` / `item_offset` / `item_limit` | 读取过长工具结果：翻页或按 JSON 字段过滤 |

工具与服务**共用同一套 JSON 处理函数**：工具是同步返回的（`call_tool_result`），
服务是同步调用（`call_service`），两者入参 / 出参完全一致，不需要维护两份文档。

### 工具调用（模型 → 宿主工具）

模型不只会说话，也能**调工具**。每次请求都把宿主的**全部工具**（`HostApi::list_tools()`）
按当前协议下发给模型：OpenAI chat 用 `tools[].function{name,description,parameters}`、
Anthropic 用 `tools[].{name,description,input_schema}`、Responses 用 `tools[].{type:"function",…}`，
都带 `tool_choice: "auto"`。模型点了工具就同步执行（`HostApi::call_tool_result`），
把结果回填成下一条消息（chat: `role:"tool"`；messages: `tool_result` 块；
responses: `function_call_output`）再问一次，直到它给出文本。

- **常开**：每轮都把宿主的全部工具下发给模型（Agent 的意义就是能动手）；不想给某个工具就把名字写进 `tool_denylist`；
- **轮数**：内置最多 8 轮。用尽后这一轮**不再下发工具**再问一次，模型没有工具可用，
  只能把结论写成文本；
- **屏蔽**：`tool_denylist = []` 默认全发。挡一个写精确名，挡一族写 `前缀*`。
  注意 `fengyegf.agent.*` 是**本插件自己**的工具，全量下发时模型可以反过来调它
  （`.run` 还会再跑一轮推理，可能套娃），需要时把它们列进 denylist；
- **内置屏蔽**：`fuyubot.td.functions` 这类「给插件作者用的通用 TDLib 大接口」**永远不下发**
  （见 `agent::RESERVED_TOOLS`）。它的入参 schema 覆盖几百个 TDLib 函数，模型好奇心一上来
  就会反复试探，一两轮就能把上下文撑爆；它也不是对话能力，`tool_denylist` 只能追加、解不开。
  就算模型幻觉间直接点了它，执行层也会回一条错误文本，不会真的调宿主；
- **失败也是内容**：工具报错回填成 `{"ok":false,"error":"…"}`，模型能换参数重试，
  不会让整轮请求失败；
- **长结果**：单次工具结果超过 4000 字时不再整段回填：原文存 artifact（内存、TTL 10 分钟），回第一页 + JSON 形状 + 读取提示；模型可用 `fengyegf.agent.result` 翻页或按字段过滤（见下节）；
- **历史**：工具往返**不落盘**，只有最终的 user / assistant 文本进会话，历史不被工具消息污染；
- **流式**：带工具的一轮**不走网关真流式**——流式通道只透传文本增量、看不到工具调用；
  自动退回「同步跑完 + 脚本分块回放」，观感一致，首字延迟变成整轮生成完。

### 长工具结果：artifact 分页与字段过滤

工具结果超过 4000 字时，不再整段塞给模型，也不直接丢尾巴：

1. 原文按 artifact id 存进内存（默认 TTL 10 分钟，最多 64 条 / 16 MiB；单条超过 2 MiB 不缓存、退回普通截断）；
2. 回给模型的是一个信封：第 1 页（3500 字）+ `artifact` + `total_chars` / `page_count` +
   JSON `shape`（顶层类型、一级键、数组长度与元素键）+ 内部读取工具的提示；
3. 模型发现内容太长时，只取自己需要的部分：

```json
// 翻页：第 2 页
{"artifact": "r3f8a1c2d4e5f6070", "page": 2}

// 只要排行榜里的名称字段（按 JSON 里的顺序返回）
{"artifact": "r3f8a1c2d4e5f6070", "path": "data.items[*].name"}

// 一次取多个字段，并分批
{"artifact": "r3f8a1c2d4e5f6070", "fields": ["data.items[*].name", "data.items[*].score"], "item_limit": 50}

// 某一项的标题
{"artifact": "r3f8a1c2d4e5f6070", "path": "data.items[0].title"}
```

- 路径语法：`a.b`、`items[0]`、`items[*]`（数组元素 / 对象的值）；在数组上省略 `[*]` 也能对每个元素取字段；
- 过滤结果永远是「值列表」：单路径用 `values`，多路径用 `路径 -> 列表`；`matched` 是总匹配数，
  `next_item_offset` 给出下一批的起点，`item_offset` / `item_limit` 分批读；
- 翻页按**字符**：`page`（1 起）或 `offset` 二选一，`limit` 上限 3500；
- 读取工具本身的结果不再触发分页；`tool_denylist` 挡掉 `fengyegf.agent.result`（或 `fengyegf.agent.*`）时，
  agent 退回普通截断，不会留下读不了的 artifact；
- artifact 只在内存里、id 不可推测（随机令牌）、不落盘；会话历史里仍然只有最终的 user / assistant 文本，
  工具结果本身不进入后续轮次。

## 安全提示（务必阅读）

本插件把宿主上**其它插件注册的工具**按协议下发给大模型，模型会自己决定调哪个。
工具本身通常没有恶意，但**模型能调用的工具，就等于它能读到的信息**——风险往往不是被攻击，
而是模型在正常对话里「顺手」把信息带出来（尤其群里 / 公开话题）。

- **配置与运行时信息**：例如 `fengyegf.llm.describe` 这类自描述接口会返回网关的
  `base_url`、模型名、是否配置了 key、可用方法等。模型完全可能因为「想确认自己能用
  什么模型」而调用，然后把你的中转地址 / 私有部署域名复述在回答里。
- **调试 / 诊断接口**：不少插件提供 `describe` / `debug` / `status` / `test` 一类方法或工具，
  本来是排查问题用的，返回内容比正常业务接口更细（路径、配置片段、上游原始响应）。
  当前版本**不会**统一屏蔽这类接口。
- **其它插件的业务数据**：工具能读到什么，模型就能在对话里复述什么；私聊里说出去，
  或者被写进会话 JSON 落盘，都算已经离开你的掌控。

目前只有内置屏蔽 `fuyubot.td.functions`（原始 TDLib 总入口，schema 巨大、会撑爆上下文）；
**其余高风险接口需要你自己判断并挡掉**：

1. 安装任何插件前，先读它的 README 与 `describe` 输出，确认工具会返回哪些字段；
2. 不需要给模型看的工具写进 `tool_denylist`（精确名或 `前缀*`），例如：

   ```toml
   [plugin.fengyegf.agent]
   tool_denylist = [
     "fengyegf.llm.describe",   # 会带出网关地址 / 模型 / key 状态
     "fengyegf.agent.*",        # 本插件自己的工具，挡掉避免套娃
     "some.plugin.debug_*",     # 前缀：some.plugin.debug_ 开头的工具都挡
   ]
   ```

   内置屏蔽是硬编码的，`tool_denylist` 只能在它之上追加、不能解除；
3. 想让模型完全不碰工具时，把所有工具列进 `tool_denylist`（本插件没有「关工具」开关）；
4. 群聊 / 公开场景按「白名单」思路收紧：只留确实需要的工具，其余全部挡掉；
5. 定期看一眼 `fengyegf.agent` 服务 `describe` 返回的 `tool_names` / `tool_denylist`，
   确认下发给模型的清单没有意外变化（插件热插拔会改变它）。

一句话：把模型当成**能替你执行操作的同事**，而不是只会聊天的窗口；它读到的东西，
早晚可能出现在某条回复里。

## `/agent` 命令

登记进宿主命令表（`role = "user"`，默认 `private + group`），给 `/help` 用；真正的分派在
worker 里，只在它拦下后处理，不触发推理。

> 本插件订阅的是原始 `updateNewMessage`，命令文本由自己的 `core::parse_command` 解析，
> **只认宿主归一化后的 `/` 前缀**（`/agent …`）。`[command].prefix` 里的 `.`、`!` 等命令头
> 由宿主先归一化成 `/agent`，插件不再重复读取或维护前缀表。

| 子命令 | 别名 | 作用 |
| --- | --- | --- |
| `/agent help` | 空 / 未知子命令 | 用法说明 |
| `/agent new [标题]` | `open` | 只在私聊：建一个话题作为新会话；不给标题用「AI 会话」（群里发会提示只能在私聊开） |
| `/agent list` | `ls` | 列出当前 chat 的会话（本地 + TDLib 话题合并；TDLib 失败只列本地） |
| `/agent use <id>` | `switch` | 按 id 查看会话摘要（路由仍按 chat + topic 自动进行） |
| `/agent reset` | `clear` | 清空当前会话的历史（连压缩摘要一起清），回复清了几条 |
| `/agent compress` | `compact` / `summarize` / `压缩` | 把当前会话历史压成摘要（之后每轮自动带上）；没有历史时给提示 |
| `/agent status` | `state` | 会话 id / 消息数 / 上下文状态 / 模型 / 回复形态 / 网关可用性 / 话题开关 |

会话 id 就是落盘文件名主干：`<chat_id>__<topic_id>`，主聊天（无话题）的 topic 记 0，
例如 `7770001__12`、`7770001__0`。

## Bot-to-Bot 与话题的前置条件

这部分**不是插件能自己打开的开关**：Telegram 侧的设置不做，消息根本不会投递到插件。

### 1. 两个 bot 都要开 Bot-to-Bot

- 在 [@BotFather](https://t.me/BotFather) 里 `/mybots` → 选 bot → **Bot Settings** →
  打开 **Bot-to-Bot Communication Mode**；
- **双方都要开**：只开一边，另一边发不出 / 收不到；
- 插件侧还要把 `allow_bots` 改成 `true`；默认 `false` 是有意的——两个 bot 互相回话很容易刷屏。

### 2. 话题（Threads / Topics）

- **私聊话题**：BotFather 里给 bot 打开 Topics / Threads 开关，私聊里才能 `createForumTopic`
  （本插件 `topic_auto_create = true` 时会自动开）；
- **群话题**：群必须是开启了话题的**超级群**（Forum），bot 需要「管理话题」权限；
- 没有话题能力时插件会降级：`create_topic` 失败 → 退回普通私聊，会话 key 的 topic 记 0，
  链路继续，不会因为建话题失败而丢消息。

### 3. 群里要 @ 或回复

- Telegram bot 默认 privacy mode：群里只收到 @ 它 / 回复它的消息。要收全部消息得在 BotFather
  `/setprivacy` → Disable（或把 bot 设为管理员）；
- 即使收到了，本插件默认 `group_require_mention = true` 仍要求文本里出现 `@<bot用户名>`——
  这是防"群里所有闲聊都进模型"的第二道闸；
- 群 / 超级群消息的 chat id 是负数（`-100…`）。

### 4. 防环策略

两层叠加，任何一层命中都直接丢（只进日志，不回复）：

1. **去重**：30 秒内同一条消息只处理一次（防 TDLib 重发 / 上游重复回调）；
2. **bot 频率与深度**：同一个对端 bot 至少间隔 5 秒，连续往返不超过 2 轮（内置常量）。

**人类消息不做时间限速**：真机上踩过「话题创建的服务消息先占掉速率窗口，紧接着用户发的
问题被静默丢弃」。人类连发是真实需求，去重靠 `(chat_id, message_id)` 就够；只有 bot 对端
才收口。`rate_limit_secs` 这个键已随这次修复移除。

再加上最外面的 `allow_bots = false` 与 `bot_allowlist`：默认不跟任何 bot 对话，
要开就明确列出允许的对端。

另外，服务消息（`messageForumTopicCreated` / `messageForumTopicEdited` / `messageChatAddMembers`、
置顶、入群等）在准入第一道闸就被丢掉：它们没有正文，让模型回答只会浪费额度、还会占住真正的
用户消息。

### 5. 回复没出现时怎么查

每轮**成功投递**都会打一行日志，先看它在不在：

```text
agent: 回复已投递（markdown；编辑消息 123，chat 456，topic Some(12)）
agent: 回复已投递（markdown；新消息 124，chat 456，topic Some(12)）
```

- 有这行 = 插件已经把消息交给 TDLib，问题在客户端 / 话题视图（例如回复进了新建的**话题**，
  而你在 General 里看）；
- 没有这行、但有 `草稿流式更新失败，直接发终稿` = 草稿被拒（比如客户端/账号不支持），终稿仍会发；
- 连「已回复」都没有 = 消息被准入 / 防环挡了，往上翻 `跳过非聊天内容` / `防环丢弃` / `发送者是 bot`；
- 要看原始 TDLib 应答：宿主加 `--trace-updates`，会逐条打印 update / response。

## 回放验证

`--replay` 不接 TDLib、不用账号，只把夹具里的 `updateNewMessage` 灌进整条管线，跑完打印插件日志与
「插件发出去的请求」（请求只留档，没有响应），逐条期望见 [examples/replay/README.md](examples/replay/README.md)。

```bash
# 1. 编译
cargo build -p fuyubot-plugin-agent --offline

# 2. 装到一个干净目录（别指向 target/debug，那里有整仓的示例产物）
#    agent 硬依赖 fengyegf.llm：同一目录里也要装 llm 插件，否则 agent 停在 Pending。
fuyubot plugin install target/debug/agent.dll --plugin-dir demo-plugins
fuyubot plugin install target/debug/llm.dll --plugin-dir demo-plugins

# 3. 回放主用例
fuyubot run --replay crates/fuyubot-plugin-agent/examples/replay/new_message.json \
  --plugin-dir demo-plugins --replay-wait 5000
```

在 crate 目录里跑时，夹具路径可以直接写 `examples/replay/new_message.json`。

输出里该看到：

- `回放 updateNewMessage → UpdateOutcome { update_type: "updateNewMessage", listeners: 1, warnings: [] }`
  —— 订阅建立成功，没有阶段警告；
- `插件打出来的日志：` 里有 `agent: /agent 已登记进命令表`、`agent: 已启动（…）`，
  然后是：查发送者身份失败（回放没有身份服务，按普通用户处理）→ 开话题（离线占位话题 **#1**）→
  网关请求失败时发「请求失败：…」→ `草稿占位 #101` → `草稿流式完成（N 块）` →
  `回复已投递（markdown；…，流式）` → `会话 7770001__1 已回复（N 字）`；
- `插件发出去的请求：` 里能看到 `getUser`、`createForumTopic`、**空文本** `sendTextMessageDraft`
  （Thinking 占位）、分块 `sendTextMessageDraft`，以及一条**不带 `reply_to`** 的 `sendMessage`
  —— 这就是官方流式的形态：草稿先出现，正式消息最后落地；
- **不应该**因为等 response 卡满 `--replay-wait`：离线合成号（`[1 << 48, 1 << 49)`）不会等应答。

> **请求号分三档**（宿主 `fuyubot-loader` 的 runtime.rs）：TDLib 自己的 `@extra` 是小号，
> 没接 TDLib 时的**合成号**是 `[1 << 48, 1 << 49)`，`simplify_invoke` 的**操作号**是
> `>= 1 << 49`。只有合成号才「永远没有响应」；把操作号也当离线会立刻返回 `None`，
> 于是发消息拿不到消息号、`invoke`（取用户 / 取话题 / 取被引用消息）永远取不到东西——
> 0.1.3 修的就是这条（之前 `>= 1 << 48` 一刀切，宿主 0.1.6 起操作号也在这一档之上）。

其余夹具（话题消息、bot 消息、命令、outgoing 噪声、群未点名）的期望现象逐条写在
[examples/replay/README.md](examples/replay/README.md)。

## 与 LLM 网关的边界

- **不自建 HTTP**：所有推理都经共享服务 `fengyegf.llm` 转发。
  原因很实际：Key、重试、超时、协议字段都归网关管，Agent 只负责「消息 ↔ 模型」的编排；
- **官方流式**：用 TDLib 1.8.70 的**草稿消息**（`sendRichMessageDraft` / `sendTextMessageDraft`）实现，和 Bot API
  `sendMessageDraft` 同一套语义——
  1. **网关流连通之后**再发**思考占位**（不是收到消息就发）：富草稿用 `inputPageBlockThinking` 块
     （文案固定 `Thinking…`，客户端渲染成思考态）；
  2. 同一个 `draft_id` 反复发草稿，客户端做**增量动画**，看起来就是逐段输出；
  3. 最后 `sendMessage` 发一条**富消息**（`inputMessageRichMessage`）落地，草稿自动消失（草稿是 30 秒临时预览）；
  4. **公式**：`$...$` 行内、独立 `$$` 块由富消息渲染成数学表达式；模型常用的
     `\(...\)` / `\[...\]` / 单行 `$$...$$` 会先归一（固定开启），
     ```math / ```latex 围栏也转成 `$$` 块，`\begin{align}` 等环境换成 `aligned`；
     流式途中停在半个公式上时跳过该帧，避免出现裸 `$`；
  4. 发出的消息**不引用用户消息**（官方演示里也没有回复引用；**收到**的引用回复见
     [引用回复](#引用回复用户点回复)）；
  5. 草稿默认挂「停止生成」按钮：用户点它时 TDLib 发 `updateStopMessageDraft`，插件立即停流、不发终稿。
  群聊没有草稿能力，直接发终稿。
  流式内容有两条路，自动选：
  1. **网关支持 `stream`**：`stream` 开流 → 每 50ms 拉增量 → 直接更新草稿。这是**真 token 流**，
     首字延迟就是模型首字延迟；
  2. **网关不支持 / 群聊 / 关掉 `stream_replies`**：同步 `chat` 拿终稿，再把终稿按块回放
     （视觉一致，但首字延迟 = 整段生成完）。
  两条路复用同一套 TDLib 草稿 API 与对外契约；
- **硬依赖**：`plugin.json` 声明 `depends`，代码里用 `Plugin::inject(["fengyegf.llm"])`；
  没装 / 没启用 llm 插件时本插件停在 Pending，不会出现「启动后每条消息只剩失败文案」。
  运行期上游失败仍按内置重试次数重试，最终回 `请求失败：<原因>`；`/agent status` 会显示网关是否在场；
- **协议字段不由本插件解释**：`gateway_method` 决定调网关的哪个方法，请求体按该方法拼，
  新增字段归网关实现。
- **工具**：工具定义与调用解析按 `gateway_method` 的协议形状渲染（三个协议键名不同），
  执行统一走宿主 `call_tool_result`，不自建 HTTP、不跨插件调服务；
  带工具的一轮退回「同步 + 回放」（原因见上一条的流式说明）。

## 构建、安装、打包

```bash
cargo build -p fuyubot-plugin-agent --offline            # 调试
cargo build -p fuyubot-plugin-agent --release            # 产物：target/release/agent.dll

fuyubot plugin install target/debug/agent.dll --plugin-dir plugins
python scripts/package.py --plugin agent --platform windows-x86_64
```

插件没有额外第三方依赖：只用 `serde` / `serde_json` / `thiserror` / `fuyubot-plugin-api`。

## 构建检查

```bash
cargo check -p fuyubot-plugin-agent --offline
cargo build --release -p fuyubot-plugin-agent
```

## 当前边界

- 推理是同步的：一个 `agent-worker` 线程串行处理消息，慢请求会排队；要并发得多开线程 / 分片，
  当前版本刻意不做（先保证会话一致与防环简单可推理）；
- 回放只验管线与请求形状：合成请求号只留档，真实发送、真实身份、真实网关返回都要真账号才验得了；
- 引用回复的正文得问 TDLib 要：被引用消息已删除 / 回放 / 无权限时只留 `[引用消息 #id]` 标记；
  富消息按块拍平，样式（粗体 / 表格边框）会退化，表格按 ` | ` 拼成一行；
- TDLib 应答现在**真的会等**：`send_message` / `edit_message_text` / `delete_messages` /
  `invoke` 都同步等 op 结果（之前被误判成离线、发完就不管），所以能拿到消息号、能发现发送失败；
  代价是每个 op 多一次等应答的时间（超时为内置常量：发送 30s / 辅助调用 15s）；
- 群里未点名默认丢弃；要「群聊随便聊」得关 `group_require_mention` 并接受模型被闲聊淹没；
- `/agent` 只认 `/` 前缀（见命令一节）：这是直接解析原始消息的代价，换来命令不经过宿主分发链；
- 媒体消息只把类型占位（`[photo]` 等）交给模型，不做 OCR / 语音转写——那是别的插件的能力；
- 工具结果超过 4000 字会转成 artifact：第 1 页回填，后续要模型自己调 `fengyegf.agent.result` 翻页 / 过滤；
  artifact 只在内存保留 10 分钟、进程重启即清空，超时后读取会报「不存在或已过期」；
- 读取工具被 `tool_denylist` 挡掉时退回普通截断，长结果的尾巴不可读；单条超过 2 MiB 的结果不缓存；
- 全量下发工具时模型可以调 `fengyegf.agent.*`（含 `.run`），可能自我递归，需要时用 `tool_denylist` 挡；
- `reply_mode = "rich"` 依赖宿主的富消息 simplify 能力，失败会逐级降级到 markdown / plain；
  真机遇到「占位不换、回复看不见」先切 `markdown` 排除富消息兼容性；
- 服务消息（话题创建 / 入群 / 置顶等）一律忽略，不推理、不建会话；
- 人类消息不做时间限速，只做去重；
- 连续消息合并会给单条消息引入最多 600ms 等待；连发消息按会话合并成一次请求，命令不参与合并。
