# KFC

插件 id `kfc` · 版本 `0.1.5` · ABI v6 · 平台 windows-x86_64 / linux-x86_64 / macos-aarch64 / android-aarch64 / android-x86_64

## 项目介绍

`/kfc` 抓一句「肯德基疯话」，渲染成一张推特风格卡片，以照片回复原消息。它解决的是「想要一张能直接发群里的肯德基梗图」的需求：插件不链接浏览器、Node 或图片处理库，只负责取文案和拼模板数据，出图交给宿主的 `fuyubot.html2img` 服务。

工作链路：`/kfc` → 文案接口 → 文本清洗 → Vue 模板 `assets/card.html` + data → `fuyubot.html2img`（`render`，`vue: true`）→ 发照片 → 删除渲染产物。卡片上的互动数字是装饰，每次出图随机生成并格式化成中文计数。

## 目录结构

源码 crate：

```text
crates/fuyubot-plugin-kfc/
├── Cargo.toml            # cdylib + rlib
├── plugin.json           # id / 版本 / entry = bin/kfc.dll
├── config.default.toml   # 随包发布的默认配置
├── README.md
├── assets/
│   ├── card.html         # Vue 模板 + 内联 CSS（编译期嵌入）
│   ├── avatar.jpg        # 卡片头像
│   └── avatar.b64        # 头像的 base64 片段
├── examples/render_card_kfc.rs
└── src/
    ├── lib.rs            # Plugin 生命周期、inject 声明
    ├── help.rs           # 把 /kfc 登记进宿主命令表
    ├── cmd/              # mod / kfc / parser
    ├── core/             # config / fetch / text / mod
    ├── host/             # command / messenger / worker / mod
    └── render/           # html2img 客户端 + 模板数据
```

部署后的 bundle：

```text
plugins/kfc/
├── plugin.json           # entry 指向 bin/kfc.dll
└── bin/kfc.dll           # Linux 为 libkfc.so，macOS 为 libkfc.dylib
```

## 命令一览

| 命令 | 权限/范围 | 作用 |
| --- | --- | --- |
| `/kfc` | `user`；私聊 + 群 | 抓一句文案，渲染卡片并以照片回复原消息；不接受参数，多写的参数会被忽略 |

命令前缀来自 `[plugin.kfc].prefixes`，缺省为 `/`、`!`、`！`、`.`、`#`；命令名大小写不敏感，支持 `/kfc@机器人名` 写法。

## 配置一览

配置段 `[plugin.kfc]`，全部可选，缺失或类型不对时用默认值：

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `api_url` | `https://kfc-crazy-thursday.vercel.app/api/index` | 文案接口，只接受 `http(s)` 开头，响应体上限 256 KiB |
| `width` | `800` | 卡片宽度（px），超出 460–1600 时夹到边界 |
| `timeout_ms` | `15000` | 拉文案超时（毫秒）；也兼容 `timeout`（秒），同时存在时 `timeout_ms` 优先 |
| `prefixes` | `["/", "!", "！", ".", "#"]` | 命令前缀列表，空数组或非法值回退默认 |

`config.default.toml` 固化上表默认值，随发布包带入，按最低优先级并入 `[plugin.kfc]`，可被用户配置覆盖。

## 使用说明

**安装**：商店里搜索 `kfc` 安装；或 `fuyubot plugin install ./kfc`（指向插件目录或解压后的包）；或把 zip 直接解压到 `plugins/kfc/`，确认根上是 `plugin.json` 与 `bin/`，然后重启 `fuyubot run`。

**最小配置**（`fuyubot.toml`）：

```toml
[plugin.kfc]
api_url = "https://kfc-crazy-thursday.vercel.app/api/index"
width = 800
timeout_ms = 15000
```

**典型用法**：

```text
/kfc
！kfc@fuyu_bot
```

**构建与打包**：

```bash
cargo build --release -p fuyubot-plugin-kfc
python scripts/package.py --plugin kfc --platform windows-x86_64
```

产物名为 `<id>-<version>-<platform>.zip`，ZIP 根就是插件目录内容（`plugin.json` + `bin/kfc.dll`）；打包脚本会把 `plugin.json` 的 `config` 指向的默认配置（本插件为 `config.default.toml`）随包带入。

**发布**：

```bash
git tag kfc-v0.1.5
git push origin kfc-v0.1.5
```

## 项目依赖（软/硬）

| 类型 | 服务名 / 要求 | 说明 |
| --- | --- | --- |
| 硬依赖 | `fuyubot.html2img` | `Plugin::inject` 声明；未安装或渲染服务未就绪时插件停在 Pending，`/kfc` 不可用 |
| 软依赖 | 无 | 文案接口是公网 HTTP，不需要其它插件 |
| 宿主内建（不算插件依赖） | `fuyubot.host.command`、`fuyubot.host.commands` | 回复命令消息、登记 `/help` 行；宿主预置服务，无需安装 |

宿主要求：ABI v6（FuyuBot 0.1.5+），支持 `Plugin::inject`、`call_service` 与 `updateNewMessage` 订阅；运行机器需能访问文案接口，图片发送依赖宿主的 TDLib 连接。
