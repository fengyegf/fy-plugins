# PPXFY（DMM / DLSite 下载与分享）

`ppxfy` · v0.1.5 · ABI v6 · Windows x86_64 / Linux x86_64 / macOS aarch64

## 项目介绍

PPXFY 是 FuyuBot 的下载与 Galgame 信息插件：把聊天里给出的下载链接变成可直接取用的分卷压缩包，把 DMM / DLSite 作品页整理成带预览的分享消息。

- `/d`：逐条收集链接（回复 `OK` 开始）→ 并发下载 → 自动解压（必要时询问密码）→ 目录树删减 → 按体积分卷打包 → 上传。
- `/p`：抓取作品页 → 解析封面 / 图集 / 购买链接 → 生成 Telegraph 预览页 → 逐项回复核对 → 可选写入 `storage.mongo` → 发送分享消息。

## 目录结构

```text
crates/fuyubot-plugin-ppxfy/
  Cargo.toml
  plugin.json
  config.default.toml
  README.md
  src/
    lib.rs            入口、生命周期与软依赖声明
    help.rs           向宿主命令表登记 /download /galgame /cancel /clearcache
    cmd/              命令层：download / galgame / cancel / clearcache / parser
    core/             配置、HTTP、压缩包识别、页面抓取、目录树
    db/               storage.mongo 客户端与作品记录模型
    galgame/          DMM / DLSite 页面解析与分享文本
    host/             消息、命令回复、任务、进度、回复等待、线程调度
    package/          分卷打包与上传准备
    telegraph/        Telegraph 页面生成
    unpack/           解压、密码交互、文件名修复入口
    util/             文件操作、乱码修复、7z 调用
```

部署后（ZIP 根 = 插件目录内容）：

```text
plugins/ppxfy/
  plugin.json
  bin/ppxfy.dll          # Linux: libppxfy.so / macOS: libppxfy.dylib
  config.default.toml
```

## 命令一览

| 命令 | 权限 / 范围 | 作用 |
| --- | --- | --- |
| `/d` / `/download` / `/downloadcommand` | 所有用户 · 私聊 / 群组 | 交互式下载：逐条回复链接，`OK` 开始；并发下载 → 解压 → 目录树 → 分卷打包上传 |
| `/p` / `/galgame` / `/galgamecommand` `<链接>` | 所有用户 · 私聊 / 群组 | 解析 DMM / DLSite 作品页，核对信息后发送分享消息，并按配置入库 |
| `/cancel` | 所有用户 · 私聊 / 群组 | 取消当前聊天的活跃任务，并清理本次流程的临时文件 |
| `/clearcache` | 所有用户 · 私聊 / 群组 | 清理没有任务登记的残留缓存（`Output/`、`ZipOutput/` 与缓存根目录文件） |

命令名不区分大小写，历史长名 `downloadcommand`、`galgamecommand` 仍可用。前缀来自宿主顶层 `[config].PREFIXES`（默认 `/`、`!`、`！`、`.`、`#`），本插件没有自己的 `prefixes` 配置项。

## 配置一览

配置段 `[plugin.ppxfy]`，键名同时接受 snake_case 与全大写写法。

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `cache_dir` | `cache` | 下载 / 解压 / 打包的工作目录（相对运行目录，其下固定 `Output/`、`ZipOutput/`） |
| `listen_timeout_ms` | `600000` | 等待用户回复（链接 / 密码 / 编号 / 包名）的超时 |
| `unzip_timeout_ms` | `1800000` | 7z 兜底解压的超时 |
| `archive_size_threshold_mb` | `1843.2` | 压缩包体积提示阈值，只记日志、不改变流程 |
| `download_concurrency` | `4` | 并发下载数，限制在 1–16 |
| `split_threshold_mb` | `1900` | 打包分卷阈值（MB） |
| `seven_zip` | `7z` | 7z 可执行文件名或绝对路径 |
| `fix_encoding` | `true` | 解压后自动修复 Shift-JIS / GBK 乱码文件名 |
| `telegraph_token` | 空 | Telegraph 账号 token；留空不生成预览页 |
| `telegraph_author` | `@ppxfy - Galgame` | 预览页作者名 |
| `telegraph_author_url` | `https://t.me/ppxfy` | 预览页作者链接 |
| `dlsite_proxy` | 空（直连） | DLSite 请求代理，不写 = 直连 |
| `mongo_collection` | `ppxfy_galgame` | 作品记录集合（经 `storage.mongo` 读写） |
| `mongodb_db` | 空（用服务端库） | 覆盖 mongo-plugin 的默认库名，不写 = 用服务端配置 |

凭据类字段（`telegraph_token` / `dlsite_proxy`）取值优先级：配置段 > 环境变量（`PPXFY_TELEGRAPH_TOKEN`、`PPXFY_DLSITE_PROXY`）> 运行目录或 `plugins/ppxfy/.env`。

## 使用说明

**安装**：从插件商店安装；或 `fuyubot plugin install ./ppxfy`；或把发布包解压到 `plugins/ppxfy/` 后重启宿主。

最小配置：

```toml
[plugin.ppxfy]
cache_dir = "cache"
listen_timeout_ms = 600000
download_concurrency = 4
split_threshold_mb = 1900
seven_zip = "7z"                 # 解压 RAR / 7z 需要系统安装 7z
telegraph_token = ""             # 要 Telegraph 预览页再填
```

**典型用法**：

1. 发送 `/d`，按提示逐条回复下载链接，最后回复 `OK` 开始；流程中可按编号删减目录，`/cancel` 随时中断。
2. 发送 `/p <作品页链接>` 解析 DMM / DLSite 作品，按提示补充信息后发出分享消息。
3. 异常退出后若残留文件，发送 `/clearcache` 清理。

**构建与打包**：

```bash
cargo build --release -p fuyubot-plugin-ppxfy
python scripts/package.py --plugin ppxfy --platform windows-x86_64
```

**发布**：`git tag ppxfy-v0.1.5 && git push origin ppxfy-v0.1.5`。

## 项目依赖（软/硬）

| 类型 | 名称 | 说明 |
| --- | --- | --- |
| 硬依赖 | 无 | 不装任何插件即可运行；`fuyubot.host.commands`、`fuyubot.host.command` 属宿主内建服务，不算插件依赖 |
| 软依赖 | `storage.mongo` | 由 mongo-plugin 提供；未安装时 `/p` 照常分享，只是不入库 |
| 系统 | `7z` | 解压 RAR / 7z 需要；只有 ZIP 时用内置解压兜底 |
| 宿主 | ABI v6 | 低于 v6 的宿主无法加载本插件 |
