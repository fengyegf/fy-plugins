# GitHub Trending 插件

基于 FuyuBot SDK Native ABI v6 的 GitHub Trending 榜单插件。用户发送命令后，插件抓取
`https://github.com/trending?since=…` 并在业务线程里解析榜单，把数据交给
`assets/card.html` 里的 Vue 卡片模板，再由宿主上的 `fuyubot.html2img` 服务渲染成
JPEG 卡片墙，并以照片回复触发消息。

整个运行链路使用 Rust 实现，不依赖 Node、浏览器或外部截图服务；渲染器只在宿主一侧
（html2img 插件）存在，本插件不链接任何渲染引擎。

## 功能

- 三个榜单周期：今日（`daily`）、本周（`weekly`）、本月（`monthly`），缺省今日。
- 参数认英文全称 / 缩写与中文，大小写和首尾空白都不敏感（见下方参数表）。
- 卡片墙固定 4 列、行数随条数往下排；浅色标题居中，周期用白底黑字徽标表示。
- 每张卡片：**作者头像**（实时抓 GitHub 头像，抓不到用首字母方块兜底）、仓库名 +
  owner、两行简介、底行 ★累计 star / +本周期新增 / ⑂fork，右侧是主语言色点与语言名。
- 页脚注明数据来源与周期（`数据来源 github.com/trending · TODAY 榜单`）。
- 参数不认识时回用法提示，不猜测用户意图。
- 抓取失败、渲染失败、发送失败各有中文提示，技术原因只写日志。
- 抓取、渲染和上传都在独立业务线程里执行（均阻塞），不阻塞 TDLib 回调线程。
- `fuyubot.html2img` 产出的临时图片发送完成后立即删除。

## 效果

下面三张都是**真实的宿主** `fuyubot 0.1.6` + `fuyubot.html2img 0.1.2` 出的图：抓的是
线上 `github.com/trending`，头像是真实抓下来并缩到 40px 的（14–15 张，约 40 KiB PNG），
1600px 宽、单张 0.2–5 秒（含抓页面与头像）：

| 今日（14 条 → 4 行） | 本周 | 本月 |
| --- | --- | --- |
| ![今日榜卡片](../../docs/trending-card-live.jpg) | ![本周榜卡片](../../docs/trending-card-weekly.jpg) | ![本月榜卡片](../../docs/trending-card-monthly.jpg) |

## 使用

```text
/trending              # 今日榜（缺省）
/trending today        # 今日榜
/trending weekly       # 本周榜
/trending 本月         # 本月榜
```

参数不区分大小写，也认缩写与数字：

| 周期 | 可接受的参数 |
| --- | --- |
| 今日 | `today` / `daily` / `day` / `d` / `1` / `今日` / `今天` / `日` / `日榜` |
| 本周 | `week` / `weekly` / `w` / `7` / `本周` / `这周` / `周` / `周榜` |
| 本月 | `month` / `monthly` / `m` / `30` / `本月` / `这个月` / `月` / `月榜` |

其它参数（例如 `/trending 去年`）返回用法提示。命令前缀按以下顺序读取：

1. `[plugin.trending].prefixes`
2. 宿主顶层 `[config].PREFIXES`
3. 缺省前缀 `/`、`!`、`！`、`.`、`#`

### 命令表与 `/help`

命令表的主人是**宿主**：宿主把命令表暴露成进程级共享服务 `fuyubot.host.commands`，
插件在 `start()` 里用它的 `register` 方法登记 `/trending`。内置 `/help` 只是这张表
的读取方（`fuyubot.host.commands.list` 之后按 `role` / `scopes` / `hidden` 过滤、
按插件分组渲染）。宿主服务启动时就在，因此**不需要** `soft_inject` 排队。登记失败
只记一条 warn，不影响 `/trending` 本身。

## 处理流程

1. 解析命令并取第一个参数：缺省 `Period::Today`，认不出就回用法提示。
2. `fetch::fetch_trending(&config, period)` 抓 `{trending_url}?since=…` 并解析成条目；
   失败回「榜单抓取失败」并记日志。
3. 条目按 `max_items` 截断（上限 `model::MAX_ITEMS = 15`，4 列正好排满 4 行）。
4. `CardData::build(period, items, 抓取时间)` 汇总总数、最大增量和语言分布；抓取时间
   是`YYYY-MM-DD HH:MM`，用 `SystemTime` 的 UTC 秒数加固定 8 小时得到北京时间。
