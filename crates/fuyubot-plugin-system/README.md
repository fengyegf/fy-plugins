# system — 系统信息卡片（Rust 插件）

TS 版 `E:\Fuyu_TDBot\plugins\system`（`index.ts` + `system.vue`，Vue + satori + resvg 管线）
的 Rust 迁移版：

`/system` → 采集宿主机的 CPU / 内存 / 磁盘 → 查询 TDLib 版本、机器人资料、
管理员资料与已加载插件 → 渲染成一张信息卡片 → 以照片回复命令消息 → 删除临时文件。

图像生成从 `Vue + satori + resvg + sharp`（Node 侧 `genImg.ts`）换成
[hyper-render](https://github.com/thomasmost/hyper-render) 的纯 Rust 方案：
「HTML + CSS → Blitz（Stylo + Taffy）布局 → Vello CPU 光栅化 → PNG」，
再把 HTML 透明层、渐变/图片背景、圆形头像用 `image` 合成成 JPEG，
**不需要 Node、不需要浏览器、不需要外部截图服务**，整个插件就是一个动态库。

## 预览

| 文件 | 说明 |
| --- | --- |
| `card-sample.jpg` | 离线样张（合成背景 + 合成头像，不联网） |
| `card-real.jpg` | 真实背景图 + 真实照片头像（`examples/render_real.rs` 联网出图） |

## 功能

- `/system` 一条命令出一张 1400×800 的信息卡片：
  - 机器人头像、名字、`@username`、ID；
  - 管理员（`[plugin.system] super_admin`）头像、名字、`@username`、ID 与留言提示；
  - TDLib 版本与 commit hash（`getOption`）；
  - CPU / RAM 使用率进度条（`sysinfo`，CPU 两次采样；SWAP 按需求去掉）；
  - 磁盘占用（Windows 盘符「磁盘C」；Linux 是「根目录」/挂载点、macOS 是卷名、
    Android 是「内部存储」/「外部存储」；已用 / 总量、百分比，最多 4 块）；
  - 宿主进程里已加载的插件胶囊（名字 + 运行状态），行数与面板高度**随右列剩余空间
    自适应**，常见数量（6–20 个）会全部画出，只有真的放不下才折成 `+N`；
- 所有外部依赖失败都**降级不炸**：TDLib 查询失败显示「未知」，头像拿不到画占位图，
  背景图下载失败退回渐变，插件清单读不到显示「暂无数据」。
- 临时 JPEG 在发送完成后自动删除；采集、下载、渲染都在独立业务线程里跑，不阻塞 TDLib。

## 与 `/help` 的联动

插件在 `start()` 里通过进程级共享服务 `fuyubot.help.commands` 的 `register`
方法，把 `/system` 登记进内置 `/help`，用户发 `/help` 就能看到这条命令。

依赖用 `soft_inject` 声明（写的是服务名，不是插件名）：有 help 就排在它后面，
没有也照常启动——登记失败只记一条 warn，不影响 `/system` 本身。服务名、方法名与
JSON 协议见插件文档的[「插件联动」](https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/linkage/)。

## 使用

```text
/system
```

命令前缀由 `[plugin.system] prefixes` 提供，缺省支持 `/`、`!`、`！`、`.` 和 `#`；
也接受 `/system@机器人名` 这种 Telegram 群聊写法。

## 处理流程

1. 解析命令（不是 `/system` 就静默忽略，和其它插件共存）。
2. `sysinfo` 采集系统信息：CPU 使用率隔 200ms 采两次，内存 / 磁盘一次（不采 SWAP）。
3. TDLib `getOption("version")` / `getOption("commit_hash")`。
4. `getMe` 取机器人资料与头像；配置了 `super_admin` 时再 `getUser` 管理员资料与头像。
5. 头像优先读 TDLib 已下载的 `profile_photo.big.local.path`；没有就发
   `downloadFile`（`priority` / `offset` / `limit` / `synchronous = true` 四个字段齐全），
   由 TDLib **下载完再回响应**，直接拿 `local.path` 读图；失败再退 `small` 缩略图。
   > 不能用「发 `downloadFile` 再等 `updateFile`」的写法：小头像往往在响应之前就下载完，
   > 那条通知会漏掉（宿主自带的 `download_file` 操作也漏了 `priority` / `synchronous`，
   > 实测会被 TDLib 直接拒绝）。
6. `list_plugins()` 读宿主进程里的插件清单（名字 + active/pending/failed 状态）。
7. 下载背景图（可缓存到系统临时目录；失败退回渐变）。
8. hyper-render 渲染 HTML 版面 → `image` 合成背景 / 圆头像 → JPEG。
9. 写入系统临时目录，以照片回复触发消息，然后删除临时文件。

## 项目结构

| 路径 | 说明 |
| --- | --- |
| `src/lib.rs` | 插件入口：实现 `Plugin`，建 worker、订阅 `updateNewMessage`、停机清理 |
| `src/host/` | 宿主适配层：channel 路由线程、simplify 消息网关（发文本 / 发照片） |
| `src/cmd/` | 命令层：文本命令解析、`/system` 业务流程 |
| `src/core/` | 与宿主无关：配置、系统信息采集、TDLib 查询、背景图、数据模型 |
| `src/render/` | hyper-render 卡片：HTML 模板、背景/头像合成、JPEG 输出 |
| `examples/` | 离线样张出图，调样式与验收用 |
| `testdata/` | 离线回放用最小配置与 `/system` 更新样例 |
| `plugin.json` | 插件元信息（native / ABI 6） |

分层原则：`host` 只谈宿主通信，`cmd` 只谈命令，`core` 只谈数据与网络，
`render` 只谈版面；任一层都可以单独 `cargo test`。

## 构建与部署

在仓库根目录执行：

```bash
cargo build --release -p fuyubot-plugin-system
```

产物：`target/release/system.dll`（Linux `libsystem.so`，macOS `libsystem.dylib`）。
部署时复制到宿主的插件目录：

```text
plugins/system/system.dll
```

静态检查导出符号（不加载、不联网、不连 Telegram）：

```bash
./fuyubot-sdk.exe check target/release/system.dll
```

`Cargo.lock` 在仓库根目录统一维护，必须一起提交；不要跑 `cargo update`。

## 配置

写在 `fuyubot.toml` 的 `[plugin.system]` 段，全部可选：

```toml
[plugin.system]
super_admin      = 123456789   # 超级管理员 user id；不配则 admin 一栏显示「未配置」
background_url   = "https://i0.hdslb.com/bfs/article/cc4a3e435ac47a6190aebf4f74a8675b88a183bb.jpg"
background_cache = true        # 背景图下载后缓存到系统临时目录
timeout_ms       = 5000        # 单次 TDLib 调用超时
avatar_timeout_ms = 20000      # 头像同步下载的超时（弱网 / 大图给足时间）
# prefixes       = ["/", "!", "！", ".", "#"]  # 不写就用缺省前缀
```

`background_url = ""` 可以关掉背景图（纯渐变，离线环境推荐）。
旧写法的大写键（`SUPER_ADMIN`、`BACKGROUND_URL`、`TIMEOUT_MS` 等）同样接受。

## 与 TS 版的差异

| | TS 版 | Rust 版 |
|---|---|---|
| 运行形态 | `plugins/system/index.ts`（Node 插件） | 单个 `system.dll`（宿主 dlopen） |
| 模板 | `system.vue`（Vue 运行时 + `<script setup>`） | `src/render/template.rs` 生成的静态 HTML |
| 渲染 | `satoriVue` → SVG → `@resvg/resvg-js` → `sharp` | hyper-render：Blitz 布局 → Vello CPU → PNG → image 合成 JPEG |
| 图片 | `<img>` / 背景图直接交给 satori | hyper-render 不加载 `<img>`，头像与背景由 `image` 在 Rust 侧合成 |
| 字体 | 仓库内 Noto Sans SC + 系统回退 | 只用系统字体（hyper-render 0.2 不支持 `@font-face`） |
| 系统信息 | `systeminformation` | `sysinfo`（CPU / 内存 / 磁盘；不取 SWAP、OS、主板、网络） |
| 管理员 | TS 版 `configs.admin.super_admin`（数据库） | `[plugin.system] super_admin`（宿主配置；Rust 宿主没有 admin 数据库段） |
| 插件清单 | `pluginManager.getPlugins()`（含版本） | 宿主 `list_plugins`（只有名字 + 状态，ABI 没有版本字段） |
| 失败策略 | 任意一步失败 → 整条命令回「获取系统信息失败」 | TDLib / 头像 / 背景各自降级，只有渲染失败才回错误 |
| 命令注册 | `cmdHandlers` | 文本命令解析（`/system`，前缀见上文） |
| 输出 | PNG（`quality: 1.5`） | JPEG 1400×800（quality 90，Telegram 照片更省流量） |

## 测试

```bash
cd crates/fuyubot-plugin-system
cargo test --offline --release                         # 单元测试：配置 / 系统信息 / TDLib 解析 / 命令解析 / 渲染
cargo run --offline --release --example render_card    # 内置样例出图（5 张，不联网）
cargo run --offline --release --example render_real    # 真实背景 + 真实照片头像（联网）
```

离线回放（不接 TDLib、不用账号，验证插件加载与 `/system` 命令链路）：

```bash
cd <fuyubot 发行目录>
./fuyubot.exe run --config fuyubot.toml \
  --replay <本插件>/testdata/replay_system.json \
  --plugin-dir plugins --replay-wait 30000
```

没有 TDLib 时日志会依次出现：系统信息采集完成 → getOption/getMe 超时降级 → 卡片已生成；
图片发送与错误提示会因没有 TDLib 而失败（属预期，回放只验到「卡片已生成」）。

## 已知限制

- hyper-render 只认系统字体，中文依赖系统 CJK 字体（Windows 上是微软雅黑）。
- `@font-face`、外部图片加载未支持；背景与头像所以在 Rust 侧合成。
- 磁盘按平台取标签：Windows 是盘符（「磁盘C」），Linux 是 `根目录` / 挂载点，
  macOS 是 `系统磁盘` / 卷名，Android 是 `内部存储` / `外部存储`；伪文件系统
  （proc / sysfs / tmpfs / 快照）与 Android 只读系统镜像不会占位，最多 4 块。
- 插件胶囊没有版本号：宿主 `list_plugins` 只暴露名字与状态。
- 后台图默认来自 TS 版 `system.vue` 里的 B 站图片；离线环境请配置
  `background_url = ""` 或预先把缓存文件放到系统临时目录。

## 许可

LGPL-3.0-or-later（与仓库内其它 Rust 插件一致）。
