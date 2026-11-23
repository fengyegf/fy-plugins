# LLM Gateway（`fengyegf.llm`）

面向 **AI Agent** 的大模型协议网关：一个 FuyuBot 原生插件，把三个常用协议分别封装在三个目录里，
**不依赖任何 AI SDK**——HTTP、SSE、认证、重试、超时全部自己实现，只依赖 `serde` / `ureq`。

- 插件 id：`fengyegf.llm`
- crate：`fuyubot-plugin-llm`（`cdylib` + `rlib`，lib name `llm`）
- ABI：v6
- 版本：`0.1.0`

## 为什么目录和常规插件不一样

常规插件按 `host / cmd / core / render` 分层；这个插件的核心是**三份互相独立的协议实现**，
所以先按协议分目录，再在共享层收敛传输细节：

```text
crates/fuyubot-plugin-llm/
├── Cargo.toml               # cdylib + rlib：既能被宿主加载，也能被其它 Rust 代码依赖
├── plugin.json              # 分发包清单（id / entry / abi / provides / config）
├── config.default.toml      # 插件默认配置（最低优先级）
├── README.md
├── examples/
│   └── probe.rs             # 命令行测试接口：三个协议 + models + test
└── src/
    ├── lib.rs               # crate 文档与导出
    ├── api.rs               # LlmClient：统一门面 / JSON 直通 / 自检 / describe
    ├── core/                # 与协议无关
    │   ├── config.rs        # ClientConfig：base_url / key / 超时 / 重试 / 路径
    │   ├── error.rs         # LlmError：OpenAI / Anthropic / 网络 / SSE 错误归一
    │   ├── http.rs          # Transport：ureq 封装、认证头、重试、同步 / 流式
    │   └── sse.rs           # SSE 解析器（多行 data、CRLF、注释、[DONE]）
    ├── openai_chat/         # 协议 1：OpenAI Chat Completions
    │   ├── mod.rs           # Client
    │   ├── types.rs         # 请求 / 响应 / 消息 / 工具 / 用量
    │   └── stream.rs        # 流式 chunk + aggregate()
    ├── openai_responses/    # 协议 2：OpenAI Responses
    │   ├── mod.rs           # Client（含 retrieve / delete）
    │   ├── types.rs         # 输入项 / 输出项 / 工具 / 事件
    │   └── stream.rs        # 流式事件 + aggregate()
    ├── anthropic_messages/  # 协议 3：Anthropic Messages
    │   ├── mod.rs           # Client（含 count_tokens）
    │   ├── types.rs         # 内容块 / 工具 / 用量
    │   └── stream.rs        # 流式事件 + aggregate()
    └── plugin.rs            # 宿主插件壳：工具 + 共享服务
```

安装到宿主后的标准分发包布局（ZIP 根就是插件目录）：

```text
plugins/fengyegf.llm/
├── plugin.json
├── config.default.toml
└── bin/
    └── llm.dll            # Linux: libllm.so / macOS: libllm.dylib
```

## 接入地址与认证

默认根地址：`https://api.deepseek.com`（以 DeepSeek 的 OpenAI 兼容入口为例；Responses / Anthropic 请按厂商改 `base_url` 或对应路径）

| 协议 | 方法 + 路径 | 认证头 |
| --- | --- | --- |
| OpenAI Chat Completions | `POST /v1/chat/completions` | `Authorization: Bearer <API Key>` |
| OpenAI Responses | `POST /v1/responses` | `Authorization: Bearer <API Key>` |
| Anthropic Messages | `POST /v1/messages` | `x-api-key: <API Key>` + `anthropic-version: 2023-06-01` |

三个路径都可以在配置里改；`base_url` 只写到根（OpenAI 官方是 `/v1`），不要带具体端点。

### HTTPS

支持。`base_url` 可以写 `http://` 或 `https://`，TLS 由 `ureq` 默认的 rustls 提供，
证书走 `webpki-roots`，不需要额外依赖或系统 OpenSSL。

