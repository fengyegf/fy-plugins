# AI Agent 运行时（`fengyegf.agent`）

Telegram 侧的 **AI Agent 运行时**：把私聊 / 群话题接上大模型，把回复发回 Telegram，
并把「对话能力」暴露成工具与共享服务。它**不是** LLM 网关——网关是 `fengyegf.llm`，
本插件只通过共享服务调它，自己不建 HTTP 客户端、不存 API Key。

- 插件 id：`fengyegf.agent`
- crate：`fuyubot-plugin-agent`（`cdylib` + `rlib`，lib name `agent`）
- ABI：v6
- 版本：`0.1.0`
- 软依赖：`fengyegf.llm`（网关不在场时降级为兜底文案，插件照常跑）

## 它负责什么、不负责什么

| 负责 | 不负责 |
| --- | --- |
| 监听 `updateNewMessage`、解析消息 / 话题 / 发送者 | 大模型协议（OpenAI / Anthropic / Responses） |
| 按 `chat + topic` 归档会话，JSON 落盘、重启不丢 | HTTP / SSE / 认证 / 重试 / Key 管理 |
| 提示词组装、单次 / 循环推理、历史裁剪 | 大模型协议 / HTTP / 鉴权 / Key（全归网关；本插件只拼会话） |
| 工具调用：宿主工具表按协议下发给模型，同步执行并把结果回填 | 工具本身的实现（工具由宿主 / 各插件注册，本插件只转发 JSON） |
| 防环：去重、Bot-to-Bot 频率与深度、白名单（人类消息不做限速） | 图片 / 语音等媒体理解（只把类型占位交给模型） |
| `/agent` 命令、4 个工具、共享服务 `fengyegf.agent` | 跨插件编排（只做被动调用方给出的 JSON） |

## 目录

```text
crates/fuyubot-plugin-agent/
├── Cargo.toml               # rlib + cdylib：既能被宿主加载，也能被 Rust 侧单测
├── plugin.json              # 分发包清单（id / entry / abi / provides / optional_depends）
├── config.default.toml      # 默认配置（最低优先级）
├── README.md
└── src/
    ├── lib.rs               # 插件壳：生命周期、state 指针、订阅 / 工具 / 服务装配
    ├── core/                # 纯数据与配置：不依赖宿主，方便单测
    │   ├── types.rs         # ChatKey / IncomingMessage / ChatMessage / TurnRequest…
    │   ├── config.rs        # Config：解析 [plugin.fengyegf.agent]，给全量默认值
    │   └── error.rs         # AgentError：稳定 code，工具 / 服务回包复用
    ├── td/                  # 唯一碰 TDLib 的地方（同步阻塞，只允许在 worker / 工具线程调）
    ├── agent/               # Agent 运行时：GatewayClient / PromptBuilder / Guard / AgentEngine
    │   ├── tools.rs         # 宿主工具表 → 三种协议的工具定义 + 同步执行
    │   └── toolcall.rs      # 工具调用 / 结果回填的协议形状（纯函数）
    ├── session/             # 会话与 JSON 落盘（原子写，按 chat + topic 一文件）
    ├── api/                 # 对外接口：5 个工具 + 共享服务 fengyegf.agent
    │   ├── mod.rs           # 注册 / 按方法名分发 / FFI 边界 / describe
    │   └── tools/           # 一个工具一个文件：chat / run / send / session / title
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
      → td::parse_incoming         纯函数解析成 IncomingMessage
      → is_outgoing / 服务消息过滤 / 命令 / 会话类型 / 群内 @ / bot 身份 / 防环 Guard
      → 命中 /agent → cmd::dispatch → 直接回复，结束（不建话题、不推理）
      → 私聊 + 无话题 + topic_auto_create → td::create_topic（新会话）
      → td::send_text_draft        空草稿 = 客户端自带的 Thinking… 占位
      → 网关支持 stream → stream + stream_poll：每块增量直接更新草稿（首字延迟真实下降）
        网关不支持 / 群聊 → AgentEngine::run 同步拿终稿，再按块回放草稿
      → 模型要调工具 → 同步执行宿主工具 → 结果回填再问一次（最多 max_tool_rounds 轮）
      → td::send_text / send_rich  正式消息落地，草稿自动消失；不引用原消息
      → session 追加落盘
```

两个设计点：

- **回调里绝不阻塞**：TDLib 调用全是同步等 response，放在订阅回调里会卡死宿主的接收循环，
  所以回调只投递，解析 / 准入 / 推理 / 发送都在 `agent-worker` 线程里做；
