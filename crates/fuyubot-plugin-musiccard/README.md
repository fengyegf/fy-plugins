# FuyuBot MusicCard

网易云音乐卡片插件，基于 FuyuBot Plugin SDK 构建。收到歌曲 ID、网易云歌曲链接或
搜索关键词后，插件会获取歌曲信息和歌词，把卡片数据交给 Vue 模板（`assets/card.html`），
由宿主上的 [`fuyubot.html2img`](https://github.com/xiaoqvan/fuyubot-plugins/tree/main/crates/fuyubot-plugin-html2img)
服务（`vue: true`）渲染成 `650 x 1220` 的 JPEG，再以照片回复触发命令的消息。

界面（HTML / CSS / 循环 / 条件 / 插值）全在模板里，Rust 只做数据映射：封面直接用
网易云 CDN 的公网 URL，二维码以 `data:image/png;base64,…` 内联；插件不拼 HTML、
不链接 hyper-render，也不做任何位图合成。

## 功能

- 支持歌曲 ID、网易云歌曲链接和关键词搜索。
- 自动读取中英文歌词，并按时间匹配原文与译文。
- 生成包含封面、歌曲信息、歌词、日期和歌曲二维码的卡片。
- 处理期间发送进度消息，完成后自动删除。
- 失败时回复具体原因，并清理渲染产物与进度消息。

## 依赖

- **必须安装 `fuyubot.html2img` 插件**：本插件用 `Plugin::inject` 声明了硬依赖，
  宿主会在渲染服务就绪后再启动 musiccard；没装时插件停在 Pending（不会静默失败）。
- 渲染器是 hyper-render 的本地 fork（Blitz + Stylo + Taffy + Vello CPU），纯 Rust、
  无 Chromium / Node；它只存在于 html2img 插件里，本插件不再链接渲染引擎。
- 卡片模板 `assets/card.html` 在编译期嵌入（`include_str!`），只有几 KiB；
  封面是网易云 CDN 的公网 URL，由 html2img 服务下载（单张上限 8 MiB）；二维码以
  data URI 内联在 `data` 里。模板远低于服务的 128 KiB 上限。

## 与 `/help` 的联动

插件在 `start()` 里通过进程级共享服务 `fuyubot.help.commands` 的 `register`
方法，把 `/musiccard` 登记进内置 `/help`，用户发 `/help` 就能看到这条命令。

依赖用 `soft_inject` 声明（写的是服务名，不是插件名）：有 help 就排在它后面，
没有也照常启动——登记失败只记一条 warn，不影响 `/musiccard` 本身。服务名、方法名与
JSON 协议见插件文档的[「插件联动」](https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/linkage/)。

## 使用

```text
/musiccard 347230
/musiccard https://music.163.com/song?id=347230
/musiccard 周杰伦 晴天
```

无参数时会返回用法提示。命令前缀由宿主顶层配置 `[config] PREFIXES` 提供，默认支持
`/`、`!`、`！`、`.` 和 `#`。

## 处理流程

1. 解析命令并提取歌曲 ID、链接或关键词。
2. 调用网易云接口完成搜索、歌曲详情与歌词获取。
3. 筛选最多 5 行歌词并匹配译文；封面只保留接口给的公网 URL，不再下载。
4. 把卡片映射成 Vue 模板的 `data`（标题 / 歌手 / 专辑 / 封面 URL / 二维码 / 日期 /
   歌词 / 适配字号），调 `fuyubot.html2img`（`vue: true`）渲染成 `650 x 1220` 的 JPEG。
5. 把服务返回的本机图片路径直接作为照片发送，并回复原消息。
6. 删除渲染产物与进度消息。

任何一步失败都会结束当前任务并回复错误原因，不会影响其他正在执行的命令。

## 项目结构

| 路径 | 说明 |
| --- | --- |
| `src/lib.rs` | 插件入口、生命周期与消息订阅 |
| `src/config.rs` | 插件配置读取（命令前缀、接口超时） |
| `src/host/` | 宿主消息收发、更新路由与业务线程 |
| `src/cmd/` | 命令解析、分发与卡片业务流程 |
| `src/netease/` | 搜索、歌曲详情、歌词与 LRC 解析（封面只取 URL） |
| `assets/card.html` | 卡片界面：`<style>` + Vue 模板（循环 / 条件 / 插值） |
| `src/render/` | 卡片数据映射（`view_data`）、二维码 data URI、html2img 服务客户端 |
| `src/model.rs` | 跨模块共享的数据结构 |
| `src/util/` | 文本处理等通用工具 |
| `plugin.json` | 插件商城 bundle 清单 |

## 配置

插件默认可以直接运行，所有配置都是可选的：

```toml
[plugin.musiccard]
request_timeout_ms = 15000   # 接口超时（缺省 15s）

[config]
PREFIXES = ["/", "!", "！", ".", "#"]
```

卡片不再落盘到本地：渲染产物由 `fuyubot.html2img` 服务写在它自己的临时目录里，
插件发完照片就删除。老配置里残留的 `temp_dir` 会被安静忽略。

## 构建

在仓库根目录执行：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p fuyubot-plugin-musiccard
```

发布产物为 `target/release/musiccard.dll`（Linux `libmusiccard.so`，macOS
`libmusiccard.dylib`）。`[lib] name` 固定为 `musiccard`，文件名与 `plugin.json` 的
`entry` 始终一致。

`Cargo.lock` 在仓库根目录统一维护，必须一起提交；不要随手 `cargo update`。

检查动态库导出符号与 ABI：

```powershell
./fuyubot-sdk.exe check target\release\musiccard.dll
```

## 发布包

商城使用按平台打包的 ZIP。Windows x86_64 本地打包（在仓库根目录执行）：

```powershell
python scripts\package.py --plugin musiccard --platform windows-x86_64 --version 0.1.1
```

产物位于 `dist/musiccard-0.1.1-windows-x86_64.zip`。

ZIP 根目录直接包含 `plugin.json` 与 `bin/musiccard.dll`，不套目录；Linux、macOS 和
Android 用同一个脚本，只切 `--platform`，包内入口会按平台改成
`bin/libmusiccard.so` / `bin/libmusiccard.dylib`。

Android 需要 Rust Android targets、Android NDK 和 `cargo-ndk`：

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --locked
cargo ndk -t arm64-v8a -t x86_64 --platform 24 build --release -p fuyubot-plugin-musiccard
python scripts\package.py --plugin musiccard --platform android-aarch64 --version 0.1.1
python scripts\package.py --plugin musiccard --platform android-x86_64 --version 0.1.1
```

GitHub Actions 页面下载到的 artifact 外层会被再包一层 ZIP；真正用于安装的是其中的
`musiccard-<version>-<platform>.zip`，或者 Release 页面里的同名资产。

推 `musiccard-v<版本>` tag 时，Actions 只构建 `plugin.json` 声明的五个平台并上传
Release：

```powershell
git tag musiccard-v0.1.1
git push origin musiccard-v0.1.1
```

商城收录仍由 `FuyuBot-plugins` 仓库的 `plugins/musiccard.yml` 指向本仓库的 Release
（`repo` + `tag` + 资产文件名）。

## 安装

将构建产物复制到主程序的 `plugins` 目录：

```powershell
Copy-Item target\release\musiccard.dll <FuyuBot目录>\plugins\
```

或使用主程序提供的安装命令：

```powershell
<FuyuBot目录>\fuyubot.exe plugin install target\release\musiccard.dll
```

启动后可以在日志中确认插件已激活。使用 `--no-tdlib` 可以在不连接 Telegram 的情况下
检查插件是否能够正常加载。

## 实现说明

- 卡片尺寸固定为 `650 x 1220`：`scale = 1.0`、`format = jpg`、`auto_height = false`。
- 界面在 `assets/card.html`（`<style>` + 原 `musiccard.vue` 的模板正文），Rust 只把
  `Card` 映射成 `data` 再调 `fuyubot.html2img`（`vue: true`），不拼 HTML、不链接渲染引擎。
- `data` 字段：`title` / `artists` / `album` / `cover` / `qrCode` / `month` / `day` /
  `lyrics`（每项 `{text, translation}`），外加模板里算不出来的排版值 `titleSize` /
  `artistsSize`（按 454px 文字列预算的适配字号；vue-template-html 没有函数调用 / Math）。
- 封面直接写网易云 CDN 的公网 URL（背景 `opacity:.34` + 卡片内 546x546 封面）；
  不可用时 `cover` 传 `null`，模板只留渐变底与封面占位，其余版面不变。二维码仍在
  本地用 `qrcode` 生成 PNG，再以 `data:image/png;base64,…` 内联。
- 虚线用一排 2px 小方块拼（Blitz 的 `border-style:dashed` 不可靠），在模板里由
  `v-for` 生成，Rust 不参与。
- 网络请求使用阻塞式 `ureq`，整体跑在命令专用业务线程中，不阻塞宿主的消息接收线程；
  html2img 的服务调用同样是同步阻塞的，只在业务线程里调。
- 歌词按字符截断，避免中文和 emoji 被字节边界切坏。
- 卡片优先使用系统字体，常见中文环境会回退到 Microsoft YaHei 或 PingFang SC。

### 模板校验（不依赖 html2img）

模板是纯 Vue 模板，可以用仓库里的 `target/vuecheck` 本地编译 + 渲染核对：

```powershell
cargo test -p fuyubot-plugin-musiccard   # 顺带把请求 / data 样张写到 target/qa/json/
cd target/vuecheck
cargo run --quiet -- ../../crates/fuyubot-plugin-musiccard/assets/card.html ../qa/json/musiccard-data.json ../qa/json/out.html
```

输出 `OK … bytes` 说明模板可编译、可渲染（示例约 26 KiB，远低于 128 KiB）。
样张覆盖三个分支：正常封面、无封面（`cover: null`）、超长标题（`titleSize` 缩到下限）。

也可以直接从 example 导出请求 / data：

```powershell
cargo run --example render_card_musiccard -- --out card.json --data data.json
```