```toml
# DeepSeek 的 OpenAI 兼容端点（Chat Completions）
[plugin.fengyegf.llm]
base_url = "https://api.deepseek.com"
api_key = "sk-…"
chat_completions_path = "/chat/completions"
```

```toml
# OpenAI 官方
base_url = "https://api.openai.com/v1"
chat_completions_path = "/chat/completions"
responses_path = "/responses"

# Anthropic 官方（x-api-key + anthropic-version 由 Transport 自动加）
base_url = "https://api.anthropic.com/v1"
messages_path = "/messages"
```

`base_url` 是三个协议共享的；如果三个协议要指向**不同厂商**，把对应路径写成绝对 URL 即可
（`ClientConfig::endpoint` 对绝对 URL 原样使用）：

```toml
base_url = "https://api.anthropic.com/v1"              # Anthropic 走官方端点
chat_completions_path = "https://api.deepseek.com/chat/completions"
api_key = "sk-…"                                       # 注意：三个协议共用一个 Key
```

> 一个 `LlmClient` / 一份插件配置对应一个 Key。要在同一进程里用不同厂商的 Key，
> 在 Rust 侧分别构造多个 `LlmClient`（内部 API），不要指望一份插件配置同时持有多个 Key。

## 功能覆盖

### OpenAI Chat Completions

- 消息：`system` / `developer` / `user` / `assistant` / `tool` / `function`；
  `content` 支持字符串或 parts（`text` / `image_url` / `input_audio` / `file`）；
  assistant 的 `tool_calls` / `function_call` / `refusal` / `audio`。
- 参数：`temperature` / `top_p` / `n` / `stop` / `seed` / `logprobs` / `top_logprobs` /
  `logit_bias` / `presence_penalty` / `frequency_penalty` / `max_tokens` /
  `max_completion_tokens` / `response_format` / `tools` / `tool_choice` /
  `parallel_tool_calls` / `stream` / `stream_options` / `reasoning_effort` /
  `modalities` / `audio` / `prediction` / `store` / `metadata` / `service_tier` /
  `user` / `verbosity` / `web_search_options`。
- 响应：`choices` / `message` / `finish_reason` / `logprobs` / `usage`（含
  `prompt_tokens_details` / `completion_tokens_details`）/ `system_fingerprint`。
- 流式：`choices[].delta`（`content` / `tool_calls` / `reasoning_content` / `refusal`）、
  `finish_reason`、`usage`（`stream_options.include_usage`）、`data: [DONE]`。
- 未建模字段一律经 `extra` 透传，不丢。

### OpenAI Responses

- 输入：字符串或 items 数组（`message` / `input_text` / `input_image` / `input_file` /
  `item_reference` / `function_call` / `function_call_output` / `reasoning` 等）。
- 参数：`instructions` / `max_output_tokens` / `temperature` / `top_p` / `top_logprobs` /
  `tools` / `tool_choice` / `parallel_tool_calls` / `reasoning` / `text` / `truncation` /
  `previous_response_id` / `store` / `metadata` / `include` / `background` / `stream` /
  `user` / `service_tier` / `prompt` / `conversation`。
- 工具：`function` / `web_search_preview` / `file_search` / `computer_use_preview` /
  `code_interpreter` / `image_generation` / `mcp`。
- 输出项：`message` / `function_call` / `reasoning` / `web_search_call` /
  `file_search_call` / `computer_call` / `image_generation_call` /
  `code_interpreter_call` / `mcp_call` / `mcp_list_tools` / `mcp_approval_request`。
- 流式事件：`response.created` / `response.in_progress` / `output_item.added` /
  `output_item.done` / `content_part.added` / `content_part.done` /
  `output_text.delta` / `output_text.done` / `function_call_arguments.delta` /
  `function_call_arguments.done` / `reasoning_summary_text.delta` /
  `reasoning_summary_text.done` / `response.completed` / `response.failed` / `error`。
