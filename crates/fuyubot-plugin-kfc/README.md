# kfc — 点个肯德基吧（Rust 插件）

TS 版 `E:\Fuyu_TDBot\plugins\kfc`（`index.ts` + `kfc.vue`）的 Rust 迁移版：

`/kfc` → 抓一句「肯德基疯话」→ 渲染成一张推特风格卡片 → 以照片回复命令消息 → 删除临时文件。

图像生成不再由插件自己完成：卡片界面是 `assets/card.html` 里的 **Vue 模板 + 内联 CSS**
（原 `kfc.vue` 的模板），Rust 只把文案 / 宽度 / 互动数字凑成 `data`，交给宿主上的
[`fuyubot.html2img`](https://github.com/xiaoqvan/fuyubot-plugins/tree/main/crates/fuyubot-plugin-html2img)
插件（服务名 `fuyubot.html2img`，方法 `render`）。渲染器是 hyper-render 的本地 fork
（Blitz + Stylo + Taffy + Vello CPU），**纯 Rust、无 Chromium / Node**，但只存在于
html2img 插件里，本插件不再链接渲染引擎。

## 依赖

- **必须安装 `fuyubot.html2img` 插件**：本插件用 `Plugin::inject` 声明了硬依赖，
  宿主会在服务就绪后再启动 kfc；没装时插件停在 Pending（不静默失败）。
- 卡片的头像内嵌为 `data:image/jpeg;base64,…`，由 html2img 加载；模板里的认证蓝标、
  互动图标仍是内联 SVG，不依赖网络。

与 TS 版的差异：

| | TS 版 | Rust 版 |
|---|---|---|
| 运行形态 | `plugins/kfc/index.ts`（Node 插件） | 单个 `kfc.dll`（宿主 dlopen） |
| 模板 | `kfc.vue`（Vue 运行时 + `<script setup>`） | `assets/card.html`（原 `kfc.vue` 的模板 + 内联 CSS） |
| 渲染 | `satoriVue` → SVG → `@resvg/resvg-js` → `sharp` | 宿主服务 `fuyubot.html2img` 的 Vue 模式（模板 + data → JPEG） |
| 头像 | 模板内 `<img src="data:image/jpeg;base64,…">` | 同样是 `<img src="data:…">`，交给 html2img 加载 |
| 字体 | 仓库内 Noto Sans SC + Windows 回退字体 | 只用系统字体（html2img 不随插件带字体） |
| 缓存 | `getImgCache` 查 MongoDB 图片表 | 无缓存；html2img 的产物在临时目录保留 24 小时，本插件发完即删 |
| 命令注册 | `cmdHandlers` | 文本命令解析（`/kfc`，前缀见下文） |

## 构建

在仓库根目录执行：

```bash
cargo build --release -p fuyubot-plugin-kfc
```

产物：`target/release/kfc.dll`（Linux `libkfc.so`，macOS `libkfc.dylib`），部署时复制
到宿主的插件目录：

```text
plugins/kfc/kfc.dll
```

`Cargo.lock` 在仓库根目录统一维护，必须一起提交；不要跑 `cargo update`。

静态检查导出符号（不加载、不联网）：

```bash
./fuyubot-sdk.exe check target/release/kfc.dll
```

## 配置

写在 `fuyubot.toml` 的 `[plugin.kfc]` 段，全部可选：

```toml
[plugin.kfc]
api_url    = "https://kfc-crazy-thursday.vercel.app/api/index" # 文案来源
width      = 800        # 卡片宽度（px），对应 TS 版 generateImage 的 width；夹到 460–1600
timeout_ms = 15000      # 拉文案的超时
# prefixes = ["/", "!", "！", ".", "#"]  # 不写就用缺省前缀
```

命令前缀缺省为 `/ ! ！ . #`；TS 宿主那套顶层 `[config] PREFIXES` 只作为第三层兜底
（Rust 宿主没有这个键，读不到就继续用缺省值），要改就在 `[plugin.kfc]` 里显式写。

渲染相关（宽度 / 输出倍率 / 格式）由本插件按上面的 `width` 调 html2img 的 `render`：
`scale = 1.0`、`format = jpg`、`auto_height = true`（视口高度下限 100px，短文案不会被留白撑高）。
html2img 自己的全局配置（`[plugin.fuyubot.html2img]`）决定它的默认值与各种上限，
本插件不覆盖。

## 与 `/help` 的联动

FuyuBot 0.1.5 起**命令表归宿主**：宿主预置了进程级服务 `fuyubot.host.commands`，
其它插件的 `/help` 只是这张表的读取方（`fuyubot.host.commands.list`）。插件在
`start()` 里调它的 `register`，把 `/kfc` 连同 `role` / `scopes` / `hidden` 一起登记：

```json
{"name":"kfc","description":"…","role":"user","scopes":["private","group"],"hidden":false}
```

`/kfc` 在代码里没有权限判断（谁都能发，私聊 / 群都能用），所以登记 `role = user`、
`scopes = ["private","group"]`、`hidden = false`——登记只是声明，不放宽也不收紧执行
权限。成功回 `{"ok":true,"name":…,"plugin":…}`，失败回 `{"error":…}`。

这张表由宿主预置，不是插件，所以**不需要 `soft_inject`**（本插件也不再声明它）。
登记失败只记一条 warn，不影响 `/kfc` 本身。服务名、方法名与 JSON 协议见插件文档的
[「插件联动」](https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/linkage/)。

## 命令

- `/kfc`：抓取并出图。接口失败 / 空文案 / 渲染失败都会回一条文本说明，
  不会静默失败。

回复形态由宿主决定，插件不自己拼 `sendMessage`：回复命令消息时只把
`{"chat_id","message_id","timeout_ms","payload":{"input_message_content":…}}` 交给
`fuyubot.host.command` 的 `reply`——bot / 白名单用户发新消息并引用命令，user 账户
自己触发的命令被「编辑原命令」，群里的 ephemeral 命令走 `sendEphemeralMessage`。
没有命令消息可回的主动推送（例如 panic 兜底提示）才走 simplify 的 `send_message`；
相册（`sendMessageAlbum`）同理留在 simplify（一次多条，没有单条命令回复语义）。

## 架构

```text
src/
  lib.rs            插件入口：init/start/stop，订阅 updateNewMessage，持有 worker
  cmd/
    parser.rs       文本命令解析（前缀 / @机器人名）
    mod.rs          Job 规划、panic 兜底
    kfc.rs          主流程：抓文案 → 清洗 → 渲染 → 发送 → 删临时文件
  core/
    config.rs       [plugin.kfc] 配置读取
    fetch.rs        阻塞 HTTP 取文案（ureq）
    text.rs         文案清洗（换行归一、控制字符 / 零宽字符清理）
  host/
    mod.rs          Ctx（host / config / messenger / 停机标志）
    command.rs      fuyubot.host.command.reply 客户端（拼入参 / 解最终 Message / 错误映射）
    messenger.rs    发送入口：回复命令走宿主命令通道，主动推送走 simplify send_message
    worker.rs       回调 → channel → 路由线程 → 业务线程
  render/
    mod.rs          数据映射 view_data()（含随机互动数字）、render_card()、宽度夹取、模板嵌入
    html2img.rs     fuyubot.html2img 服务客户端（vue 模式：拼入参 / 调服务 / 解析 path）
assets/
  card.html        Vue 卡片模板（<style> + 原 kfc.vue 的模板正文；头像 base64、图标内联）
  avatar.jpg       头像原图（便于查看，模板里用的是它的 base64）
  avatar.b64       同一张图的 base64
```

线程模型（三条硬约束决定）：

1. `updateNewMessage` 回调跑在宿主的 TDLib 接收线程上，**不能阻塞**；
2. 发消息 / 上传图片要等响应，而响应同样由接收线程投递——在回调里等就是自己等自己；
3. 插件卸载前必须 join 自己起的线程，否则动态库被卸载后仍有代码在跑。

所以：回调只把更新丢进 channel，常驻 worker 线程解析并分发，每条命令再开一个短命线程
跑完整流程（HTTP + 渲染服务调用 + 上传都是阻塞的）。命令回复
（`fuyubot.host.command.reply`）同样**同步阻塞**（宿主当场等 TDLib 的最终消息），
也只能在这个业务线程里调，绝不在更新回调里调。

## 渲染管线

```text
文案
  ↓ text::normalize          换行归一、控制字符清理
  ↓ render::view_data()      { content, width, contentWidth, textWidth, 四个随机互动数字, 互动行尺寸 }
  ↓ host.call_service("fuyubot.html2img", "render", { html: assets/card.html, vue: true, data, … })
  ↓ 服务里：Vue 模板 + data → HTML → JPEG
  ↓ 服务返回 { ok: true, path: "…/html2img_xxx.jpg" }
  ↓ send_photo(path) → 删除 path
```

## 互动行（新版 X 布局 + 随机数字）

互动行按新版 X 的排布：左区「回复 / 转帖 / 喜欢 / 查看」四等分平铺，右区「书签 / 分享」
收在一起——查看（bar-chart）是左区第四格，和网页版新版 X 一致。四个数字每次出图现掷：

| 字段 | 随机范围 | 格式 |
|---|---|---|
| `replyCount` | 300–19,999 | 千分位或万，如 `4,584` / `1.2万` |
| `repostCount` | 900–99,999 | 万位上下浮动，如 `1.2万` / `3万` |
| `likeCount` | 4,000–999,999 | 多为万，如 `12万` |
| `viewsCount` | 30,000–9,999,999 | 必定是万，如 `368万` |

格式化规则与 X 的中文计数一致（`format_count`）：小于 1 万用千分位；1 万–10 万取一位小数、
截断不四舍五入；10 万以上取整数万。随机数用进程内 xorshift64*（种子混时间 / 栈地址 / 序号），
不为四个装饰数字引 `rand`。

互动图标全部来自 x.com 的图标 sprite（新版 X 的 `icon-reply` / `icon-repost` / `icon-heart` /
`icon-bar-chart` / `icon-bookmark` / `icon-share-ios` 的 path），不是自绘：查看那个条形图就是
X 自己的 `bar-chart`。图标边长 / 数字字号 / 间距按内容列宽度等比缩放（800 宽卡片下图标 32px、
数字 22px），460 的窄卡片缩到图标 16px、数字 10px，四个数字 + 书签 / 分享仍然排得进
273px 的内容列。

## 已知限制

- **内容列 / 正文的宽度由 Rust 算成确定像素**（`contentWidth` / `textWidth`）：渲染器按
  `min-width: auto` 取内容列的 min-content 宽度，`flex-grow: 1` 收不窄，正文和互动图标行
  会顶出卡片右边缘（线上表现：正文第一行被切掉、互动图标只剩第一个）。互动行的图标 /
  字号 / 间距同样由 `actionIcon` / `actionFont` 等字段算好交给模板，不走 flex 收缩兜底。
- **正文插值必须紧贴标签**：`<div class="tweet-text" …>{{ content }}</div>`。
  `vue-template-html` 会把模板里的换行 / 缩进原样留在输出里，而正文是
  `white-space: pre-wrap`，插值前后多出来的缩进会变成「空一行 + 首行缩进」。
- 渲染器**不执行 JavaScript**：卡片是 Vue 模板 + 静态 CSS，编译后就是静态 HTML，正好不需要 JS。
- html2img **只加载公网 `http(s)` 与 `data:` 图片**（`file://`、相对路径、私网 / 回环地址
  都会被拒绝）；本卡片的头像走 `data:`，其它图形是内联 SVG，不依赖网络。
- html2img 默认限制单次 HTML ≤ 128 KiB、高度 ≤ 6000px、输出倍率 ≤ 3.0：本插件的模板约
  31 KiB，正文超长时由服务直接报错，命令层会回一条文本说明。
- 模板由 html2img 内的 `vue-template-html` 编译，只支持 Vue 模板的子集：没有函数调用、
  没有 `Math` / `Date` 等全局对象，也没有 `?.` / `??`。卡片模板只做插值和循环，不碰这些。
- 只认系统字体（html2img 不随插件带字体）：中文依赖系统装了的 CJK 字体（Windows 默认有），
  所以观感与 TS 版的 Noto Sans SC 略有差别。字体链固定为
  `Segoe UI → Segoe UI Emoji → Segoe UI Symbol → Microsoft YaHei`：
  接口文案偶尔带 `◁ ▷ ↻ ❚ ♬` 这类符号，少了 `Segoe UI Symbol` 就是豆腐块；
  但 `Segoe UI Emoji` 必须排在它**前面**，否则 emoji 会被 Symbol 的单色字形抢走。
  少数旗帜类 emoji 仍不完美：可能显示成方块，或退化成区域指示字母（🇨🇳 → `CN`）。
- 宽度夹在 460–1600：低于 460 时内容列放不下「头像 + 昵称 + 蓝标 + 句柄 + 六个互动动作」，
  句柄会断在半个词上，用户名行进入换行兜底后卡片底部还会多出一条空白带
  （实测 400–456 有 96–139px），所以直接抬高下限而不是渲染出残版式。

## 测试

```bash
cd crates/fuyubot-plugin-kfc
cargo test
```

- 单元测试（`src/**`）：命令解析、配置解析、文案清洗（换行 / 控制字符 / `\t`）。
- 真实出图需要宿主上装好 `fuyubot.html2img`；模板本身可以用与 html2img 相同的
  `vue-template-html` 编译器离线验证（开发期工具，不必进仓库）。
- 请求样张（手动，导出 html2img 的请求 JSON）：

  ```bash
  cargo run --example render_card_kfc -- "文案" render-sample.json 800
  ```