- **命令短路**：`/agent` 在准入之后、推理之前就返回，命令不会触发网关调用，也不会建会话话题
  （`/agent new` 除外，它显式建一个）。

## 配置

默认值在 `config.default.toml`，用户在自己的 `fuyubot.toml` 里覆盖；
三层优先级是「包默认 < `plugin-config/` 运行时落盘 < 用户配置」，插件只用 `HostExt::config_value` 读合并后的结果。

```toml
[plugin.fengyegf.agent]
gateway_service = "fengyegf.llm"
model = ""
reply_mode = "markdown"
topic_auto_create = true
```

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `true` | 总开关。关掉 = 不订阅、不注册工具 / 服务，插件仍在进程里可见 |
| `gateway_service` | `"fengyegf.llm"` | 网关共享服务名；为空回落默认值 |
| `gateway_method` | `"chat"` | 走网关的哪个方法（`chat` / `messages` / `responses`） |
| `model` | `""` | 空 = 用网关的默认模型 |
| `system_prompt` | 内置一句 | 系统提示词；留空则不注入 `system` 消息 |
| `format_guide` | `true` | 在系统提示后附加「Telegram 富消息支持的 Markdown 语法」说明（含公式写法），让模型输出可渲染的语法 |
| `max_history` | `20` | 每次请求带多少条历史（0 会被纠正成 20） |
| `max_tokens` | `1024` | 空 = 用网关默认 |
| `temperature` | `0.6` | 空 = 不下发该字段 |
| `model_tools` | `true` | 是否把宿主**全部工具**按协议下发给模型；关掉 = 纯文本对话（请求体不带 `tools`） |
| `max_tool_rounds` | `4` | 模型连续调用工具的最大轮数；用尽后再问一次且不再下发工具，逼它给结论 |
| `tool_denylist` | `[]` | 不下发的工具名：精确名或以 `*` 结尾的前缀（如 `fengyegf.agent.run`、`my.plugin.*`）；空 = 全部 |
| `tool_result_max_chars` | `4000` | 单个工具结果回填给模型的最大字符数，超出截断 |
| `reply_mode` | `"rich"` | 终稿形态：`plain` / `markdown` / `rich`（失败逐级降级）。rich = `inputMessageRichMessage` |
| `stream_replies` | `true` | 私聊用 TDLib 草稿消息做官方流式；关掉 = 直接发终稿 |
| `rich_drafts` | `true` | 流式草稿也用富消息（`sendRichMessageDraft`）；false 退回空文本草稿 |
| `thinking_text` | `"Thinking…"` | 富草稿里 `inputPageBlockThinking` 思考块的文案 |
| `stream_chunk_chars` | `30` | 流式草稿每次追加的字符数 |
| `stream_interval_ms` | `200` | 草稿更新间隔（毫秒，最小按 100 生效）；调大更省请求、防 429 |
| `stream_max_seconds` | `6` | 流式总时长上限；超过就自动放大块、快速补完 |
| `stream_can_stop` | `true` | 是否显示「停止生成」按钮（用户按下会收到 `updateStopMessageDraft`） |
| `gateway_stream` | `true` | 网关提供 `stream` 方法时用**真 token 流**更新草稿；不支持则自动退回分块回放 |
| `stream_timeout_ms` | `120000` | 一次流式的总超时 |
| `stream_idle_timeout_ms` | `30000` | 两次增量之间最多等多久，超时视为模型卡住 |
| `stream_poll_ms` | `50` | 流式轮询间隔（越小越跟手，调用越频繁） |
| `send_timeout_ms` | `30000` | 发送 / 编辑等应答的超时 |
| `action_timeout_ms` | `15000` | 话题 / 身份等辅助调用超时 |
| `allow_private` | `true` | 是否接私聊 |
| `allow_groups` | `true` | 是否接群 / 超级群 |
| `group_require_mention` | `true` | 群里必须 @ 我（或回复我的消息）才接 |
| `allow_bots` | `false` | Bot-to-Bot 总开关；默认关 |
| `bot_allowlist` | `[]` | 允许对话的对端 bot 用户名（不带 @）；空 = 只按 `allow_bots` |
| `bot_max_depth` | `2` | 单个会话里 bot 之间最多连续往返轮数 |
| `bot_rate_limit_secs` | `5` | 同一对端 bot 的最小回复间隔（秒） |
| `dedupe_window_secs` | `30` | 同一条消息的去重窗口（秒） |
| `topic_auto_create` | `true` | 私聊首条消息自动开话题（新会话） |
| `topic_title_max_chars` | `24` | 话题标题长度上限（首次聊天直接用首条消息内容作标题） |
| `topic_title_auto` | `true` | 新对话首次聊天时用首条消息设置话题标题，**不额外调用模型**；AI 想改名用工具 `fengyegf.agent.title` |
| `topic_icon_color` | `7321584` | 话题图标颜色（Telegram 只认六个值） |
| `session_dir` | `""` | 会话落盘目录；空 = `data/plugins/fengyegf.agent/sessions` |
| `session_max_sessions` | `500` | 会话文件数上限（修剪用） |
| `session_max_messages` | `200` | 单会话保留的消息条数上限 |
| `session_ttl_days` | `7` | 会话自动清理天数（启动时 + 每小时检查）；`0` = 永久保留，不自动删 |
| `commands` | `true` | 是否登记 `/agent` 命令 |
| `tools` | `true` | 是否注册 4 个工具 |
| `service` | `true` | 是否提供共享服务 `fengyegf.agent` |
| `retry_times` | `5` | 首次失败后的重试次数（0 = 不重试；总共尝试 `retry_times + 1` 次；配置类错误不重试） |
| `retry_delay_ms` | `800` | 两次重试之间的等待（毫秒） |
| `failure_text` | `"请求失败：{error}"` | 重试耗尽后发给用户的文案；`{error}` 换成失败原因 |
| `normalize_math` | `true` | 富消息前把 LaTeX 归一：`\(...\)`→行内 `$...$`、`\[...\]`/单行 `$$...$$`→独立 `$$` 块、```math/latex``` 围栏→`$$` 块、`align`→`aligned`；普通代码围栏不动 |
| `max_reply_chars` | `3500` | 单条回复字符上限（限幅 200–4000，Telegram 单条 4096） |
| `loop_max_turns` | `3` | 循环模式默认最多迭代轮数 |