- 额外端点：`retrieve(id)`（`GET /responses/{id}`）、`delete(id)`（`DELETE /responses/{id}`）。

### Anthropic Messages

- 消息：`user` / `assistant`；内容块 `text` / `image`（base64 或 URL） / `document` /
  `tool_use` / `tool_result` / `thinking` / `redacted_thinking` / `server_tool_use` /
  `web_search_tool_result`。
- 参数：`max_tokens`（必填）/ `system`（字符串或 block 数组）/ `metadata` /
  `stop_sequences` / `stream` / `temperature` / `top_k` / `top_p` / `tools` /
  `tool_choice` / `thinking` / `service_tier`；`cache_control` 可透传。
- 工具：自定义工具（`input_schema`）与内置工具（`computer` / `bash` / `text_editor` /
  `web_search`）。
- 响应：`content` / `stop_reason` / `stop_sequence` / `usage`（含
  `cache_creation_input_tokens` / `cache_read_input_tokens` / `server_tool_use`）。
- 流式事件：`message_start` / `content_block_start` / `content_block_delta` /
  `content_block_stop` / `message_delta` / `message_stop` / `ping` / `error`；
  delta 含 `text_delta` / `input_json_delta` / `thinking_delta` / `signature_delta` 等。
- 额外端点：`count_tokens(&request)`（`POST /messages/count_tokens`）。

## 配置

默认值在 `config.default.toml`，用户在自己的 `fuyubot.toml` 里覆盖：

```toml
[plugin.fengyegf.llm]
base_url = "https://api.deepseek.com"
api_key = "sk-…"                 # 也可以只设环境变量 FUYUBOT_LLM_API_KEY
default_model = "deepseek-chat"
default_max_tokens = 1024

# 可选
timeout_secs = 120
connect_timeout_secs = 15
read_timeout_secs = 300
max_retries = 2
anthropic_version = "2023-06-01"
enable_tools = true
enable_service = true

[plugin.fengyegf.llm.headers]
X-Tenant = "demo"
```

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `base_url` | `https://api.deepseek.com` | 服务根地址，不带端点后缀 |
| `api_key` | 空 | 为空时回退 `FUYUBOT_LLM_API_KEY` |
| `default_model` | `deepseek-chat` | 请求没带 `model` 时补齐 |
| `default_max_tokens` | `1024` | 请求没带上限时补齐（按协议填不同字段） |
| `timeout_secs` | `120` | 普通请求总超时 |
| `connect_timeout_secs` | `15` | 连接超时 |
| `read_timeout_secs` | `300` | 流式单次读超时 |
| `max_retries` | `2` | 429 / 5xx / 网络错误重试次数 |
| `chat_completions_path` | `/chat/completions` | 可改 |
| `responses_path` | `/responses` | 可改 |
| `messages_path` | `/messages` | 可改 |
| `models_path` | `/models` | 可改 |
| `headers` | 空 | 追加到每个请求的自定义头 |
| `debug` | `false` | 往 stderr 打请求 / 响应摘要（不打 Key） |

## 内部调用（Rust）

crate 同时产出 `rlib`，其它 crate 可以直接 `use llm::…`：

```rust
use llm::{ClientConfig, LlmClient, Protocol};
use serde_json::json;

let client = LlmClient::new(ClientConfig::new(
    "https://api.deepseek.com",
    "sk-…",
))?;

// 1) JSON 直通：请求体就是协议原生 body，覆盖所有字段
let value = client.request_json(
    Protocol::OpenAiChat,
    &json!({
        "model": "deepseek-chat",
        "messages": [{ "role": "user", "content": "你好" }],
        "max_tokens": 256,
    }),
)?;
println!("{}", value);

// 2) 类型化客户端
use llm::openai_chat::{ChatCompletionRequest, ChatMessage};
let request = ChatCompletionRequest {
    model: "deepseek-chat".into(),
    messages: vec![ChatMessage::user("你好")],
    max_tokens: Some(256),
    ..Default::default()
};
let response = client.openai_chat().create(&request)?;
println!("{:?}", response.choices);

// 3) 流式 + 聚合
let mut stream = client.request_stream(Protocol::OpenAiChat, &json!({
    "model": "deepseek-chat",
    "messages": [{ "role": "user", "content": "你好" }],
    "stream": true,
}))?;
for event in stream.by_ref() {
    println!("{}", event?);
}
```

