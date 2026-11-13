# FuyuBot Epic

Epic 免费游戏卡片插件，基于 FuyuBot Plugin SDK 构建。插件读取 Epic 商店促销接口，
筛选当前免费和即将免费的游戏，把结果整理成 Vue 模板的 `data`，交给宿主上的
[`fuyubot.html2img`](https://github.com/xiaoqvan/fuyubot-plugins/tree/main/crates/fuyubot-plugin-html2img)
服务渲染成卡片图（JPEG），以照片回复命令消息。

**界面不在 Rust 里拼**：卡片版式、循环、条件与插值都在
[`assets/card.html`](assets/card.html)（`<style>` + Vue 模板正文）里；插件只做
数据映射，渲染交给 html2img（服务名 `fuyubot.html2img`，方法 `render`，
`vue: true`；渲染器是 hyper-render 的本地 fork，Blitz + Stylo + Taffy + Vello CPU，
纯 Rust、无 Chromium / Node）。本插件不链接渲染引擎，也不再下载、解码、合成封面。

## 依赖

- **必须安装 `fuyubot.html2img` 插件**：本插件用 `Plugin::inject` 声明了硬依赖，
  宿主会在服务就绪后再启动 epic；没装时插件停在 Pending（不静默失败）。
- 封面只把公网 `https` URL 放进 `data`，由 html2img 拉取与裁切，插件不下载任何
  封面字节。
- 页头的礼物图标与 Epic 标记内联在模板里，不依赖网络。

## 功能

- 查询 Epic 当前免费和即将免费的游戏。
- 封面以公网 https URL 交给渲染服务；接口没给封面、或 URL 不是公网 https
  （`http://`、`file://`、`data:`、私网 / 回环地址）时数据里是空串，模板画深灰
  占位块，不影响出图。
- 当前免费与即将免费**排在同一行**（顺序 present → Coming，与原版 `epic.vue` 一致），
  超过 `max_per_row` 张才用 `flex-wrap` 折到下一行；支持多行多列和自定义输出缩放。
- 渲染失败时自动降级为文本游戏清单。
- html2img 返回的临时图在发送完成后立即删除。
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
3. 生成 Vue 模板的 `data`：`present` / `Coming` 两组卡片（每项
   `{title, startDate, endDate, offerImage}`）+ 版式参数（画布宽、卡片宽、封面高、
   间距、页头标题、空列表文案、背景色）。标题按两行预算截断；封面 URL 先做公网
   校验，不可用就传空串。
4. 调 `host.call_service("fuyubot.html2img", "render", …)`：`vue: true` +
   `data`，固定 `width`（按列数算出的画布宽）、`height = 100`（兜底视口）、
   `auto_height = true`（高度由内容撑开）、`scale` = 配置值（超 16M 像素自动下调）、
   `format = "jpg"`。
5. 拿到服务返回的本机 `path`，`send_photo` 发送，随后删除该文件。

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
| `scale` | `2.0` | 输出缩放，限制为 `0.5` 到 `3.0`（html2img 的 `max_scale` 默认值）；超像素上限时还会自动下调 |
| `max_per_row` | `4` | 每行最多展示的卡片数（决定画布宽度） |
| `max_cards` | `8` | 单张图最多展示的游戏数 |
| `card_width` | `280` | 卡片宽度，单位为 CSS 像素（模板里的 `width`） |
| `cover_height` | `157` | 封面高度，单位为 CSS 像素（模板里的图片高度） |
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
cargo fmt --check -p fuyubot-plugin-epic
cargo clippy -p fuyubot-plugin-epic --all-targets -- -D warnings
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

并确保宿主插件目录里有 `fuyubot.html2img`（本插件的硬依赖）。
重启宿主后可以在插件列表中确认 `epic` 已加载。

## 项目结构

```text
src/
  lib.rs            插件入口与生命周期（inject 声明 html2img 硬依赖）
  core/
    config.rs       [plugin.epic] 与命令前缀配置
    fetch.rs        带超时、代理和大小限制的 HTTP 客户端
    epic.rs         Epic 接口解析、封面选择与日期换算
  render/
    mod.rs          画布宽度、倍率预算、渲染入口（不拼 HTML）
    data.rs         业务模型 → Vue 模板 data（present / Coming）
    html2img.rs     fuyubot.html2img 服务客户端（拼入参 / 调服务 / 解析 path）
  host/
    mod.rs          共享运行上下文
    messenger.rs    文本与照片消息发送
    worker.rs       消息回调、路由线程与业务线程
  cmd/
    mod.rs          命令规划与 panic 兜底
    parser.rs       命令前缀与机器人名解析
    epic.rs         /epic 完整业务流程
assets/
  card.html         Vue 卡片模板（<style> + 模板正文，含内联 SVG）
examples/
  render_card_epic.rs  导出 html2img 请求 JSON 与模板 data
```

`assets/card.html` 在编译期嵌入（`include_str!`），部署时不需要单独复制。

## 模板约定

`assets/card.html` 是 `<style>` + Vue 模板正文，**没有** `<template>` / `<script>`
外壳：html2img 直接 `Template::compile(html)`。约定：

- 模板里不出现 `<script>`、事件、组件、`v-model`；
- 表达式受 vue-template-html 限制：**没有函数调用、没有 `Math` / `Date`、不用
  `?.` / `??`**。这类值（标题截断、封面可用性、日期文案）都在 `render/data.rs`
  里算好，模板只读字段；
- 随数据变化的样式走 `:style="'width:' + width + 'px'"`，图片走 `:src`；
- 模板必须保持 ≤ 128 KiB（html2img 的 `max_html_bytes`）。

本地校验模板（不依赖 html2img）：

```bash
cd target/vuecheck
cargo run --quiet -- ../../crates/fuyubot-plugin-epic/assets/card.html \
  ../qa/json/epic-mixed.data.json out.html
```

`data` 的字段见 `src/render/data.rs`；把任一份 data.json 交给上面的命令即可校验模板。

## 渲染说明

- 画布**宽度固定**（按列数算好后传给 html2img），高度用 `auto_height: true` 由内容
  撑开；`present` 与 `Coming` 的卡片放在**同一个** `flex-wrap: wrap` 行里（顺序
  present → Coming，与原版 `epic.vue` 一致），`max_per_row` 决定一行放几张、画布多宽。
- 输出格式固定 `jpg`：卡片是深色不透明背景，JPEG 体积小、够 Telegram 照片用。
- 发请求前按服务的默认上限做校验：估算高度 ≤ 6000px、总像素 ≤ 16M（超出时自动下调
  `scale`，最低 0.5）、模板 ≤ 128 KiB。校验失败会给出明确原因并降级成文本清单。
- 封面 URL 只接受公网 `https`：服务会拒绝私网 / 回环地址和 `file://`，提前筛掉
  坏 URL 能让那张卡显示占位块，而不是让整张图渲染失败。
- 全过程不下载封面、不做位图合成，也不启动浏览器或外部截图服务。

## 已知限制

- 必须安装 `fuyubot.html2img`：没有它插件停在 Pending，`/epic` 不会工作。
- html2img **只加载公网 `http(s)` 与 `data:` 图片**，且单张 8 MiB、连接 / 读取有超时；
  Epic CDN 不可达时封面会显示成深灰占位块（文字信息照常出图）。
- html2img 只认系统字体：中文依赖系统装了的 CJK 字体（Windows 默认有），
  Linux 宿主需要安装 Noto Sans CJK 等中文字体。
- 渲染器不执行 JavaScript、不加载远程样式表：模板是静态 Vue 模板 + 内联样式。
- 标题最多展示两行，超长内容在 Rust 侧按全角宽度截断；Epic 接口失败时回复错误提示。

## 测试

```bash
cargo test -p fuyubot-plugin-epic
```

- 单元测试（`src/**`）：仓库原有的通用测试。
- 真实出图需要宿主上装好 `fuyubot.html2img`，本地单测不断言图片字节。
- 手动生成样张：

  ```bash
  cargo run --example render_card_epic -- epic-sample.json
  ```