## 对外契约

### 共享服务 `fengyegf.agent`

其它插件按名字调用（`host.call_service("fengyegf.agent", "chat", input)`），
入参 / 出参都是 JSON 对象。服务名 `fengyegf.agent`，一共 5 个方法：

| 方法 | 用途 |
| --- | --- |
| `chat` | 单次完成：带 `chat_id` 时按会话取历史并落盘（`RunMode::Single`、1 轮），不带则直通网关、不落历史 |
| `run` | 在某个会话上跑一轮（带历史、可循环迭代） |
| `send` | 主动往指定 chat / topic 发一条消息（markdown 优先富消息，失败逐级降级） |
| `session` | 会话管理：`list` / `get` / `reset` / `delete` / `create`（`get` 优先用 `id`，否则 `chat_id + topic_id`） |
| `describe` | 能力 / 配置自检（不联网、不发请求） |

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
| `max_turns` | `loop_max_turns`（3） | 循环上限；`single` 时固定 1 |

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

`action` 必填且只认 `list` / `get` / `reset` / `delete` / `create`（大小写宽松）。

| action | 必要定位 | 出参 |
| --- | --- | --- |
| `list` | 可选 `chat_id` | `{"ok":true,"sessions":[{"id":"7770001__12","chat_id":7770001,"topic_id":12,"title":"旅行计划","message_count":3,"updated_at":1760000000}]}`；不带 `chat_id` 列全部会话 |
| `get` | `chat_id`（+ `topic_id`）或 `id` | `{"ok":true,"session":{"id":"…","chat_id":…,"topic_id":…,"title":"…","created_at":…,"updated_at":…,"messages":[{"role":"user","content":"…","ts":…}],"meta":{}}}` |
| `reset` | `chat_id`（+ `topic_id`）或 `id` | `{"ok":true,"removed":3}`：清掉了多少条消息（会话本身保留） |
| `delete` | 同上 | `{"ok":true,"removed":1}`；`0` = 本来就不存在 |
| `create` | `chat_id`（+ `topic_id`） | `{"ok":true,"session":{…}}`；`title` 空时用「AI 会话」 |

#### `describe`：自检

```json
{
  "ok": true,
  "plugin": "fengyegf.agent",
  "version": "0.1.0",
  "gateway": {"service": "fengyegf.llm", "method": "chat", "available": false},
  "config": {"reply_mode": "markdown", "model": "", "max_history": 20, "allow_bots": false, "session_dir": "data/plugins/fengyegf.agent/sessions", "tools": true, "tool_count": 9, "tool_names": ["fengyegf.agent.chat", "…"], "tool_denylist": []},
  "sessions": 7
}
```