- `LlmClient::from_env()`：从 `FUYUBOT_LLM_*` 环境变量构造；
- `LlmClient::test(model)`：三个协议各发一个最小请求，返回 `TestReport`；
- `LlmClient::describe()`：不联网的能力清单；
- 三个类型化客户端：`openai_chat()` / `openai_responses()` / `anthropic_messages()`。

## 外部调用

### 工具（宿主 / AI / 其它插件）

| 工具 | 入参 | 说明 |
| --- | --- | --- |
| `fengyegf.llm.chat` | Chat Completions 原生 body | 同步返回上游 JSON |
| `fengyegf.llm.responses` | Responses 原生 body | 同步返回上游 JSON |
| `fengyegf.llm.messages` | Messages 原生 body | 同步返回上游 JSON |
| `fengyegf.llm.models` | `{}` | `GET /models` |
| `fengyegf.llm.test` | `{"model":"…"}` | 三协议自检（延迟 / 文本 / 错误） |
| `fengyegf.llm.describe` | `{}` | 协议、端点、认证、默认模型、工具清单 |

### 共享服务（其它插件）

服务名 `fengyegf.llm`，方法 `chat` / `responses` / `messages` / `models` / `test` /
`describe`，入参与工具相同：

```rust
let raw = host.call_service(
    "fengyegf.llm",
    "chat",
    &serde_json::json!({
        "model": "deepseek-chat",
        "messages": [{ "role": "user", "content": "你好" }]
    })
    .to_string(),
)?;
let value: serde_json::Value = serde_json::from_str(&raw)?;
```

出参形状：

```json
{ "ok": true, "protocol": "openai-chat", "data": { "id": "…", "choices": [] } }
```

失败：

```json
{ "ok": false, "kind": "api", "status": 401, "message": "…", "type": "…", "code": "…" }
```

### 插件依赖声明

### 流式服务（stream / stream_poll / stream_cancel）

`chat` 这类方法是同步 JSON（一次返回终稿），真流式单独一组方法：

| 方法 | 入参 | 出参 |
| --- | --- | --- |
| `stream` | 同 `chat`（可选 `protocol`） | `{"ok":true,"stream_id":1,"protocol":"openai-chat"}`，立刻返回，读线程在后台读 SSE |
| `stream_poll` | `{"stream_id":1,"cursor":0}` | `{"ok":true,"chunks":["你好", …],"cursor":3,"done":false,"error":null,"model":"…","usage":{…}}` |
| `stream_cancel` | `{"stream_id":1}` | `{"ok":true,"cancelled":true}` |

- 每次 `stream_poll` 只拿 `cursor` 之后的新块；`done` 为 true 后登记会被移除。
- 错误走 `error: {code, message}`；`cancelled: true` 表示被 `stream_cancel` 或插件停止中断。
- 为什么轮询而不是事件：服务契约只有同步 JSON 一种，跨插件不依赖 emit 作用域；
  调用方自己掌握节奏（`fengyegf.agent` 默认 50ms 一轮）。
- 工具（`fengyegf.llm.chat` 等）仍是同步的，传 `stream: true` 会被拒绝。

调用方典型流程：`stream` 拿 id → 循环 `stream_poll` 拼 chunks → `done` 后自己落地
（agent 是发 `sendMessage`）→ 用户点停止就 `stream_cancel`。

