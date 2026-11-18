# RustO OCR

fengyegf.rusto · v0.1.1 · ABI v6 · windows-x86_64 / linux-x86_64 / macos-aarch64 / android-aarch64 / android-x86_64

## 项目介绍

RustO OCR 是一个**纯 Rust 的图片文字识别插件**：图片进、文本出。引擎是 [RustO!](https://github.com/byrizki/rusto-rs)（`rusto-rs` 0.4.1 + 纯 Rust 的 MNN 运行时 `rusto-nn`），模型为 PP-OCRv6 Tiny INT8 的 det / rec / dict 三件套；不依赖 Python、OpenCV、ONNX Runtime，也不起下载子进程。

- **解决什么问题**：截图、聊天记录、票据里的文字想直接拿走；其它插件需要 OCR 时不必各自引引擎。
- **用户侧入口**：回复图片发 `/ocr`，或图片说明写 `/ocr`；私聊直发图且 `auto = true` 时才自动识别（默认关，不抢图）。
- **跨插件入口**：进程级共享服务 `rusto.ocr`（协议 v1，含 `recognize` / `warm` / `status`）+ 工具 `rusto.ocr`（给宿主 / AI 发现调用；服务通道 `async: true` 的结果走 `rusto.ocr.done` 事件）。
- **工作链路**：`updateNewMessage` 回调只投递 → 路由线程解析命令 → 短命业务线程「定位 `file_id` → `downloadFile` 落盘 → 长边限幅 → 引擎识别 → 回复 / 编辑占位消息」；服务方法跑在调用方线程，同步返回 JSON。

## 目录结构

源码（crate 根）：

```text
crates/fuyubot-plugin-rusto/
├── plugin.json               # id / 版本 / ABI v6 / platforms
├── Cargo.toml                # [lib] name = "rusto"（cdylib + rlib）
├── config.default.toml       # 最低优先级默认配置（随包分发）
├── README.md
├── NOTICE.md                 # 第三方许可声明（随包分发）
├── models/ppocrv6-tiny-int8/ # 内置模型：det.mnn / rec.mnn / dict.txt / SHA256SUMS
├── src/
│   ├── lib.rs                # 生命周期：配置、worker、命令表、服务与工具
│   ├── core/                 # config.rs（[plugin.fengyegf.rusto]）、error.rs
│   ├── engine/               # OcrBackend 接缝、EnginePool、models.rs、rusto_backend.rs
│   ├── host/                 # command / download / messenger / photo / worker / service
│   └── cmd/                  # parser.rs（命令解析）、ocr.rs（业务）、mod.rs
└── testdata/                 # 回放数据（replay_photo.json、fuyubot.replay.toml）
```

部署后（bundle 解压到宿主插件目录）：

```text
plugins/fengyegf.rusto/
├── plugin.json
├── config.default.toml
├── NOTICE.md
├── bin/rusto.dll             # Linux: bin/librusto.so，macOS: bin/librusto.dylib
└── models/ppocrv6-tiny-int8/{det.mnn,rec.mnn,dict.txt}
```

## 命令一览

| 命令 | 权限 / 范围 | 作用 |
| --- | --- | --- |
| `/ocr` | user · 私聊 / 群 | 识别图片文字（回复图片，或图片说明里写命令） |
| `/ocr detail`（`d`、`详细`） | user · 私聊 / 群 | 附行级置信度与四点坐标，以及行数、耗时 |
| `/ocr status`（`状态`） | user · 私聊 / 群 | 引擎状态：`loaded`、模型、并发、最近耗时 |
| `/ocr warm`（`预热`） | user · 私聊 / 群 | 预加载模型（部署后先跑一次，避免首张慢） |
| `/ocr path <本机路径>` | user · 私聊 / 群 | 识别本机图片，不经过 TDLib 下载（排障用） |
| `/ocr help`（`-h`、`--help`、`帮助`） | user · 私聊 / 群 | 用法提示 |

命令名前缀为 `/`、`!`、`！`、`.`、`#`，命令名是 `ocr`，也接受插件名 `rusto`；前缀硬编码在 `src/cmd/parser.rs`，**不读** `[plugin.fengyegf.rusto].prefixes` 或宿主 `[config].PREFIXES`。插件自身不做权限判断，`/ocr` 以 `role = user`、`scopes = private/group` 登记到宿主 `fuyubot.host.commands`（登记失败只影响 `/help` 一行）。未识别的子命令会退回普通 `/ocr`。

## 配置一览

配置段：`[plugin.fengyegf.rusto]`。下表默认值与 `src/core/config.rs` 一致。

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `auto` | `false` | 私聊直接发图是否自动识别；默认关，只认显式命令 |
| `max_side` | `960` | 识别前长边限幅（像素），夹紧到 320–4096；越小越快 |
| `concurrency` | `1` | 同时识别数，夹紧到 1–8；单引擎峰值内存约 300–400 MiB |
| `rec_workers` | `1` | 引擎识别线程数，0–8；`0` = 按 CPU 核数自选（16 核约 15 线程） |
| `model` | `"ppocrv6-tiny-int8"` | 模型预设名，同时是模型目录名；另有 `ppocrv6-tiny` / `-fp16` / `-small` / `-medium` 等档位 |
| `model_dir` | 空 | 模型根目录；空 = 依次尝试 `./data/plugins/rusto/models`、`./models`、rusto 缓存目录 |
| `mirror` | 空 | 模型下载源前缀；空 = HuggingFace 官方源 |
| `detail_default` | `false` | `/ocr` 默认是否带行级明细 |
| `reply_placeholder` | `true` | 是否先回「识别中…」占位消息，再编辑为结果 |
| `chunk_size` | `3800` | 单条回复最大字符数，夹紧到 200–4000（Telegram 4096 上限留余量） |
| `timeout_ms` | `20000` | 单次识别总预算（下载 + 排队 + 推理），夹紧到 1000–120000 |

读取顺序：`start()` 传入的配置 JSON → 宿主 `config_value("rusto", "")` 兜底；缺键、类型不符回退缺省，超范围夹到边界；也接受 `maxSide` 这类 camelCase 别名。宿主没有配置热重载通道，改完 `fuyubot.toml` 需重载插件。包内 `config.default.toml` 是随包分发的最低优先级默认值，不写等于用源码缺省。

## 使用说明

**安装**（三选一）：插件商店直接安装；`fuyubot plugin install ./rusto`；或把 bundle 解压到 `plugins/fengyegf.rusto/`（`plugin.json` 在根、动态库在 `bin/`）后重启宿主。

**最小配置**（`fuyubot.toml`）：

```toml
[plugin.fengyegf.rusto]
auto = false
max_side = 960
concurrency = 1
model = "ppocrv6-tiny-int8"
# 国内网络 / 离线部署时按需打开：
# mirror = "https://hf-mirror.com"
# model_dir = "C:/fuyubot/plugins/fengyegf.rusto/models"
```

**典型用法**：对图片消息回复 `/ocr`；或直接在图片说明里写 `/ocr`；要看行级明细用 `/ocr detail`。部署后先 `/ocr warm`，再用 `/ocr status` 确认 `loaded = true`。默认先回「识别中…」再编辑成结果，超长文本按 `chunk_size` 分块并带「（1/N）」；没有图片会回复用法提示。

**模型获取**：默认首启识别时按 `model` 找模型，缺了就从 HuggingFace 下载（可用 `mirror` 换源），之后零网络；离线 / 内网环境用 `python scripts/fetch-models.py` 把三件套放进 `crates/fuyubot-plugin-rusto/models/<preset>/`（`.mnn` 优先、`.rten` 兜底，det 与 rec 须同格式）。手动放置时目录是 `<model_dir>/<preset>/{det,rec,dict}`，放好后 `/ocr warm` 或重载插件即可。

**构建与打包**：

```bash
cargo build --release -p fuyubot-plugin-rusto
python scripts/package.py --plugin rusto --platform windows-x86_64
```

`package.py` 会把 crate 根的 `models/` 与 `NOTICE.md` **自动**打进 bundle（存在才带），`--assets` 只用于追加别的文件；产物为 `dist/fengyegf.rusto-0.1.1-windows-x86_64.zip`，ZIP 根就是插件目录内容。体积参考：`rusto.dll` 约 3.2 MiB、模型三件套约 3.5 MB；离线包 zip 实测约 4.2 MiB（解压 ≈6.7 MB），远低于商城 50 MB 上限；不带 `models/` 即在线包。

**发布**：

```bash
git tag fengyegf.rusto-v0.1.1
git push origin fengyegf.rusto-v0.1.1
```

## 项目依赖（软/硬）

| 类型 | 名称 | 用途 | 缺失时 |
| --- | --- | --- | --- |
| 硬依赖 | 宿主 ABI v6（`fuyubot-plugin-api`） | 插件加载、事件订阅、服务 / 工具注册 | 无法加载 |
| 硬依赖（宿主内建） | `fuyubot.host.command`.`reply` | 回复命令消息（占位、结果、分块） | 回复失败，命令无输出 |
| 硬依赖（宿主内建） | `fuyubot.host.commands`.`register` | 把 `/ocr` 登记进 `/help` | 只少一行帮助，`/ocr` 仍可用 |
| 硬依赖（宿主内建） | TDLib 通道 `downloadFile` / `getMessage` / `edit_message_text` | 图片落盘、取引用消息、编辑占位 | 下载 / 编辑失败，退化为直接回复 |
| 软依赖 | 无（不调用其它插件提供的服务） | — | — |
| 对外提供 | `rusto.ocr`（服务 v1 + 工具） | 兄弟插件同步拿识别 JSON；异步结果走 `rusto.ocr.done` 事件 | 其它插件按软依赖处理，未安装则不可用 |
| 外部资源 | PP-OCRv6 权重（det / rec / dict） | 识别必需；默认 `ppocrv6-tiny-int8`，随包或首启下载 | 报 `model_missing` 并提示缺失文件 |
| 外部网络 | HuggingFace `byrizki/rusto-models` | 在线包首启下载模型（可用 `mirror` 换源） | 改用离线包或手动放模型 |

`fuyubot.host.command`、`fuyubot.host.commands` 等是**宿主内建服务**，不算插件依赖；本插件也不依赖 `fuyubot.html2img`（非渲染类，不出图）、`storage.mongo`、`fuyubot.host.cache`。第三方许可：`rusto-rs` MIT、PP-OCRv6 权重 Apache-2.0，随包附带 `NOTICE.md`。