5. `render::render_card(&host, &card, width)` 调 `fuyubot.html2img` 出图。
6. 通过宿主的 `fuyubot.host.command` 服务 `reply` 回复照片（发新消息并挂 `reply_to`、
   编辑原命令消息还是群里的 ephemeral，都由宿主决定），随后删除服务产出的临时文件。
7. 每个阶段边界检查停机标志，插件 `stop()` 时不必等满一次抓取 + 渲染 + 上传。

## 渲染链路

界面不在 Rust 里拼：整张卡片（HTML / CSS / 循环 / 条件 / 插值）都在
`assets/card.html`（`<style>` + Vue 模板）。插件只把 `CardData` 映射成模板的
`data`，交给宿主上的
[`fuyubot.html2img`](https://github.com/xiaoqvan/fuyubot-plugins/tree/main/crates/fuyubot-plugin-html2img)
插件的 `render` 方法（`vue: true`），拿回本机图片路径后以照片发送，发完即删。

依赖用 `inject` 声明为**硬依赖**（写的是服务名 `fuyubot.html2img`，不是插件名）：
没装 html2img 时本插件停在 Pending，比启动后每条命令都渲染失败更容易排查。

交给服务的入参：

| 字段 | 值 | 说明 |
| --- | --- | --- |
| `html` | `assets/card.html` | `<style>` + Vue 模板，编译期嵌入 |
| `vue` | `true` | 按 Vue 模板编译，用 `data` 渲染 |
| `data` | `render::view_data()` 的结果 | 榜单条目 + 千分位 / 短格式 / 截断 / 头像 / 首字母等派生值 |
| `width` | 配置的卡片宽度，夹到 `1200`–`1920` | 每张卡片再按 4 列与 32px 内边距四等分 |
| `height` | `240` | `auto_height` 时只是下限，避免短卡片被默认视口撑出留白 |
| `auto_height` | `true` | 高度按内容撑开 |
| `scale` | `1.0` | 1 CSS px = 1 输出 px |
| `format` | `jpg` | 深色底卡片，JPEG 体积小、Telegram 上传够用 |
| `dark` | `true` | 深色底，同时影响 `prefers-color-scheme` |

模板表达式受 vue-template-html 限制（没有 JS 运行时）：不能调用函数，也不能用
`Math` / `Date` / `?.` / `??`。千分位（`thousands`）、短数字（`compact`）、描述截断
（`truncate`）、头像兜底的首字母（`initial`）这些结果都先在 Rust 里算好，模板只读
字段；HTML 转义由渲染器负责，Rust 不再自己拼 HTML，也不再自己转义。

服务调用是**同步阻塞**的，只在业务线程里发生；每次渲染的 HTML 必须控制在 128 KiB
以内（模板编译期嵌入，超限直接报错，不会发出坏请求）。

## 回复方式

回复命令消息走宿主的 `fuyubot.host.command` 服务的 `reply` 方法，入参是
`{"chat_id":…,"message_id":…,"timeout_ms":…,"payload":{"input_message_content":…}}`，
成功返回 TDLib 最终的 `Message`，失败回 `{"@type":"error",…}`。发新消息并挂
`reply_to`、编辑原命令消息、还是群里的 ephemeral 消息，都由宿主决定——插件不再自己拼
`chat_id` / `reply_to`。主动推送（没有命令消息可回，例如任务 panic 时的内部错误提示）
继续用 simplify 的 `send_message`。

图片上传走 TDLib 的「上传本地照片」分支：`inputPhoto` 里塞 `inputFileLocal`，
宽高交给服务器替换。上传超时上限 120 秒，普通文本 30 秒。

## 项目结构

| 路径 | 说明 |
| --- | --- |
| `src/lib.rs` | 插件入口、订阅生命周期和启停管理 |
| `src/host/` | 宿主通信：回调投递、收件与调度线程、命令回复通道与消息网关 |
| `src/cmd/` | 命令解析、任务分发与 `/trending` 业务 |
| `src/core/` | 配置、榜单页抓取、HTML 解析、作者头像抓取和数据模型 |
| `src/render/` | Vue 卡片模板嵌入与 `fuyubot.html2img` 服务客户端 |
| `assets/card.html` | 卡片界面：`<style>` + 4 列卡片墙 Vue 模板 |
| `plugin.json` | 插件元信息与 ABI 声明 |

模块职责保持单向：`host` 负责宿主通信，`cmd` 负责任务编排，`core` 负责数据与网络，
`render` 负责版面与图像输出。

线程模型由三条硬约束决定：`updateNewMessage` 回调跑在 TDLib 接收线程上不能阻塞；
发消息 / 上传要等响应，而响应同样由接收线程投递；插件卸载前必须 join 自己起的线程。
因此回调只把更新丢进 channel，一个常驻 worker 线程负责解析与分发，每条命令再开一个
短命线程跑完整流程。

## 配置

在 `fuyubot.toml` 中增加 `[plugin.trending]` 段。所有字段均可选：

```toml
[plugin.trending]
trending_url = "https://github.com/trending"
width        = 1600
timeout_ms   = 15000
max_items    = 15
prefixes     = ["/", "!", "！", ".", "#"]
```

| 字段 | 缺省值 | 说明 |
| --- | --- | --- |
| `trending_url` | `https://github.com/trending` | 榜单页地址（不带查询串，`?since=…` 由抓取层拼） |
| `width` | `1600` | 卡片宽度，限制在 `1200` 至 `1920` 像素（4 列每张约 272–412px） |
| `timeout_ms` | `15000` | 单次抓取（连接 + 读取）超时，单位毫秒 |
| `max_items` | `15` | 最多渲染几条，限制在 1 至 30；实际还受版面 `MAX_ITEMS = 15` 约束 |
| `avatar` | `true` | 是否抓仓库作者头像；关掉后卡片用首字母方块，出图更快 |
| `avatar_timeout_ms` | `6000` | 头像抓取的整体预算，夹在 500–20000；到点收工，已抓到的照用 |
| `prefixes` | 跟随宿主配置 | 插件识别的命令前缀 |

## 构建

在仓库根目录执行：

```bash
cargo build --release --locked -p fuyubot-plugin-trending
```

产物位于 `target/release/trending.dll`；Linux / macOS 分别是 `libtrending.so` /
`libtrending.dylib`。`[lib] name` 固定为 `trending`，所以动态库文件名与
`plugin.json` 的 `entry` 始终对得上。

静态检查（不 dlopen，只读 PE 导出表核对 ABI v6 的七个符号；ABI v6 的
`fuyubot-sdk check` 不可用时也能用）：

```bash
python scripts/check_abi.py target/release/trending.dll
python scripts/package.py --plugin trending --platform windows-x86_64
```

## 离线回放验证

不接账号也能端到端验证：把本插件与 `fuyubot.html2img` 装进临时插件目录，用
`docs/replay/new_message.json` 回放一条 `/trending`，宿主会在本地抓页面、渲染并记录
发送请求（请求只留档，不真的发出）。想让回放不依赖外网，先把
`https://github.com/trending?since=daily` 存一份页面快照，再把
`[plugin.trending].trending_url` 指向本地 HTTP 服务上的这份快照。

## 运行行为与限制

- 卡片宽度低于 `1200px` 或高于 `1920px` 时由插件自动夹取；窄于 `1200px` 4 列会挤成三行简介。
- 头像走 `https://github.com/{owner}.png?size=80`，6 个线程并发抓、缩到 40px PNG 再
  以 data URI 内联；整批受 `avatar_timeout_ms` 与 48 KiB 预算约束（避免撑爆 html2img
  的 128 KiB HTML 上限），抓不到或超预算的条目回退成首字母方块。
- 渲染依赖宿主上的 `fuyubot.html2img`（硬依赖）：没装时本插件停留在 Pending；
  服务不可用、HTML 超限或渲染报错时，命令回复失败提示而不是崩掉。
- 榜单页由 GitHub 提供且没有公开 JSON 接口，只能抓 HTML；GitHub 改版、限流或不可达时
  命令返回抓取失败提示。
- 抓取时间显示为北京时间，按固定 UTC+8 偏移计算：中国不实行夏令时，不做时区换算。
- 系统缺少合适的中文字体时，卡片中的 CJK 字符可能使用系统回退字体。
- 插件停止时会等待当前阻塞操作到达安全边界；图片上传网关的超时上限为 120 秒。
