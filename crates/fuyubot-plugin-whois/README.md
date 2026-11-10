# Whois 插件

基于 FuyuBot SDK Native ABI v5 的域名 Whois 查询插件。用户发送命令后，插件查询
域名注册信息与服务器 IP 归属，在本地渲染为 PNG 信息卡片，并以照片回复触发消息。

整个运行链路使用 Rust 实现，不依赖 Node、浏览器或外部截图服务。

## 功能

- 支持裸域名、完整 URL、端口、路径和中文域名。
- 展示域名状态、注册人与注册商、注册和到期时间、DNS 服务器、服务器 IP 归属与运营商。
- 卡片左上角优先显示网站自己的图标（`/favicon.ico` 或首页声明的图标），取不到时回退到内置图标。
- 根据注册和到期状态展示对应徽标。
- 未注册域名直接回复文本，不生成空卡片。
- Whois 接口返回明确错误时，将原因回复给用户。
- IP 归属查询失败时保留主体卡片，并使用已有数据或占位值兜底。
- 查询、渲染和发送均在独立业务线程中执行，不阻塞 TDLib 回调线程。
- 临时图片发送完成后立即删除。

## 与 `/help` 的联动

插件在 `start()` 里通过进程级共享服务 `fuyubot.help.commands` 的 `register`
方法，把 `/whois` 登记进内置 `/help`，用户发 `/help` 就能看到这条命令。

依赖用 `soft_inject` 声明（写的是服务名，不是插件名）：有 help 就排在它后面，
没有也照常启动——登记失败只记一条 warn，不影响 `/whois` 本身。服务名、方法名与
JSON 协议见插件文档的[「插件联动」](https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/linkage/)。

## 使用

```text
/whois example.com
/whois https://www.example.com/path
/whois 例子.中国
```

不带参数时返回用法提示。

命令前缀按以下顺序读取：

1. `[plugin.whois].prefixes`
2. 宿主顶层 `[config].PREFIXES`
3. 缺省前缀 `/`、`!`、`！`、`.`、`#`

## 处理流程

1. 解析命令并提取第一个参数。
2. 清理协议、用户信息、`www.`、端口、路径、查询串和末尾根点。
3. 请求 Whois 接口。
4. 未注册时回复文本并结束。
5. 查询服务器 IP 归属；失败时读取 Whois 数据中的同名字段兜底。
6. 抓取网站图标：`/favicon.ico` → `www.` 前缀 → 首页声明的图标；失败则用内置图标。
7. 合并数据并渲染 PNG 信息卡片。
8. 写入系统临时目录，发送照片，再删除临时文件。

## 项目结构

| 路径 | 说明 |
| --- | --- |
| `src/lib.rs` | 插件入口、订阅生命周期和启停管理 |
| `src/host/` | 宿主通信、TDLib 消息解析和消息网关 |
| `src/cmd/` | 命令解析、任务分发与 `/whois` 业务 |
| `src/core/` | 配置、接口请求、域名清洗和数据模型 |
| `src/render/` | HTML 模板、内联素材和 PNG 渲染 |
| `plugin.json` | 插件元信息与 ABI 声明 |

模块职责保持单向：`host` 负责宿主通信，`cmd` 负责任务编排，`core` 负责数据与网络，
`render` 负责版面与图像输出。

## 构建

在仓库根目录执行：

```bash
cargo build --release --locked -p fuyubot-plugin-whois
```

`--offline` 也可以加上，前提是本机 cargo 缓存里已经有过全部依赖。

产物位于 `target/release/whois.dll`；Linux / macOS 分别是 `libwhois.so` /
`libwhois.dylib`。`[lib] name` 固定为 `whois`，所以动态库文件名与 `plugin.json` 的
`entry` 始终对得上。

`Cargo.lock` 在仓库根目录统一维护，必须一起提交；不要随手 `cargo update`。

静态检查插件导出符号：

```bash
./fuyubot-sdk.exe check target/release/whois.dll
```

## 发布包

发布包按平台拆分，产物名形如 `whois-<version>-<platform>.zip`，ZIP 根目录直接是
`plugin.json` 与动态库，不套目录。

本地打包（在仓库根目录执行）：