不发网络请求、不碰 TDLib：网关可用性来自宿主的服务清单，会话数来自本地目录，适合面板 / 诊断用。

### 工具（宿主 / AI / 其它插件）

| 工具 | 入参 | 说明 |
| --- | --- | --- |
| `fengyegf.agent.chat` | 同服务 `chat`；required：`prompt` | 单次完成 |
| `fengyegf.agent.run` | 同服务 `run`；required：`prompt` | 会话内一轮 |
| `fengyegf.agent.send` | 同服务 `send`；required：`chat_id`（`text` / `markdown` 至少一个） | 主动发送 |
| `fengyegf.agent.session` | 同服务 `session`；required：`action` | 会话查询 |
| `fengyegf.agent.title` | `chat_id` / `topic_id` / `title`（全部必填） | 重命名话题标题，并回写会话落盘标题 |

工具与服务**共用同一套 JSON 处理函数**：工具是同步返回的（`call_tool_result`），
服务是同步调用（`call_service`），两者入参 / 出参完全一致，不需要维护两份文档。

### 工具调用（模型 → 宿主工具）

模型不只会说话，也能**调工具**。每次请求都把宿主的**全部工具**（`HostApi::list_tools()`）
按当前协议下发给模型：OpenAI chat 用 `tools[].function{name,description,parameters}`、
Anthropic 用 `tools[].{name,description,input_schema}`、Responses 用 `tools[].{type:"function",…}`，
都带 `tool_choice: "auto"`。模型点了工具就同步执行（`HostApi::call_tool_result`），
把结果回填成下一条消息（chat: `role:"tool"`；messages: `tool_result` 块；
responses: `function_call_output`）再问一次，直到它给出文本。

- **开关**：`model_tools = true`（默认开）。关掉 = 纯文本对话，请求体里不带 `tools`；
- **轮数**：`max_tool_rounds = 4`。用尽后这一轮**不再下发工具**再问一次，模型没有工具可用，
  只能把结论写成文本；
- **屏蔽**：`tool_denylist = []` 默认全发。挡一个写精确名，挡一族写 `前缀*`。
  注意 `fengyegf.agent.*` 是**本插件自己**的工具，全量下发时模型可以反过来调它
  （`.run` 还会再跑一轮推理，可能套娃），需要时把它们列进 denylist；
- **失败也是内容**：工具报错回填成 `{"ok":false,"error":"…"}`，模型能换参数重试，
  不会让整轮请求失败；
- **截断**：结果超过 `tool_result_max_chars` 截断并注明，避免一个工具撑爆上下文；
- **历史**：工具往返**不落盘**，只有最终的 user / assistant 文本进会话，历史不被工具消息污染；
- **流式**：带工具的一轮**不走网关真流式**——流式通道只透传文本增量、看不到工具调用；
  自动退回「同步跑完 + 脚本分块回放」，观感一致，首字延迟变成整轮生成完。

## `/agent` 命令

登记进宿主命令表（`role = "user"`，默认 `private + group`），给 `/help` 用；真正的分派在
worker 里，只在它拦下后处理，不触发推理。

> 本插件订阅的是原始 `updateNewMessage`，命令文本由自己的 `core::parse_command` 解析，
> **只认 `/` 前缀**（`/agent …`）。宿主 `[command].prefix` 的归一化作用于宿主命令表，
> 不改变这里的解析——user 账户把前缀改成 `.` 时，`/agent` 仍然可用，`.agent` 不会进来。

| 子命令 | 别名 | 作用 |
| --- | --- | --- |
| `/agent help` | 空 / 未知子命令 | 用法说明 |
| `/agent new [标题]` | `open` | 只在私聊：建一个话题作为新会话；不给标题用「AI 会话」（群里发会提示只能在私聊开） |
| `/agent list` | `ls` | 列出当前 chat 的会话（本地 + TDLib 话题合并；TDLib 失败只列本地） |
| `/agent use <id>` | `switch` | 按 id 查看会话摘要（路由仍按 chat + topic 自动进行） |
| `/agent reset` | `clear` | 清空当前会话的历史，回复清了几条 |
| `/agent status` | `state` | 会话 id / 消息数 / 模型 / 回复形态 / 网关可用性 / 话题开关 |

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

1. **去重**：`dedupe_window_secs` 内同一条消息只处理一次（防 TDLib 重发 / 上游重复回调）；
2. **bot 频率与深度**：同一个对端 bot 至少间隔 `bot_rate_limit_secs`，连续往返不超过
   `bot_max_depth` 轮。

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
fuyubot plugin install target/debug/agent.dll --plugin-dir demo-plugins

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
  网关不在场 / 报错时回退兜底文案 → `草稿占位 #101` → `草稿流式完成（N 块）` →
  `回复已投递（markdown；…，流式）` → `会话 7770001__1 已回复（N 字）`；
