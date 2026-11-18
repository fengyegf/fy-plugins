# RustO OCR 插件

纯 Rust 的图片文字识别插件：**图片进、文本出**。用户发图 + `/ocr` 就能拿到识别
文本；同时把识别能力以进程级共享服务 `rusto.ocr`（v1）和工具 `rusto.ocr` 暴露
给其它插件，做到「别的插件把图丢进来，拿文本走」。

引擎是 [RustO!](https://github.com/byrizki/rusto-rs)（`rusto-rs` 0.4.1 + 纯 Rust
的 MNN 运行时 `rusto-nn`），模型是 PP-OCRv6 Tiny 的 det / rec / dict 三件套
（Apache-2.0）。没有 Python、OpenCV、ONNX Runtime，也不起下载子进程；一个 cdylib
装下全部能力。

## 定位

- **图像 → 文本**：`/ocr` 是默认入口。回复图片、图片说明里写命令、或私聊直发图
  （`auto = true`）三种方式命中后回复纯文本，适合截图、聊天记录、票据这类
  「有字想拿走」的场景。
- **跨插件联动**：共享服务 `rusto.ocr` 让兄弟插件用 `call_service` 同步拿 JSON；
  工具 `rusto.ocr` 给宿主 / AI 做能力发现。识别结果按行给，`detail` 打开时带
  行级置信度与四点坐标，字幕烧录、翻译对齐、笔记摘录都能直接用。
- **不抢图**：默认 `auto = false`，必须显式触发，避免每条图片消息都烧一次 CPU。
- **可换引擎**：引擎收口在 `OcrBackend` trait（`src/engine/`），以后接云端 OCR /
  `ocrs` / mock 只加一个 impl，命令层与服务协议都不用动。

## 命令

| 命令 | 说明 |
| --- | --- |
| `/ocr` | 识别图片文字（对图片消息回复，或图片说明里写命令） |
| `/ocr detail` | 识别并附行级置信度与坐标（别名 `/ocr d`、`/ocr 详细`） |
| `/ocr status` | 引擎状态：是否已加载、模型、并发、最近耗时（别名 `/ocr 状态`） |
| `/ocr warm` | 预加载模型（首次部署排障用，别名 `/ocr 预热`） |
| `/ocr path <本机路径>` | 识别本机图片，不经过 Telegram 下载（排障用） |
| `/ocr help` | 用法提示（别名 `/ocr -h`、`/ocr 帮助`） |

命令前缀与仓库其它插件一致：`/`、`!`、`！`、`.`、`#`；命令名是 `ocr`，也接受
插件名 `rusto`。触发方式三选一：

1. 对一条图片消息**回复** `/ocr`；
2. 图片的 caption 以命令开头（例如 `/ocr 看看这段`）；
3. 私聊直接发图，且 `[plugin.rusto].auto = true`（默认关）。

回复形态：默认先回一条「识别中…」占位消息，识别完编辑成结果
（`reply_placeholder = true`）；文本超过 `chunk_size` 自动分块，多段带「（1/N）」。
`/ocr detail` 额外附行数、耗时和最多 40 行的行级置信度。没有图会回复用法提示；
模型缺失会直接告诉你缺哪个文件、应该放到哪里。

## 配置 `[plugin.rusto]`

```toml
[plugin.rusto]
auto = false
max_side = 960
concurrency = 1
rec_workers = 1
model = "ppocrv6-tiny-int8"
model_dir = ""
mirror = ""
detail_default = false
reply_placeholder = true
chunk_size = 3800
timeout_ms = 20000
```

| 键 | 默认 | 范围 | 说明 |
| --- | --- | --- | --- |
| `auto` | `false` | bool | 私聊直接发图是否自动识别。默认关，只认显式命令 |
| `max_side` | `960` | 320–4096 | 识别前长边限幅（像素）。实测 960 档约 1.1 s、640 档约 0.45 s；越大越准也越慢 |
| `concurrency` | `1` | 1–8 | 同时识别数。单引擎峰值内存约 300–400 MiB，默认串行 |
| `rec_workers` | `1` | 0–8 | 引擎内部识别线程数。默认 1；设 0 会让 rusto-nn 按 CPU 核数起常驻线程（16 核 = 15 个） |
| `model` | `"ppocrv6-tiny-int8"` | 预设名 | 模型预设，同时是模型目录名（见「模型」一节） |
| `model_dir` | `""` | 路径 | 模型根目录；空 = 先找宿主数据目录，再找内置 `models` |
| `mirror` | `""` | URL | 模型下载源前缀；空 = HuggingFace 官方源 |
| `detail_default` | `false` | bool | `/ocr` 默认是否带行级明细 |
| `reply_placeholder` | `true` | bool | 是否先回「识别中…」再编辑为结果 |
| `chunk_size` | `3800` | 200–4000 | 单条回复最大字符数（Telegram 上限 4096 留余量） |
| `timeout_ms` | `20000` | 1000–120000 | 单次识别总预算（下载 + 排队 + 推理） |

配置读两处，后者覆盖前者：插件 `start()` 收到的配置 JSON → 宿主的
`config_value("rusto", …)`。缺键、类型不对、超范围（例如 `max_side = 99999`）都会
退回缺省值或夹到边界。宿主没有配置热重载通道，改完 `fuyubot.toml` 要重载插件。

## 共享服务 `rusto.ocr`（v1）

插件加载后通过 `provide_service_shared` 注册进程级共享服务 `rusto.ocr`，协议版本
`1`。服务方法跑在**调用方线程**上，是同步调用：小图直接拿结果，大图或批量请用
`async: true`。

### `recognize`

入参（字段只增不改，三选一给图片）：

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `file_id` | integer | — | TDLib 文件 id，优先级最高（其它插件最常用的形态） |
| `path` | string | — | 本机图片路径（插件进程可读） |
| `data` | string | — | data URL 或裸 base64，**原始字节 ≤ 8 MiB**，超了回 `too_large` |
| `detail` | boolean | `detail_default` | 是否返回行级 `text` / `score` / `box` |
| `max_side` | integer | `max_side` | 长边限幅，320–4096 |
| `timeout_ms` | integer | `timeout_ms` | 本次预算，1000–120000 |
| `async` | boolean | `false` | 只服务通道支持：立即回 `task_id`，结果走事件 `rusto.ocr.done` |

成功出参：

```json
{
  "ok": true,
  "text": "第一行\n第二行",
  "elapsed_ms": 412,
  "truncated": false,
  "lines": [
    { "text": "第一行", "score": 0.97, "box": [[12.0, 40.5], [300.2, 41.0], [300.0, 80.1], [11.8, 79.6]] }
  ],
  "engine": { "backend": "rusto", "model": "ppocrv6-tiny-int8", "rec_workers": 1 }
}
```

- `text`：按行拼接的纯文本，可以直接回复；
- `lines`：仅 `detail: true` 时非空；`box` 是四点坐标（左上、右上、右下、左下），
  已换算回原图尺度；
- `truncated`：文本超长被截断（保护用，正常情况为 `false`）。

`async: true` 时立刻回 `{"ok":true,"task_id":7,"pending":true}`，识别完成后宿主收到
事件 `rusto.ocr.done`，负载是 `{"task_id":7,"result":{…同上…}}`。

### `warm`

入参忽略，出参 `{"ok":true,"loaded":true}`；失败时是统一的 `{ok:false,code,message}`。

### `status`

```json
{
  "ok": true,
  "loaded": true,
  "backend": "rusto",
  "model": "ppocrv6-tiny-int8",
  "model_config": "ppocrv6-tiny-int8",
  "concurrency": 1,
  "active": 0,
  "rec_workers": 1,
  "last_elapsed_ms": 412
}
```

`loaded` 表示引擎是否已成功加载；`model` 是后端实际用的模型，`model_config` 是配置
里的预设名；`active` / `concurrency` 看排队；`last_elapsed_ms` 是最近一次识别耗时。

### 错误码

失败统一返回 `{"ok": false, "code": "...", "message": "..."}`：

| `code` | 触发场景 | 调用方怎么处理 |
| --- | --- | --- |
| `bad_input` | 三个图片字段都没给、不是合法 JSON、base64 解不开、`file_id` 下载失败 | 检查入参，换 `path` / `data` 重试 |
| `too_large` | `data` 原始字节 > 8 MiB | 改成传 `file_id`，或自己缩图再传 |
| `model_missing` | det / rec / dict 缺文件；`message` 里带绝对路径 | 放模型 / 首启下载 / 用离线包 |
| `engine_unavailable` | 模型加载失败、后端不可用 | 看插件日志里的具体原因；通常仍是模型问题 |
| `engine_busy` | 并发闸门排满，等待超预算 | 稍后重试，或调大 `concurrency` |
| `timeout` | 识别超时 | 调大 `timeout_ms` / 降 `max_side` / 降并发 |
| `internal` | 其它内部错误（含 panic 兜底） | 报 bug，附带 `message` |

调用方示例（另一个插件里）：

```text
call_service("rusto.ocr", "recognize", {"file_id": 123456789, "detail": true, "timeout_ms": 30000})
```

> 服务是进程级共享的：rusto 插件卸载后服务自动回滚。没装 rusto 时调用会失败，
> 需要「有 OCR 就用」的插件请按软依赖处理，别写成硬依赖。

## 工具 `rusto.ocr`

给宿主 / AI 发现与调用，schema 与服务入参同构（少 `async`），`timeout_ms` 默认
30000，`idempotent: true`：

```json
{
  "name": "rusto.ocr",
  "version": "0.1.0",
  "description": "识别图片中的文字（纯 Rust OCR / PP-OCRv6）：给 file_id、本机路径或 base64 图片，返回文本与行级明细",
  "input_schema": {
    "type": "object",
    "properties": {
      "file_id": { "type": "integer", "description": "TDLib 文件 id（优先）" },
      "path": { "type": "string", "description": "本机图片路径" },
      "data": { "type": "string", "description": "data URL 或 base64，原始 ≤ 8 MiB" },
      "detail": { "type": "boolean", "description": "是否返回行级 text/score/box" },
      "max_side": { "type": "integer", "description": "长边限幅，默认 960" },
      "timeout_ms": { "type": "integer", "description": "预算，默认 20000" }
    },
    "anyOf": [{ "required": ["file_id"] }, { "required": ["path"] }, { "required": ["data"] }]
  },
  "output_schema": {
    "type": "object",
    "properties": {
      "ok": { "type": "boolean" },
      "text": { "type": "string" },
      "elapsed_ms": { "type": "integer" },
      "lines": { "type": "array" },
      "code": { "type": "string" },
      "message": { "type": "string" }
    }
  },
  "idempotent": true,
  "timeout_ms": 30000
}
```

工具是同步调用，不支持 `async`；结果结构、错误码与服务完全一致。

## 模型

识别需要三个文件：`det`（文本检测）、`rec`（文本识别）、`dict`（字符字典）。
目录约定是 `<model_dir>/<preset>/`，`model_dir` 留空时依次尝试「宿主进程工作目录
下的 `data/plugins/rusto/models`」和「内置 `models`」。

默认模型是 **PP-OCRv6 Tiny INT8**（HuggingFace `byrizki/rusto-models`，Apache-2.0）：

| 文件 | 体积 | 说明 |
| --- | --- | --- |
| `ppocrv6-tiny-int8/det.mnn` | 543,168 B | 文本检测（MNN 格式，rusto-nn 原生 kernel） |
| `ppocrv6-tiny-int8/rec.mnn` | 2,912,240 B | 文本识别 |
| `ppocrv6-tiny-int8/dict.txt` | 27,156 B | 字符字典（中英 + 拉丁，约 6904 字） |

合计约 **3.5 MB**。这个预设目录里 det / rec 两套格式都在：`.mnn`（rusto-nn 原生
MNN kernel，最快）和 `.rten`（通用解释器，能跑但慢）；插件、脚本都按「先
`.mnn`、没有再退 `.rten`」处理。默认选 INT8 是因为它比 FP32 的
`ppocrv6-tiny`（约 6.2 MB）更小更快，精度对截图 / 聊天记录这个场景够用。

### 方式一：首启自动下载（默认）

第一次识别时按 `model` 找模型，缺了就按 HuggingFace 直链下载到缓存，之后零网络。
要求插件进程能访问 HuggingFace；国内网络建议配 `mirror`：

```toml
[plugin.rusto]
mirror = "https://hf-mirror.com"
```

下载失败会以 `model_missing` / `engine_unavailable` 报出来，不会静默。

### 方式二：离线包（国内 / 无网环境推荐）

把模型拉进 crate，再和 dll 一起打进 bundle：

```powershell
python scripts/fetch-models.py
python scripts/package.py --plugin rusto --platform windows-x86_64
```

`package.py` 会**自动**把 crate 根的 `models/` 与 `NOTICE.md` 打进包，所以这里
不传 `--assets` 也是离线包；要额外带别的文件才用 `--assets`。产物
`dist/rusto-0.1.0-windows-x86_64.zip` 里有 `plugin.json`、`rusto.dll`、
`models/ppocrv6-tiny-int8/{det.mnn,rec.mnn,dict.txt}` 和 `NOTICE.md`，解压即用。
实测 `rusto.dll` 约 3.2 MB + 模型约 3.5 MB，打好的 zip 约 4.2 MB（已压缩），离商城 50 MB 上限还很远。

装好后把 `model_dir` 指向解压出来的 `models` 目录即可（默认预设已经是
`ppocrv6-tiny-int8`，不用改 `model`；ABI 没有「插件自身目录」接口，建议写绝对路径）：

```toml
[plugin.rusto]
model_dir = "C:/fuyubot/plugins/rusto/models"
```

完整的在线 / 离线两条发布流程见 [`BUNDLE.md`](BUNDLE.md)。

### 方式三：`model_dir` 指已有目录

已经有模型（别的插件、别的机器拷过来的、自己转换的）时，只要按
`<model_dir>/<preset>/` 摆好三个文件即可，预设名随目录名走：

```text
D:/models/rusto/ppocrv6-tiny-int8/det.mnn
D:/models/rusto/ppocrv6-tiny-int8/rec.mnn
D:/models/rusto/ppocrv6-tiny-int8/dict.txt
```

```toml
[plugin.rusto]
model = "ppocrv6-tiny-int8"     # 默认值，也可以不写
model_dir = "D:/models/rusto"
```

`python scripts/fetch-models.py --dest D:/models/rusto` 就是按这个布局落盘，还支持
`--mirror` 与 `--force`。

## 排障

**模型缺失（`model_missing`）**

错误信息里带缺失文件的绝对路径，照路径放文件即可。三种来源任选：等首启下载、
跑 `fetch-models.py` 打离线包、或把模型拷到 `model_dir`。如果配了相对
`model_dir`，注意它是相对**宿主进程的工作目录**，不是相对插件目录 —— 建议直接写
绝对路径。`/ocr status` 里 `loaded=false`、`model_config` 是配置的预设名，能快速
确认找的是不是你以为的那套模型。

**国内下载慢 / 失败**

1. 配 `mirror = "https://hf-mirror.com"`（或自建镜像）；
2. 直接用离线包（`fetch-models.py` + `package.py --assets models`），首启零网络；
3. 手工把 det / rec / dict 放到 `<model_dir>/<preset>/`，然后 `/ocr warm`。

**`/ocr status` 看什么**

- `loaded`：引擎有没有成功加载。`false` 且报过 `engine_unavailable`，先看日志里
  模型加载失败的原因；
- `backend` / `model` / `model_config`：实际后端、实际模型、配置预设，三者不一致
  通常是 `model_dir` 指错；
- `concurrency` / `active`：并发上限与当前在跑的数量，`active` 长期顶满说明在排队；
- `rec_workers`：引擎内部线程数；
- `last_elapsed_ms`：最近一次识别耗时，`null` 表示还没跑过。懒加载只在第一次
  识别或 `/ocr warm` 时发生，所以刚部署完 `loaded = false` 是正常的。

**其它常见错误**

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `engine_busy` | 并发排满、等待超预算 | 稍后重试；单机 bot 默认 `concurrency = 1` 够用，真需要再调大 |
| `timeout` | 长边太大 / CPU 弱 / 并发高 | 降 `max_side`、调 `timeout_ms`、降 `concurrency` |
| `too_large` | 走 `data` 内联且原始字节 > 8 MiB | 改传 `file_id`，或先缩图 |
| 识别结果为空 | 图片没字 / 太小 / 方向太斜 | `/ocr detail` 看行数与置信度；换清晰原图 |
| 首次识别特别慢 | 懒加载模型（秒级） | 部署后先跑一次 `/ocr warm` |

## 构建与打包

```bash
cargo build --release -p fuyubot-plugin-rusto
./fuyubot-sdk.exe check target/release/rusto.dll   # 静态核对 ABI 符号
python scripts/package.py --plugin rusto --platform windows-x86_64
```

`[lib] name = "rusto"`，所以产物是 `rusto.dll` / `librusto.so` /
`librusto.dylib`，与 `plugin.json` 的 `entry` 始终对得上。发布包名形如
`rusto-<version>-<platform>.zip`，ZIP 根直接是 `plugin.json` 与动态库。

打包脚本会把 crate 根下的 `models/` 与 `NOTICE.md` **自动**打进 ZIP（存在才带），
所以 `cargo build` 之后直接跑 `package.py`：本地有模型就是离线包，没有就是在线包；
`--assets` 只用来追加别的文件。两条发布流程见
[`BUNDLE.md`](BUNDLE.md)。

## 项目结构

| 路径 | 说明 |
| --- | --- |
| `src/lib.rs` | 插件入口、生命周期与命令 / 服务 / 工具注册 |
| `src/host/` | 宿主通信：图片下载、消息收发、共享服务与工具注册 |
| `src/cmd/` | 命令解析、任务投递与 `/ocr` 业务 |
| `src/core/` | 配置与错误类型 |
| `src/engine/` | 引擎接缝：`OcrBackend` trait、引擎池、模型路径解析与 RustO 后端 |
| `plugin.json` | 插件元信息与 ABI 声明 |
| `NOTICE.md` | 第三方组件与权重的许可声明（随发布包分发） |

## 许可

插件源码与仓库其它插件一致；引擎 `rusto-rs` 是 MIT，默认 PP-OCRv6 权重是
Apache-2.0，MNN 格式说明见 [`NOTICE.md`](NOTICE.md)。默认**不使用** Indra
（OpenRAIL-M）权重，也不随包分发。
