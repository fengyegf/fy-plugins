# FuyuBot Epic

Epic 免费游戏卡片插件，基于 FuyuBot Plugin SDK 构建。插件会读取 Epic 商店促销
接口，筛选当前免费和即将免费的游戏，下载封面并生成卡片图，以照片回复命令消息。

## 功能

- 查询 Epic 当前免费和即将免费的游戏。
- 自动下载并裁切游戏封面；单张封面失败时保留占位，不影响整张图。
- 支持多行、多列卡片布局和自定义输出缩放。
- 渲染失败时自动降级为文本游戏清单。
- 临时图片在发送完成后自动删除。
- 命令前缀与宿主顶层 `[config] PREFIXES` 保持一致。

## 与 `/help` 的联动

插件在 `start()` 里通过进程级共享服务 `fuyubot.help.commands` 的 `register`
方法，把 `/epic` 登记进内置 `/help`，用户发 `/help` 就能看到这条命令。

依赖用 `soft_inject` 声明（写的是服务名，不是插件名）：有 help 就排在它后面，
没有也照常启动——登记失败只记一条 warn，不影响 `/epic` 本身。服务名、方法名与
JSON 协议见插件文档的[「插件联动」](https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/linkage/)。

## 使用

```text
/epic
/epicfree
```

两个命令等价。没有免费游戏时回复文本提示；接口请求失败时回复
“获取Epic免费游戏信息失败。”。

## 处理流程

1. 请求 Epic 促销接口并解析当前免费、即将免费的游戏。
2. 按 `max_cards` 截断列表，当前免费游戏优先。
3. 并发下载封面，单张失败时保留深灰占位。
4. 生成 HTML/CSS 版面，由 `hyper-render` 渲染文字和卡片布局。
5. 检测封面槽位，裁切并合成封面，输出 PNG。
6. 写入临时目录并以照片发送，随后删除临时文件。

## 配置

配置位于 `fuyubot.toml` 的 `[plugin.epic]` 段，所有字段均可省略：

```toml
[plugin.epic]
locale = "zh-CN"
country = "CN"
timezone_offset_hours = 8
api_url = "https://store-site-backend-static.ak.epicgames.com/freeGamesPromotions"
proxy = ""
timeout_ms = 20000
cache_dir = "cache"
scale = 2.0
max_per_row = 4
max_cards = 8
card_width = 280
cover_height = 157
caption = ""
user_agent = "Mozilla/5.0 (compatible; fuyubot-epic/0.1)"
```

字段说明：

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `locale` | `zh-CN` | Epic 接口语言 |
| `country` | `CN` | 接口国家与 `allowCountries` |
| `timezone_offset_hours` | `8` | 日期换算使用的 UTC 偏移 |
| `api_url` | Epic 官方接口 | 可替换为区域接口或镜像 |
| `proxy` | 空 | HTTP 代理地址，例如 `http://127.0.0.1:7890` |
| `timeout_ms` | `20000` | 单次 HTTP 请求超时 |
| `cache_dir` | `cache` | 发送前的临时 PNG 目录 |
| `scale` | `2.0` | 输出缩放，限制为 `0.5` 到 `4.0` |
| `max_per_row` | `4` | 每行最多展示的卡片数 |
| `max_cards` | `8` | 单张图最多展示的游戏数 |
| `card_width` | `280` | 卡片宽度，单位为 CSS 像素 |
| `cover_height` | `157` | 封面高度，单位为 CSS 像素 |
| `caption` | 空 | 图片说明；留空时不发送说明 |
| `user_agent` | 内置 UA | Epic 接口请求头 |

命令前缀由宿主顶层配置提供：

```toml
[config]
PREFIXES = ["/", "!", "！", ".", "#"]
```

## 构建

在仓库根目录执行：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p fuyubot-plugin-epic
```

Windows 发布产物：`target/release/epic.dll`（Linux `libepic.so`，macOS
`libepic.dylib`）。

`Cargo.lock` 在仓库根目录统一维护，必须一起提交；不要随手 `cargo update`。

检查动态库入口与 ABI：

```powershell
./fuyubot-sdk.exe check target\release\epic.dll
```

发布包由仓库根目录的 `scripts/package.py` 生成（`epic-<version>-<platform>.zip`），
多平台构建见根 README 的「发布一个插件」。

## 安装

将 `target/release/epic.dll` 复制到宿主的插件目录：

```powershell
Copy-Item target\release\epic.dll <FuyuBot目录>\plugins\epic\
```

重启宿主后可以在插件列表中确认 `epic` 已加载。

## 项目结构

```text
src/
  lib.rs            插件入口与生命周期
  core/
    config.rs       [plugin.epic] 与命令前缀配置
    fetch.rs        带超时、代理和大小限制的 HTTP 客户端
    epic.rs         Epic 接口解析、封面选择与日期换算
  render/
    mod.rs          渲染参数与统一入口
    template.rs     HTML/CSS 卡片模板
    card.rs         渲染、槽位检测与合成编排
    compose.rs      封面裁切、圆角遮罩与 PNG 合成
  host/
    mod.rs          共享运行上下文
    messenger.rs    文本与照片消息发送
    worker.rs       消息回调、路由线程与业务线程
  cmd/
    mod.rs          命令规划与 panic 兜底
    parser.rs       命令前缀与机器人名解析
    epic.rs         /epic 完整业务流程
assets/
  gift.svg          页头礼物图标
  epic-logo.svg     Epic 官方标记
```

`assets/` 中的 SVG 在编译时内联进模板，部署时不需要单独复制。

## 渲染说明

- 卡片先由 `hyper-render` 生成带占位块的 PNG，再由 `image` 合成封面。
- 有封面的槽位使用洋红色标记，渲染后扫描像素定位实际矩形。
- 槽位检测数量异常时退回解析式坐标，避免无法出图。
- 封面按中心裁切，经过 Lanczos3 缩放和顶部圆角遮罩后贴回原位。
- 全过程不启动浏览器或外部截图服务。

## 已知限制

- `hyper-render` 只使用系统字体。Linux 宿主需要安装 Noto Sans CJK 等中文字体。
- 不支持 `@font-face`，也不加载 HTML 中的图片或背景图。
- 标题最多展示两行，超长内容会截断。
- Epic 接口或 CDN 不可达时，封面可能降级为占位，接口失败则会回复错误提示。