- `插件发出去的请求：` 里能看到 `getUser`、`createForumTopic`、**空文本** `sendTextMessageDraft`
  （Thinking 占位）、分块 `sendTextMessageDraft`，以及一条**不带 `reply_to`** 的 `sendMessage`
  —— 这就是官方流式的形态：草稿先出现，正式消息最后落地；
- **不应该**因为等 response 卡满 `--replay-wait`：离线请求号（`>= 1 << 48`）不会等应答。

其余夹具（话题消息、bot 消息、命令、outgoing 噪声、群未点名）的期望现象逐条写在
[examples/replay/README.md](examples/replay/README.md)。

## 与 LLM 网关的边界

- **不自建 HTTP**：所有推理都经共享服务 `fengyegf.llm`（`gateway_service`）转发。
  原因很实际：Key、重试、超时、协议字段都归网关管，Agent 只负责「消息 ↔ 模型」的编排；
- **官方流式**：用 TDLib 1.8.70 的**草稿消息**（`sendRichMessageDraft` / `sendTextMessageDraft`）实现，和 Bot API
  `sendMessageDraft` 同一套语义——
  1. **网关流连通之后**再发**思考占位**（不是收到消息就发）：富草稿用 `inputPageBlockThinking` 块
     （文案 `thinking_text`，客户端渲染成思考态）；
     关掉 `rich_drafts` 时退回空文本草稿（客户端自带的 Thinking…）；
  2. 同一个 `draft_id` 反复发草稿，客户端做**增量动画**，看起来就是逐段输出；
  3. 最后 `sendMessage` 发一条**富消息**（`inputMessageRichMessage`）落地，草稿自动消失（草稿是 30 秒临时预览）；
  4. **公式**：`$...$` 行内、独立 `$$` 块由富消息渲染成数学表达式；模型常用的
     `\(...\)` / `\[...\]` / 单行 `$$...$$` 会先归一（`normalize_math`），
     ```math / ```latex 围栏也转成 `$$` 块，`\begin{align}` 等环境换成 `aligned`；
     流式途中停在半个公式上时跳过该帧，避免出现裸 `$`；
  4. 全程**不引用用户消息**（官方演示里也没有回复引用）；
  5. `can_stop = true` 时用户可点「停止生成」，TDLib 发 `updateStopMessageDraft`，插件立即停流、不发终稿。
  群聊没有草稿能力，直接发终稿。
  流式内容有两条路，自动选：
  1. **网关支持 `stream`（`fengyegf.llm` v0.1.1+）**：`stream` 开流 → 每 `stream_poll_ms`
     拉增量 → 直接更新草稿。这是**真 token 流**，首字延迟就是模型首字延迟；
  2. **网关不支持 / 群聊 / 关掉 `gateway_stream`**：同步 `chat` 拿终稿，再把终稿按块回放
     （视觉一致，但首字延迟 = 整段生成完）。
  两条路复用同一套 TDLib 草稿 API 与对外契约；
- **软依赖**：服务不在场时 `GatewayClient::available()` 为 false，主链路按 `retry_times` 重试后
  发 `failure_text`（失败原因给用户看，不再回"离线模式"文案），
  插件不崩、不阻塞其它插件；`/agent status` 会把「网关不在场」直接显示出来；
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
- 群里未点名默认丢弃；要「群聊随便聊」得关 `group_require_mention` 并接受模型被闲聊淹没；
- `/agent` 只认 `/` 前缀（见命令一节）：这是直接解析原始消息的代价，换来命令不经过宿主分发链；
- 媒体消息只把类型占位（`[photo]` 等）交给模型，不做 OCR / 语音转写——那是别的插件的能力；
- 工具结果按 `tool_result_max_chars` 截断，大 JSON / 长文本会掉尾巴；
- 全量下发工具时模型可以调 `fengyegf.agent.*`（含 `.run`），可能自我递归，需要时用 `tool_denylist` 挡；
- `reply_mode = "rich"` 依赖宿主的富消息 simplify 能力，失败会逐级降级到 markdown / plain；
  真机遇到「占位不换、回复看不见」先切 `markdown` 排除富消息兼容性；
- 服务消息（话题创建 / 入群 / 置顶等）一律忽略，不推理、不建会话；
- 人类消息不做时间限速，只做去重（`rate_limit_secs` 已移除）。
