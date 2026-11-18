# FuyuBot Plugins（Rust 原生插件源码仓）

仓库：<https://github.com/fengyegf/fy-plugins>

一个 Cargo workspace 管理全部原生插件。每个插件是一个**独立版本的库 crate**
（`rlib` + `cdylib`），可以单独打 tag、单独构建、单独发布，互不牵连。

发布产物是 FuyuBot 插件商城契约要求的 bundle ZIP，收录到
[CatMoeCircle/FuyuBot-plugins](https://github.com/CatMoeCircle/FuyuBot-plugins)；
本仓不往 crates.io 发布。

## 插件一览

| id | crate | 版本 | 平台 | 入口 |
| --- | --- | --- | --- | --- |
| `whois` | `crates/fuyubot-plugin-whois` | 0.1.4 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `whois.dll` |
| `musiccard` | `crates/fuyubot-plugin-musiccard` | 0.1.4 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `bin/musiccard.dll` |
| `epic` | `crates/fuyubot-plugin-epic` | 0.1.4 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `epic.dll` |
| `kfc` | `crates/fuyubot-plugin-kfc` | 0.1.5 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `kfc.dll` |
| `ppxfy` | `crates/fuyubot-plugin-ppxfy` | 0.1.5 | Windows / Linux / macOS（**不含 Android**） | `ppxfy.dll` |
| `system` | `crates/fuyubot-plugin-system` | 0.1.4 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `system.dll` |
| `trending` | `crates/fuyubot-plugin-trending` | 0.1.0 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `trending.dll` |
| `bidibot` | `crates/fuyubot-plugin-bidibot` | 0.1.0 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `bidibot.dll` |
| `rusto` | `crates/fuyubot-plugin-rusto` | 0.1.0 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `rusto.dll` |

平台列表的唯一来源是各 crate 的 `plugin.json`；发布流水线按它生成构建矩阵。

> **图像生成**：`epic` / `kfc` / `musiccard` / `system` / `trending` / `whois` 的卡片图统一由宿主上的
> [`fuyubot.html2img`](https://github.com/xiaoqvan/fuyubot-plugins/tree/main/crates/fuyubot-plugin-html2img)
> 插件渲染（`host.call_service("fuyubot.html2img", "render", …)`）：每个插件的卡片界面是
> `assets/card.html` 里的 **Vue 模板 + 内联 CSS**，Rust 只负责把业务数据凑成 `data`，
> 渲染时传 `vue: true`；渲染器（hyper-render + Blitz + Vello，纯 Rust、无 Chromium）全仓只有一份。
> 这几个插件都用 `Plugin::inject` 声明了**硬依赖**，没装 html2img 时会停在 Pending，
> 不会启动后每条命令都失败。`ppxfy` 不做图像生成，没有这个依赖。

### 平台验证状态

表里的平台都已至少构建并打包过一次，但**只验证「能编出 cdylib + bundle」，
没有做对应系统的真机运行**：

| 平台 | 验证方式 | 结果 |
| --- | --- | --- |
| `windows-x86_64` | 本机 `cargo build --release` + `fuyubot-sdk.exe check` + `package.py` | 6/6 通过 |
| `linux-x86_64` | GitHub Actions `verify-platforms`（ubuntu-latest） | 6/6 通过 |
| `macos-aarch64` | GitHub Actions `verify-platforms`（macos-15） | 6/6 通过 |
| `android-aarch64` / `android-x86_64` | 本机 NDK 27.2 + `cargo ndk` + `package.py`（whois / musiccard / epic / kfc / system） | 10/10 通过 |

> `trending`（GitHub Trending 榜单卡片）是后加的，只做过本机验证：
> `cargo build --release` + `scripts/check_abi.py`（ABI 符号 7/7）+
> `scripts/package.py --platform windows-x86_64`，并在宿主 `fuyubot 0.1.6` +
> `fuyubot.html2img 0.1.2` 的离线回放里真实出图（今日 / 本周 / 本月三档、1600px 与
> 1200px 两档宽度）。它还没进上面那批多平台验证，也没跑过真机。

> `bidibot` v0.1.0（论坛群话题式中转）同样只做过本机验证：`cargo build --release` +
> `cargo test`（114 项）+ `cargo clippy -D warnings` + `scripts/check_abi.py`（ABI 符号 7/7）+
> `scripts/package.py --platform windows-x86_64`，并在宿主 0.1.6 的离线回放里验过
> 建话题 / 话题内转发 / 话题内回复回到用户 / 话题内拉黑；用户名片在装了 `fuyubot.html2img` 时
> 会渲染成图片卡片（软依赖，没装自动降级文字），已用真实 html2img 0.1.2 出图验证。
> 未进多平台矩阵，也没在真账号上跑过完整收发。
> 口径见 [`docs/bidibot-topics-design.md`](docs/bidibot-topics-design.md)，配置与命令见
> [`crates/fuyubot-plugin-bidibot/README.md`](crates/fuyubot-plugin-bidibot/README.md)。

> `rusto` v0.1.0（纯 Rust OCR，图片进文本出）同样只做过本机验证：`cargo build --release` +
> `cargo test`（40 项，含图片消息回放夹具）+ `cargo clippy -D warnings` +
> `scripts/check_abi.py`（ABI 符号 7/7）+ `scripts/package.py --platform windows-x86_64`
> （离线包 4.2 MiB，crate 根的 `models/` 与 `NOTICE.md` 由打包脚本自动带入）。
> 未进多平台矩阵，也没在真机上跑过完整收发；模型获取与在线 / 离线两种包见
> [`crates/fuyubot-plugin-rusto/BUNDLE.md`](crates/fuyubot-plugin-rusto/BUNDLE.md)。

验证记录：<https://github.com/fengyegf/fy-plugins/actions/runs/42824916393>。
上面的验证是 ABI v5 时期跑的；迁到 ABI v6 后，只有构建/打包结论仍然成立，宿主加载
需要新版（ABI v6）的 `fuyubot-sdk` / FuyuBot 才能验证——本机附带的
`fuyubot-sdk.exe` 还是 ABI v5，不能再用来 check ABI v6 插件。

没有 ABI v6 的 SDK 也能自己核对导出符号：`python scripts/check_abi.py target/debug/*.dll`
静态读 PE 导出表，检查七个 ABI 符号（不 dlopen、不执行插件代码）。本次 ABI v6 审计与
`/help` 联动改造的结论见 [`AUDIT-ABI-V6.md`](AUDIT-ABI-V6.md)，统一口径见
[`docs/abi-v6-spec.md`](docs/abi-v6-spec.md)。

**能打包的**：`plugin.json` 里声明、并且已经构建出动态库的平台组合。

**不能打包的**：

- 没在 `plugin.json` 里声明的平台——例如 ppxfy 的 Android（它依赖 native-tls，
  Android 交叉编译链没打通，**故意不声明**），`package.py` 会直接报错；
- 声明了但没构建过的平台——`package.py` 找不到 `target/.../release/lib*.{so,dll,dylib}`
  就报错，不会产出空包；
- 脚本只认五个平台键：`windows-x86_64`、`linux-x86_64`、`macos-aarch64`、
  `android-aarch64`、`android-x86_64`（没有 windows-aarch64 / linux-aarch64 /
  macOS x86_64）。

发布前想重新核对桌面平台矩阵（Linux / macOS），手动跑 `verify-platforms` workflow
（Actions → verify-platforms → Run workflow），或推 `verify/platforms` 分支；
Android 的两端仍按发布流水线（tag）构建。

## 命令一览

插件命令都是**普通文本消息**：前缀可配（缺省 `/` `!` `！` `.` `#`），命令名大小写不敏感，
`/whois@FuyuBot` 里的 `@FuyuBot` 会被剥掉。每个插件启动时把自己登记进宿主的命令表
（`fuyubot.host.commands`），所以下表就是 `/help` 里看到的内容。除 `bidibot` 的群管理命令
登记为 `role = owner`、`scopes = group` 外，其余命令都不做权限判断，一律 `role = user`、
`scopes = private + group`（频道不登记，频道里没有「回命令」的会话上下文）。

| 插件 | 命令 | 别名 | 参数 | 说明 |
| --- | --- | --- | --- | --- |
| `whois` | `/whois` | — | `<域名>`，可给完整 URL | 查域名注册信息与服务器 IP 归属，出信息卡片；不带参数回用法提示 |
| `musiccard` | `/musiccard` | — | `<歌曲 ID / 网易云链接 / 关键词>` | 生成网易云音乐卡片；参数按空格拼回，关键词可带空格 |
| `epic` | `/epic` | `/epicfree` | — | Epic 当前免费 / 即将免费的游戏卡片 |
| `kfc` | `/kfc` | — | — | 随机一句肯德基疯话，渲染成推特卡片 |
| `ppxfy` | `/download` | `/d`、`/downloadcommand` | — | 交互式下载：回复链接（可多条，`OK` 结束）→ 解压 → 目录树确认 → 打包上传 |
| `ppxfy` | `/galgame` | `/p`、`/galgamecommand` | `<DMM / DLSite 作品链接>` | 解析作品页 → Telegraph 预览页 → 逐项核对（`BACK` / `CANCEL`）→ 记录入库 → 分享消息 |
| `ppxfy` | `/cancel` | — | — | 取消当前聊天的下载 / 核对任务，并清理临时文件 |
| `ppxfy` | `/clearcache` | — | — | 兜底清理 `cache/`（没有活跃任务时也能用） |
| `system` | `/system` | — | — | 宿主机系统信息与机器人状态卡片 |
| `trending` | `/trending` | — | `[today\|weekly\|monthly]`，中文（今日 / 本周 / 本月）也认，缺省今日 | GitHub Trending 4 列卡片墙 |
| `bidibot` | `/bidistart` | — | — | 欢迎语：双向私聊中转怎么用 |
| `bidibot` | `/bidihelp` | — | — | 帮助（私聊里也能用） |
| `bidibot` | `/bidistatus` | — | — | 主人：运行状态（群 / 管理员 / 广告 / 话题数） |
| `bidibot` | `/bidilist` | — | — | 主人：拉黑列表（前 20 条） |
| `bidibot` | `/bidiban` | — | `[id] [原因]` | 主人：拉黑；在该用户的话题里发可省略 id |
| `bidibot` | `/bidiunban` | — | `[id]` | 主人：解封；在话题里发可省略 id |
| `bidibot` | `/bidiwarn` | — | `[id]` | 主人：查看某用户的广告警告数 |
| `bidibot` | `/bidiclear` | — | `[id]` | 主人：清零某用户的广告警告 |
| `bidibot` | `/bidiword` | — | `add\|del\|list [词]` | 主人：维护广告关键词 |
| `bidibot` | `/bidisay` | — | `[id] <文字>` | 主人：以 bot 身份主动给用户发一条 |
| `bidibot` | `/bidicard` | — | — | 主人：在当前话题里重发这个用户的名片 |
| `rusto` | `/ocr` | `/rusto` | `[detail\|status\|warm\|path <本机路径>\|help]` | 识别图片文字；对图片消息回复或在图片说明里写命令。`detail` 附行级置信度与坐标（也认 `d` / `详细`），`status` / `warm` 看引擎 / 预加载，`path` 识别本机图片，`help` 用法 |

几点补充：

- 别名不单独占 `/help` 一行，只写进主命令的说明里；
- 未识别的命令**静默忽略**，所以多个插件可以共存，同一条消息只由匹配到的插件处理；
- `ppxfy` 的交互步骤（链接、密码、编号、包名）都是**回复提示消息**，等待上限是
  `listen_timeout_ms`（缺省 10 分钟），`/cancel` 随时可打断；
- 参数不认识时（例如 `/trending foo`）只回一条用法提示，不替用户猜。
- `bidibot` 的私聊中转不靠命令：用户直接发消息就进对应话题，主人在话题里说话 / 引用回复即回到用户；
  `/bidi*` 只是群里的管理入口（转发本身对用户透明）。
- `rusto` 的 `/ocr` 必须带上图片（回复图片或写在图片说明里），私聊直发图要 `auto = true` 才自动识别；
  不带图只回用法提示，不会替你猜。

## 配置一览

每个插件只读自己名字那一段 `[plugin.<id>]`，段内全部平铺；键名同时接受 `snake_case`
与大写写法，缺失或类型不对的键退回缺省值。键的含义、缺省值与夹取范围见各插件自己的
README（下表「插件」一列），本文只列「有哪些键」。

| 插件 | 配置段 | 键 |
| --- | --- | --- |
| [whois](crates/fuyubot-plugin-whois/README.md) | `[plugin.whois]` | `whois_api_url` `ip_api_url` `width` `timeout_ms` `favicon` `favicon_timeout_ms` `prefixes` |
| [musiccard](crates/fuyubot-plugin-musiccard/README.md) | `[plugin.musiccard]` | `request_timeout_ms` |
| [epic](crates/fuyubot-plugin-epic/README.md) | `[plugin.epic]` | `locale` `country` `timezone_offset_hours` `api_url` `user_agent` `proxy` `timeout_ms` `scale` `max_per_row` `max_cards` `card_width` `cover_height` `caption` |
| [kfc](crates/fuyubot-plugin-kfc/README.md) | `[plugin.kfc]` | `api_url` `width` `timeout_ms` `prefixes` |
| [ppxfy](crates/fuyubot-plugin-ppxfy/README.md) | `[plugin.ppxfy]` | `cache_dir` `listen_timeout_ms` `unzip_timeout_ms` `archive_size_threshold_mb` `download_concurrency` `split_threshold_mb` `seven_zip` `fix_encoding` `telegraph_token` `telegraph_author` `telegraph_author_url` `dlsite_proxy` `mongo_collection` `mongodb_db` |
| [system](crates/fuyubot-plugin-system/README.md) | `[plugin.system]` | `super_admin` `background_url` `timeout_ms` `avatar_timeout_ms` `prefixes` |
| [trending](crates/fuyubot-plugin-trending/README.md) | `[plugin.trending]` | `trending_url` `width` `timeout_ms` `max_items` `avatar` `avatar_timeout_ms` `prefixes` |
| [bidibot](crates/fuyubot-plugin-bidibot/README.md) | `[plugin.bidibot]` | `group_id` `owner_id` `topic_name_max` `topic_icon_color` `topic_card` `card_image` `card_width` `ai_base_url` `ai_api_key` `ai_model` `ai_timeout_ms` `ai_temperature` `ad_mode` `auto_ban_after` `warn_cooldown_secs` `ad_keywords` `ad_keywords_extra` `max_relay_chars` `send_timeout_ms` `rate_limit_per_min` `notify_blocked_every_secs` `store_path` `skills_dir` `command_table` `prefixes` |
| [rusto](crates/fuyubot-plugin-rusto/README.md) | `[plugin.rusto]` | `auto` `max_side` `concurrency` `rec_workers` `model` `model_dir` `mirror` `detail_default` `reply_placeholder` `chunk_size` `timeout_ms` |

两点约定：

- 命令前缀有两种取法，缺省都是 `["/", "!", "！", ".", "#"]`：`whois` / `kfc` / `system` /
  `trending` / `bidibot` 先读本插件段的 `prefixes`，没写再跟随宿主顶层 `[config].PREFIXES`；
  `musiccard` / `epic` / `ppxfy` 只跟随宿主顶层 `[config].PREFIXES`，本段没有这个键；
  `rusto` 的五个前缀写死在解析层，本段也没有这个键。
- `ppxfy` 的两个凭据（`telegraph_token` / `dlsite_proxy`）按「配置段 > 环境变量
  `PPXFY_TELEGRAPH_TOKEN` / `PPXFY_DLSITE_PROXY` > `.env`」取值，代码里不留缺省值；
  galgame 记录入库走 mongo-plugin 的 `storage.mongo`，没装时分享照常、只是不入库。

## 目录

```text
crates/
  fuyubot-plugin-api/        # vendored 宿主门面（ABI v6），不是插件
  fuyubot-plugin-<id>/       # 一个插件一个 crate
    Cargo.toml               # 版本 / 依赖 / [lib] name 都独立
    plugin.json              # 商城 bundle 元信息（id / entry / abi / platforms）
    src/ examples/ tests/ assets/
vendor/
  zip/                       # zip 9.1.3 的 Windows 文件名补丁
scripts/
  package.py                 # 打一个平台一个插件的 bundle ZIP
  plan.py                    # 由 plugin.json 计算发布矩阵（CI 用）
  check_abi.py               # 静态核对 dylib 的七个 ABI 符号（ABI v6）
docs/
  abi-v6-spec.md             # ABI v6 五条契约 + /help 联动的统一口径
.github/workflows/
  ci.yml                     # fmt + clippy
  release.yml                # <id>-v<version> tag → 多平台构建 → Release
```

## 环境

- Rust 1.99+（`rust-version` 在根 `Cargo.toml`）
- 插件 ABI：**v6**（宿主当前版本）
- 打包脚本：Python 3.9+（只用标准库）

## 构建与检查

```bash
# 单个插件
cargo build --release -p fuyubot-plugin-whois

# 全部插件的类型检查（会编译 Blitz / Vello，第一次比较慢）
cargo check --workspace --all-targets

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

产物在 `target/release/`：Windows 是 `<id>.dll`，Linux `<id>.so` 带 `lib` 前缀，
macOS 是 `lib<id>.dylib`。`[lib] name` 固定为插件 id，所以动态库文件名与
`plugin.json` 的 `entry` 始终对得上。

## 发布一个插件

版本号只属于单个插件：改该 crate 的 `Cargo.toml` 和 `plugin.json` 里的 `version`
（两者必须一致），提交后打 tag：

```bash
git tag whois-v0.1.2
git push origin whois-v0.1.2
```

`release` 流水线会：

1. 从 tag 解析出 `id` / `version`，校验与 `Cargo.toml`、`plugin.json` 一致；
2. 按 `plugin.json` 的 `platforms` 生成矩阵（Windows / Linux / macOS 原生构建，
   Android 走 NDK + `cargo-ndk`）；
3. 每个平台执行 `python scripts/package.py --plugin <id> --platform <p> --version <v>`；
4. 把 `<id>-<version>-<platform>.zip` 和 `SHA256SUMS.txt` 挂到 GitHub Release。

手动触发（Actions → release → Run workflow）只上传各平台 artifact，不建 Release，
适合先验证某个平台能不能编过。

bundle 契约（与商城一致）：

- ZIP 根就是插件目录内容：`plugin.json` 在根，不套一层；
- `plugin.json` 的 `version`、`platforms`、`entry` 由打包脚本按目标平台改写；
- 不收 `target/`、`dist/`、`.git/`，也没有符号链接。

## 收录到插件商城

商城仓的 `plugins/<id>.yml` 指向本仓库的 Release：

```yaml
repo: fengyegf/fy-plugins
homepage: https://github.com/fengyegf/fy-plugins
versions:
  - version: 0.1.1
    tag: whois-v0.1.1                    # 一个仓库多个插件，tag 必须带 id 前缀
    assets:
      windows-x86_64: whois-0.1.1-windows-x86_64.zip
      linux-x86_64: whois-0.1.1-linux-x86_64.zip
      macos-aarch64: whois-0.1.1-macos-aarch64.zip
      android-aarch64: whois-0.1.1-android-aarch64.zip
      android-x86_64: whois-0.1.1-android-x86_64.zip
```

资产 URL 由 `repo` + `tag` + 文件名拼出来，所以 tag 必须用 `<id>-v<version>`；
`assets` 里的文件名与 Release 上的保持一致即可。

## 新增一个插件

1. 复制 `crates/fuyubot-plugin-whois` 为 `crates/fuyubot-plugin-<id>`；
2. 改 `Cargo.toml` 的 `name`（`fuyubot-plugin-<id>`）、`version`、`description`，
   以及 `[lib] name = "<id>"`；
3. 改 `plugin.json` 的 `id`、`name`、`entry`、`platforms`，`version` 与 Cargo 一致；
4. 在 `src/lib.rs` 里 `define_plugin!(YourPlugin, "<id>")`；
5. `cargo check -p fuyubot-plugin-<id>`，然后在 `release.yml` 的 `workflow_dispatch`
   选项里加上新 id（tag 触发不需要改）。

## 为什么 vendored `fuyubot-plugin-api`

`crates/fuyubot-plugin-api` 是上游 0.1.2 门面源码的本地副本：

- crates.io 上 `fuyubot-plugin-api` 的 0.1.x 已全部 yank，新仓库没法重新解析；
- 上游 0.1.3 已弃用，只剩一行弃用说明，但门面里的 `HostExt` / `Subscription` 还在用；
- 底层 ABI 依赖是 `fuyubot-abi = "=0.1.3"`，即 **ABI v6**，与当前 FuyuBot 宿主一致。

所以门面源码随仓维护，ABI 版本由 `fuyubot-abi` 决定。上游若恢复门面发布，删掉这个
crate、改回 registry 依赖即可，插件源码不用动。

## 依赖与锁文件

- `Cargo.lock` 必须提交：插件是动态库，构建结果要可复现；
- 不要随手 `cargo update`，尤其是 `image`、`fuyubot-abi`；
- 第三方补丁只有 `vendor/zip`（见根 `Cargo.toml` 的 `[patch.crates-io]`）。

## 协议

各 crate 的协议以自身 `Cargo.toml` 为准；`fuyubot-plugin-api` 沿用上游
MIT OR Apache-2.0。
