# FuyuBot PPXFY

FuyuBot 的下载与 Galgame 信息插件。它支持多链接下载、压缩包解压、目录浏览与
分卷打包，也可以把 DMM / DLSite 作品整理成分享消息和 Telegraph 预览页。

## 功能

- `/d`：交互式接收多个下载链接，并发下载、解压、浏览目录并打包上传。
- `/p`：解析 DMM / DLSite 作品页，补充信息并生成分享消息。
- `/cancel`：取消当前聊天的活跃任务并清理下载缓存。
- `/clearcache`：清理没有任务登记的残留缓存。
- 可选接入 `mongo-plugin`，把 Galgame 作品记录写入 `storage.mongo`。
- 支持 ZIP、RAR、7z 等常见压缩格式，7z 不可用时回退到内置 ZIP 处理。
- 支持乱码文件名修复与超大压缩包分卷。

## 与 `/help` 的联动

插件在 `start()` 里通过进程级共享服务 `fuyubot.help.commands` 的 `register`
方法，把 `/download`、`/galgame`、`/cancel`、`/clearcache` 四条命令登记进内置
`/help`，用户发 `/help` 就能看到它们（别名 `/d`、`/p` 写在说明里）。

`soft_inject` 里同时写了 `fuyubot.help.commands`（联动）和 `storage.mongo`
（可选入库）两个**服务名**：有 help 就排在它后面，没有也照常启动——登记失败只记
一条 warn，不影响插件本身。服务名、方法名与 JSON 协议见插件文档的
[「插件联动」](https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/linkage/)。

## 命令

### 下载

```text
/d
/download
/downloadcommand
```

发送命令后按提示回复下载链接，可以逐条添加多个链接，回复 `OK` 开始处理。插件会：

1. 并发下载所有链接并显示多行进度。
2. 选择可用的压缩包。
3. 自动尝试无密码解压，必要时询问密码。
4. 展示目录树，删除不需要的内容。
5. 选择目录打包，并按大小自动分卷。
6. 上传生成的压缩包。

### Galgame

```text
/p <作品页链接>
/galgame <作品页链接>
/galgamecommand <作品页链接>
```

插件会读取作品信息、封面、图集和购买链接，必要时通过回复补充信息，然后发送作品
消息。配置 Telegraph token 后还会创建预览页。

### 任务与缓存

```text
/cancel
/clearcache
```

`/cancel` 会结束当前聊天的等待步骤并清理任务产物；`/clearcache` 用于清理异常退出
后遗留的 `Output/`、`ZipOutput/` 和缓存根目录文件。

命令前缀由宿主顶层 `[config] PREFIXES` 提供，默认支持 `/`、`!`、`！`、`.` 和 `#`。

## 配置

配置位于 `fuyubot.toml` 的 `[plugin.ppxfy]` 段：

```toml
[plugin.ppxfy]
cache_dir = "cache"
listen_timeout_ms = 600000
unzip_timeout_ms = 1800000
archive_size_threshold_mb = 1843.2
download_concurrency = 4
split_threshold_mb = 1900
seven_zip = "7z"
fix_encoding = true
telegraph_token = ""
telegraph_author = "@ppxfy - Galgame"
telegraph_author_url = "https://t.me/ppxfy"
dlsite_proxy = ""
mongo_collection = "ppxfy_galgame"
mongodb_db = ""
```

常用字段：

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `cache_dir` | `cache` | 下载、解压和打包的工作目录 |
| `listen_timeout_ms` | `600000` | 等待用户回复的超时 |
| `unzip_timeout_ms` | `1800000` | 7z 解压超时 |
| `archive_size_threshold_mb` | `1843.2` | 压缩包大小提示阈值 |
| `download_concurrency` | `4` | 同时下载的文件数，限制为 1 到 16 |
| `split_threshold_mb` | `1900` | 打包分卷阈值 |
| `seven_zip` | `7z` | 7z 可执行文件名称或绝对路径 |
| `fix_encoding` | `true` | 修复 Shift-JIS / GBK 文件名乱码 |
| `telegraph_token` | 空 | Telegraph 账号 token |
| `dlsite_proxy` | 空 | DLSite 请求代理 |
| `mongo_collection` | `ppxfy_galgame` | Galgame 记录集合 |
| `mongodb_db` | 空 | 覆盖 mongo-plugin 默认库 |

### 凭据

凭据优先从 `[plugin.ppxfy]` 读取，其次读取环境变量，最后读取工作目录或
`plugins/ppxfy/.env`：

```dotenv
PPXFY_TELEGRAPH_TOKEN=
PPXFY_DLSITE_PROXY=
```

可以复制 `.env.example` 为 `.env` 后填写。`.env` 已被忽略，不要提交真实凭据。

### MongoDB

Galgame 记录通过 `storage.mongo` 服务写入，连接串和默认库由 `mongo-plugin` 管理：

```toml
[plugin.mongo-plugin]
uri = "mongodb://127.0.0.1:27017"
db = "ppxfy"
```

没有安装或启动 `mongo-plugin` 时，下载和 Galgame 分享仍可正常使用，只是不入库。

## 依赖

- Rust 1.93 或更高版本。
- 处理 RAR、7z 等格式时需要系统安装 `7z`，或在配置中指定路径。
- Galgame 预览页需要 Telegraph token。
- 数据库记录需要 `mongo-plugin`，属于软依赖。

## 构建

在仓库根目录执行：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p fuyubot-plugin-ppxfy
```

Windows 产物：`target/release/ppxfy.dll`（Linux `libppxfy.so`，macOS
`libppxfy.dylib`）。

检查插件导出符号与 ABI：

```powershell
./fuyubot-sdk.exe check target\release\ppxfy.dll
```

依赖中的 `zip` 使用仓库根目录的 [vendor/zip](../../vendor/zip) 本地补丁，修复
Windows 下日文文件名触发的路径切片 panic。`Cargo.lock` 统一在仓库根目录维护，不要
随意执行 `cargo update`。

## 安装

将构建产物复制到宿主的插件目录：

```powershell
Copy-Item target\release\ppxfy.dll <FuyuBot目录>\plugins\ppxfy\
```

重启宿主后确认 `ppxfy` 已加载。需要 7z 或 mongo-plugin 时，同时确认对应程序和插件
已安装。

## 项目结构

```text
src/
  lib.rs            插件入口、生命周期与依赖声明
  cmd/              下载、Galgame、取消、缓存清理与命令解析
  core/             配置、HTTP、压缩包、页面与目录树处理
  db/               storage.mongo 客户端与 Galgame 记录模型
  galgame/          DMM / DLSite 页面解析与分享文本
  host/             消息、任务、进度、回复等待与线程调度
  package/          分卷压缩与上传准备
  telegraph/        Telegraph 页面生成
  unpack/           解压、密码交互与文件名修复入口
  util/             文件、文件名修复与 7z 调用
```
