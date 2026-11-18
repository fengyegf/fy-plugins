# GitHub Trending

插件 id：`fengyegf.trending` ｜ 版本：0.1.1 ｜ ABI：v6 ｜ 平台：windows-x86_64 / linux-x86_64 / macos-aarch64 / android-aarch64 / android-x86_64

## 项目介绍

`/trending [today|weekly|monthly]` 抓取 `https://github.com/trending?since=…` 的 HTML，解析出仓库条目与作者头像，映射成 4 列卡片墙的 Vue 模板数据，再交给宿主上的 `fuyubot.html2img` 服务渲染成 JPEG，以照片回复命令消息，发送后删除临时产物。

抓取（`ureq` 阻塞 HTTP）、解析、头像处理都在插件自己的业务线程里完成，插件不链接任何渲染引擎；模板在 `assets/card.html`，Rust 只提供 `data`。抓取失败、参数不认识、渲染失败各有中文提示，技术原因只写日志。

![预览](preview.jpg)

## 目录结构

源码 crate：

```text
crates/fuyubot-plugin-trending/
├─ plugin.json                 # native / ABI 6 / entry = bin/trending.dll
├─ Cargo.toml                  # cdylib（宿主加载）+ rlib（测试 / examples）
├─ config.default.toml         # 最低优先级默认配置（随包发布）
├─ README.md
├─ preview.jpg                 # README 预览图，不打进 bundle
├─ assets/card.html            # 4 列卡片墙：<style> + Vue 模板，编译期嵌入
└─ src/
   ├─ lib.rs                   # Plugin 实现、订阅 updateNewMessage、启停
   ├─ help.rs                  # 把 /trending 登记进宿主命令表
   ├─ host/                    # mod.rs、worker.rs、messenger.rs、command.rs
   ├─ cmd/                     # parser.rs（前缀 / @机器人）、trending.rs（业务）
   ├─ core/                    # config.rs、fetch.rs、parse.rs、model.rs、avatar.rs
   └─ render/                  # mod.rs、html2img.rs
```

部署后的 bundle（zip 根就是插件目录内容）：

```text
plugins/fengyegf.trending/
├─ plugin.json
├─ config.default.toml
└─ bin/trending.dll            # Linux: libtrending.so；macOS: libtrending.dylib
```

## 命令一览

| 命令 | 权限 / 范围 | 作用 |
| --- | --- | --- |
| `/trending [周期]` | 无权限判断（登记 `role=user`），私聊 / 群聊 | 抓指定周期榜单，出 4 列卡片墙并以照片回复 |

`/trending@机器人名` 这类群聊写法由解析器剥掉 `@` 后缀；参数不认识时回用法提示，不猜用户意图。命令前缀来自 `[plugin.fengyegf.trending].prefixes`，未配置时跟随宿主 `[config].PREFIXES`。

## 配置一览

配置段：`[plugin.fengyegf.trending]`，字段全部可选。

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `trending_url` | `https://github.com/trending` | 榜单页地址（不带查询串，`?since=…` 由抓取层拼） |
| `width` | `1600` | 卡片宽度（px），夹到 `1200`–`1920`；1600 正好排 4 列 |
| `timeout_ms` | `15000` | 单次抓取（连接 + 读取）超时（毫秒） |
| `max_items` | `15` | 最多渲染几条，夹到 `1`–`30`；版面硬上限也是 15 |
| `avatar` | `true` | 是否抓作者头像；关掉用首字母方块兜底，出图更快 |
| `avatar_timeout_ms` | `6000` | 头像抓取整体预算（毫秒），夹到 `500`–`20000` |
| `prefixes` | `["/", "!", "！", ".", "#"]` | 命令前缀；不写则跟随宿主 `[config].PREFIXES` |

## 使用说明

**安装**

- 插件商店直接安装；或
- 源码目录：`fuyubot plugin install ./trending`；或
- 手动解压 zip 到宿主的 `plugins/fengyegf.trending/` 后重启宿主。

**最小配置**（`fuyubot.toml`）：

```toml
[plugin.fengyegf.trending]
width = 1600
max_items = 15
avatar = true
timeout_ms = 15000
avatar_timeout_ms = 6000
```

**典型用法**

```text
/trending              # 今日榜（缺省）
/trending weekly       # 本周榜
/trending 本月         # 本月榜
```

周期参数不区分大小写、忽略首尾空白，中英文都认：

| 周期 | 可用参数 |
| --- | --- |
| 今日 | `today` / `daily` / `day` / `d` / `1` / `今日` / `今天` / `日` / `日榜` |
| 本周 | `week` / `weekly` / `w` / `7` / `本周` / `这周` / `周` / `周榜` |
| 本月 | `month` / `monthly` / `m` / `30` / `本月` / `这个月` / `月` / `月榜` |

**构建与打包**

```bash
cargo build --release -p fuyubot-plugin-trending
python scripts/package.py --plugin trending --platform windows-x86_64
```

产出 `target/release/trending.dll` 与 `fengyegf.trending-0.1.1-windows-x86_64.zip`（根为 `plugin.json`、`config.default.toml`、`bin/trending.dll`）。

**发布**

```bash
git tag fengyegf.trending-v0.1.1
git push origin fengyegf.trending-v0.1.1
```

tag 名固定为 `<id>-v<version>`；同一版本已打过 tag 时，先递增 `plugin.json` 的版本号再打新 tag。

## 项目依赖（软/硬）

| 类型 | 服务名 / 条件 | 说明 |
| --- | --- | --- |
| 硬依赖 | `fuyubot.html2img` | 调 `render` 出图；经 `Plugin::inject` 声明，未安装时插件停在 Pending |
| 软依赖 | `fuyubot.host.commands` | 登记 `/trending` 供 `/help` 展示；登记失败只记 warn |
| 宿主内建（不算插件依赖） | `fuyubot.host.command` | 命令回复通道，发新消息 / 编辑原消息 / ephemeral 由宿主决定 |
| 外部条件 | 可出网访问 `github.com` | 抓榜单页与作者头像；限流、改版或断网时命令回「榜单抓取失败」 |

宿主要求：支持 FuyuBot Native **ABI v6**。平台：windows-x86_64 / linux-x86_64 / macos-aarch64 / android-aarch64 / android-x86_64。