其它插件要用共享服务时，按 FuyuBot 的分发包规范二选一：

- 在对方 `plugin.json` 写 `"optional_depends": ["fengyegf.llm"]`（软依赖，只影响排序）；
- 或在对方插件代码里 `impl Plugin` 覆盖 `fn soft_inject() -> vec!["fengyegf.llm"]`。

本插件的 `plugin.json` 声明了 `provides: ["fengyegf.llm"]`。

## 测试接口

### 1. 命令行 probe（推荐先跑这个）

```bash
export FUYUBOT_LLM_BASE_URL=https://api.deepseek.com
export FUYUBOT_LLM_API_KEY=sk-…

# 三协议自检
cargo run -p fuyubot-plugin-llm --example probe -- --protocol test

# 单协议
cargo run -p fuyubot-plugin-llm --example probe -- --protocol chat      --prompt "你好"
cargo run -p fuyubot-plugin-llm --example probe -- --protocol responses --prompt "你好"
cargo run -p fuyubot-plugin-llm --example probe -- --protocol messages  --prompt "你好"

# 流式
cargo run -p fuyubot-plugin-llm --example probe -- --protocol chat --prompt "你好" --stream

# 直接喂协议原生 body（tools / vision / 结构化输出等）
cargo run -p fuyubot-plugin-llm --example probe -- --protocol chat --json body.json

# 模型列表
cargo run -p fuyubot-plugin-llm --example probe -- --protocol models
```

### 2. 插件内自检工具

装进宿主后调用 `fengyegf.llm.test`（或服务 `fengyegf.llm.test`），
返回每个协议的 `ok` / `latency_ms` / `text` / `error`，不需要账号、不需要发消息。

### 3. curl 对照

```bash
# OpenAI Chat Completions（DeepSeek 官方 OpenAI 兼容入口）
curl -sS https://api.deepseek.com/chat/completions \
  -H "Authorization: Bearer $DEEPSEEK_KEY" -H "Content-Type: application/json" \
  -d '{"model":"deepseek-chat","messages":[{"role":"user","content":"pong"}],"max_tokens":64}'

# OpenAI Responses（OpenAI 官方）
curl -sS https://api.openai.com/v1/responses \
  -H "Authorization: Bearer $OPENAI_KEY" -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o-mini","input":"pong","max_output_tokens":64}'

# Anthropic Messages（Anthropic 官方）
curl -sS https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_KEY" -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-3-5-sonnet-latest","max_tokens":64,"messages":[{"role":"user","content":"pong"}]}'
```

## 构建、安装、打包

```bash
# 编译（workspace 根）
cargo build -p fuyubot-plugin-llm --release
# 产物：target/release/llm.dll（Linux libllm.so / macOS libllm.dylib）

# 装进项目（目录或产物）
fuyubot plugin install target/release/llm.dll --config fuyubot.toml

# 打商城包（ZIP 根 = 插件目录内容）
python scripts/package.py --plugin llm --platform windows-x86_64
```

## 安全

- 仓库里**不写 Key**：`config.default.toml` 的 `api_key` 为空，部署时写到用户配置或环境变量；
- `describe` 只返回打码后的 Key；
- `debug = true` 只打印方法 / URL / 状态 / 字节数，不打印请求体与 Key；
- 工具 / 服务的返回值是进程内 JSON，不经过日志。

## 当前边界

- 本次重点在**协议对接与封装**，插件壳只做工具 / 服务注册，没有命令、卡片、消息链路；
- 工具是同步 JSON 调用（宿主工具表没有流式通道）；流式只在 Rust 库 API 与 probe 里提供；
- 未建模字段虽然能透传，但类型化访问需要后续补字段（`extra` 永远可用）；
- 默认值只是示例（DeepSeek Chat）；模型名由上游决定，Responses / Anthropic 需要按厂商配置。
