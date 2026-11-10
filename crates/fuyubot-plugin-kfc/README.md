# kfc — 点个肯德基吧（Rust 插件）

TS 版 `E:\Fuyu_TDBot\plugins\kfc`（`index.ts` + `kfc.vue`）的 Rust 迁移版：

`/kfc` → 抓一句「肯德基疯话」→ 渲染成一张推特风格卡片 → 以照片回复命令消息 → 删临时文件。

图像生成从 `Vue + satori + resvg + sharp`（Node 侧 `genImg.ts`）换成
[hyper-render](https://github.com/thomasmost/hyper-render) 的纯 Rust 方案：
「HTML + CSS → Blitz（Stylo + Taffy）布局 → Vello CPU 光栅化 → PNG」，
**不需要 Node、不需要浏览器、不需要外部截图服务**，整个插件就是一个动态库。

与 TS 版的差异：

| | TS 版 | Rust 版 |
|---|---|---|
| 运行形态 | `plugins/kfc/index.ts`（Node 插件） | 单个 `kfc.dll`（宿主 dlopen） |
| 模板 | `kfc.vue`（Vue 运行时 + `<script setup>`） | `src/render/template.rs` 生成的静态 HTML |
| 渲染 | `satoriVue` → SVG → `@resvg/resvg-js` → `sharp` | hyper-render：Blitz 布局 → Vello CPU → PNG |
| 字体 | 仓库内 Noto Sans SC + Windows 回退字体 | 只用系统字体（hyper-render 0.2 不支持 `@font-face`） |
| 头像 | 模板内 `<img src="data:image/jpeg;base64,…">` | 内联 `<svg>` 里的 `<image href="data:…">`（hyper-render 不加载 `<img>` 的 data URI） |
| 缓存 | `getImgCache` 查 MongoDB 图片表 | 无缓存：首张 ~60 ms（字体集合初始化），之后每张 ~15 ms |
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

## 与 `/help` 的联动

插件在 `start()` 里通过进程级共享服务 `fuyubot.help.commands` 的 `register`
方法，把 `/kfc` 登记进内置 `/help`，用户发 `/help` 就能看到这条命令。

依赖用 `soft_inject` 声明（写的是服务名，不是插件名）：有 help 就排在它后面，
没有也照常启动——登记失败只记一条 warn，不影响 `/kfc` 本身。服务名、方法名与
JSON 协议见插件文档的[「插件联动」](https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/linkage/)。

## 命令

- `/kfc`：抓取并出图。接口失败 / 空文案 / 渲染失败都会回一条文本说明，
  不会静默失败；命令消息本身会被回复引用。

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
    messenger.rs    sendMessage / sendPhoto 的 simplify 封装
    worker.rs       回调 → channel → 路由线程 → 业务线程
  render/
    mod.rs          渲染入口 render_card()、串行锁、高度上限、宽度夹取
    template.rs     HTML 模板（纯函数，可单测）
    assets.rs       头像 SVG、认证蓝标、5 个互动图标
    samples.rs      调样式用的样张生成器（仅 `#[cfg(test)]`，默认 ignored）
  tests/
    acceptance.rs   只走公开 API 的集成验收用例
assets/
  avatar.jpg        KFC 老爷爷头像（从 kfc.vue 的 base64 拆出来，便于查看）
  avatar.b64        同一张图的 base64，编译期内嵌进代码
```

线程模型（三条硬约束决定）：

1. `updateNewMessage` 回调跑在宿主的 TDLib 接收线程上，**不能阻塞**；
2. 发消息 / 上传图片要等响应，而响应同样由接收线程投递——在回调里等就是自己等自己；
3. 插件卸载前必须 join 自己起的线程，否则动态库被卸载后仍有代码在跑。

所以：回调只把更新丢进 channel，常驻 worker 线程解析并分发，每条命令再开一个短命线程
跑完整流程。

## 渲染管线

```text
文案
  ↓ text::normalize          换行归一、控制字符清理
  ↓ template::build          静态 HTML（头像 / 蓝标 / 图标都是内联 SVG + data URI）
  ↓ hyper-render             Blitz 布局（flex / 自动高度）+ Vello CPU 光栅化
  ↓ PNG                      写临时文件 → send_photo 上传 → 删除
```

## 已知限制

- hyper-render 0.2 **不加载 `<img>` 的图片**（`data:` URI 与 http 都不行）；
  头像走「内联 SVG + `<image href="data:…">`」的迂回路径（已验证可用）。
- 只认系统字体（`@font-face` 未支持）：中文依赖系统装了的 CJK 字体（Windows 默认有），
  所以观感与 TS 版的 Noto Sans SC 略有差别。字体链固定为
  `Segoe UI → Segoe UI Emoji → Segoe UI Symbol → Microsoft YaHei`：
  接口文案偶尔带 `◁ ▷ ↻ ❚ ♬` 这类符号，少了 `Segoe UI Symbol` 就是豆腐块；
  但 `Segoe UI Emoji` 必须排在它**前面**，否则 emoji 会被 Symbol 的单色字形抢走。
  少数旗帜类 emoji 仍不完美：可能显示成方块，或退化成区域指示字母（🇨🇳 → `CN`）。
- 宽度夹在 460–1600：低于 460 时内容列放不下「头像 + 昵称 + 蓝标 + 句柄」，
  句柄会断在半个词上，用户名行进入换行兜底后卡片底部还会多出一条空白带
  （实测 400–456 有 96–139px），所以直接抬高下限而不是渲染出残版式。
- 不执行 JavaScript —— 模板是静态 HTML/CSS，正好不需要。

## 测试

```bash
cd crates/fuyubot-plugin-kfc
cargo test
```

- 单元测试（`src/**`）：命令解析、配置解析、文案清洗（换行 / 控制字符 / `\t`）、
  HTML 转义、模板尺寸、渲染合法性（PNG 签名 / 高度随行数增长 / 宽度夹取 / 极端输入不 panic）。
- 集成测试（`tests/acceptance.rs`）：只走公开 API 的验收用例（合法 PNG、长文本更高、
  宽度生效、转义、配置默认值）。
- 视觉样张（手动）：

  ```bash
  cargo test --lib render_samples -- --ignored --nocapture   # 写到 target/samples/
  cargo run --example render_card -- "文案" out.png 800       # 单张，可指定宽度
  ```