```powershell
cargo build --release --locked -p fuyubot-plugin-whois
python scripts\package.py --plugin whois --platform windows-x86_64 --version 0.1.0
```

产物位于 `dist/whois-0.1.0-windows-x86_64.zip`。

Linux / macOS / Android 只切 `--platform`，脚本会按目标平台选库文件名并改写包内
`entry`。Android 需要额外装 target、NDK 和 `cargo-ndk`：

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --locked
cargo ndk -t arm64-v8a -t x86_64 --platform 24 build --release --locked -p fuyubot-plugin-whois
python scripts\package.py --plugin whois --platform android-aarch64 --version 0.1.0
python scripts\package.py --plugin whois --platform android-x86_64 --version 0.1.0
```

打包脚本会校验 `plugin.json` 的 `schema` / `id` / `kind` / `abi`，并核对命令行版本与
`Cargo.toml`、`plugin.json` 一致；生成 ZIP 后还会重新打开检查条目、`version`、
`platforms` 与 `entry`，避免发出装不上的包。

多平台构建由仓库根目录的 `.github/workflows/release.yml` 完成：

- 推送 `whois-v<版本>` tag：只构建 `plugin.json` 声明的五个平台，打包后挂到 Release；
- 手动触发（Actions → release → Run workflow）：只产出各平台 artifact，不建 Release。

```powershell
git tag whois-v0.1.0
git push origin whois-v0.1.0
```

从 Actions 页面下载的 artifact 外层会被 GitHub 再包一层 ZIP，真正用于安装的是其中的
`whois-<version>-<platform>.zip`，或者 Release 页面上的同名资产。

## 部署

发布包解压后就是可直接加载的插件目录，把它放进宿主的插件目录：

```text
plugins/whois/
├── plugin.json
└── whois.dll          # Linux / macOS 为 libwhois.so / libwhois.dylib
```

`entry` 必须与动态库文件名一致；仓库里的 `plugin.json` 是 Windows 入口，打包脚本会按
目标平台改写 `entry`、`platforms` 和 `version`。

`target/`、`dist/`、动态库、压缩包和本地预览图片均已加入 `.gitignore`，不应提交
到源码仓库。

## 配置

在 `fuyubot.toml` 中增加 `[plugin.whois]` 段。所有字段均可选：

```toml
[plugin.whois]
whois_api_url = "https://whoiscx.com/api/whois/info/"
ip_api_url    = "https://whoiscx.com/api/whois/ip/"
width         = 1200
timeout_ms    = 15000
favicon       = true
favicon_timeout_ms = 6000
prefixes      = ["/", "!", "！", ".", "#"]
```

| 字段 | 缺省值 | 说明 |
| --- | --- | --- |
| `whois_api_url` | `https://whoiscx.com/api/whois/info/` | Whois 查询接口 |
| `ip_api_url` | `https://whoiscx.com/api/whois/ip/` | IP 归属查询接口 |
| `width` | `1200` | 卡片宽度，限制在 `900` 至 `1600` 像素 |
| `timeout_ms` | `15000` | 单次 HTTP 查询超时，单位毫秒 |
| `favicon` | `true` | 是否抓取网站图标，关掉后固定使用内置图标 |
| `favicon_timeout_ms` | `6000` | 抓取网站图标的整体预算，限制在 `500` 至 `30000` 毫秒 |
| `prefixes` | 跟随宿主配置 | 插件识别的命令前缀 |

## 运行行为

- 卡片宽度低于 `900px` 时两列信息会过度挤压，因此由插件自动夹取到允许范围。
- 网站图标只从目标站点自己取（`/favicon.ico`、`www.` 前缀、首页 `<link rel="icon">`），
  不经过第三方图标服务；站点不可达、返回 403 或没有图标时，卡片回退到内置的渐变地球图标。
- 图标抓取有独立预算，最多让 `/whois` 多等 `favicon_timeout_ms`（缺省 6 秒）；IP 字面量
  和 `localhost` 这类主机名不会去抓，避免被用来探测内网。
- 系统缺少合适的中文字体时，卡片中的 CJK 字符可能使用系统回退字体。
- Whois 数据来自配置的公开接口；接口不可用或限流时，命令会返回失败提示。
- 插件停止时会等待当前阻塞操作到达安全边界；图片上传网关的超时上限为 120 秒。
