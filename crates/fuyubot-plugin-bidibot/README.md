# Bidibot 双向私聊中转

`bidibot` · v0.1.0 · ABI v6 · Windows / Linux / macOS / Android

## 项目介绍

Bidibot 把陌生人私聊收进一个论坛群：每个用户一个话题（话题名 = 用户昵称），用户消息**真转发**进话题，主人在话题里回复后以 bot 自己的身份发回用户，用户看不到主人身份 / 账号。带广告判定与三次熔断、限流、单向拉黑，状态落在本地库。

- 用户 → 群：私聊 → 命令 / 封禁 / 广告 / 限流检查 → 确保话题（首次用昵称建 `createForumTopic`）→ 原消息转发进话题。
- 群 → 用户：主人在话题里回复 → 文本 `sendMessage` 重发、媒体 `forwardMessages(send_copy)` 复制内容 → 不带转发头、不暴露主人。
- 名片：头像 / 名称 / @用户名 · ID 交给宿主 `fuyubot.html2img` 渲染成图片；没有渲染器时降级为头像 + 文字，再降为纯文字。

## 目录结构

```text
crates/fuyubot-plugin-bidibot/
  Cargo.toml
  plugin.json
  config.default.toml
  README.md
  assets/card.html      名片 Vue 模板（编译进动态库）
  skills/ad.md          广告判定提示词（缺失 / 为空时用内置副本）
  src/
    lib.rs              入口、生命周期、html2img 软依赖探测
    help.rs             向宿主命令表登记 /bidi* 命令
    relay.rs            双向中转主链路（私聊 ⇄ 群话题）
    cmd/                命令层：分派 mod / 管理命令 owner / 前缀解析 parser
    core/               配置、本地库、广告判定、LLM、skills
    host/               消息网关与业务线程
    render/             名片数据与 fuyubot.html2img 客户端
```

部署后（ZIP 根 = 插件目录内容）：

```text
plugins/bidibot/
  plugin.json
  bin/bidibot.dll        # Linux: libbidibot.so / macOS: libbidibot.dylib
  config.default.toml
  skills/ad.md           # 可选：覆盖内置广告提示词
```

运行期数据写在宿主工作目录的 `data/bidibot/db.json`（首次运行自动创建）；话题映射、封禁、警告、用户档案都在这里。

## 命令一览

| 命令 | 权限 / 范围 | 作用 |
| --- | --- | --- |
| `/bidistart` | 所有人 · 私聊 + 群 | 欢迎语与用法提示 |
| `/bidihelp` | 所有人 · 私聊 + 群 | 帮助；管理员看到多出的管理命令列表 |
| `/bidistatus` | 主人 · 群 | 运行状态：群、话题数、库大小、计数 |
| `/bidilist` | 主人 · 群 | 拉黑列表（前 20 条） |
| `/bidiban [id] [原因]` | 主人 · 群 | 拉黑；在该用户的话题里不带 id 即拉黑话题主人 |
| `/bidiunban [id]` | 主人 · 群 | 解除拉黑 |
| `/bidiwarn [id]` | 主人 · 群 | 查看某用户的广告警告数 |
| `/bidiclear [id]` | 主人 · 群 | 清零警告（不解除拉黑） |
| `/bidiword add\|del\|list [词]` | 主人 · 群 | 维护广告关键词 |
| `/bidisay [id] <文字>` | 主人 · 群 | 以 bot 身份主动给某用户发一条 |
| `/bidicard` | 主人 · 群 | 在当前话题里重发该用户的名片 |

命令前缀来自 `[plugin.bidibot].prefixes`（默认 `/`、`!`、`！`、`.`、`#`）；宿主顶层 `[config].PREFIXES` 仅在插件未给前缀时兜底。管理命令登记为 `role = "owner"`，宿主的 `[roles].owners` 必须包含 `owner_id`，否则会被命令闸门拦下。

## 配置一览

配置段 `[plugin.bidibot]`，键名同时接受 snake_case 与全大写写法。

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `group_id` | `0` | 论坛超级群 id（负数）；`0` = 未配置，私聊只回「暂时不可用」 |
| `owner_id` | `0` | 允许下管理命令 / 回复用户的主人 user id；`0` = 管理命令全部不可用 |
| `ad_mode` | `keyword` | 广告判定：`off` / `keyword` / `llm`；配 `llm` 但没有 AI 凭据时自动退回 `keyword` |
| `ai_base_url` | `https://api.openai.com/v1` | OpenAI 兼容 API 地址（仅 `ad_mode = "llm"` 使用） |
| `ai_api_key` | 空 | 大模型 API key；留空 = 不用大模型 |
| `ai_model` | `gpt-4o-mini` | 模型名 |
| `ai_timeout_ms` | `20000` | 单次大模型判定超时（毫秒，最小 1000） |
| `ai_temperature` | `0.2` | 采样温度（0–2） |
| `auto_ban_after` | `3` | 广告警告累计到多少次自动熔断拉黑（最少 1） |
| `warn_cooldown_secs` | `30` | 同一用户两次警告的最小间隔（秒） |
| `topic_name_max` | `28` | 话题名截断长度（Telegram 上限 128） |
| `topic_icon_color` | `7322096` | 话题图标颜色（`0x6FB9F0`），只认 6 个允许值，非法值退回默认 |
| `topic_card` | `true` | 新建话题时是否发送用户名片 |
| `card_image` | `true` | 名片优先用 `fuyubot.html2img` 渲染成图片（无渲染器自动降级） |
| `card_width` | `460` | 名片图 CSS 宽度（px，2 倍图输出，范围 320–1200） |
| `store_path` | `data/bidibot/db.json` | 本地库路径（相对宿主工作目录，原子落盘） |
| `skills_dir` | `skills/bidibot` | 广告提示词目录；文件缺失 / 为空时用内置 `ad.md` |
| `max_relay_chars` | `3500` | 单条文本中转的最大字符数（200–4096，超出截断） |
| `send_timeout_ms` | `30000` | 单次发送等待超时（毫秒，1000–120000） |
| `rate_limit_per_min` | `12` | 单用户每分钟最多中转条数；`0` = 不限制 |
| `notify_blocked_every_secs` | `3600` | 被封禁用户最短多少秒才重复提示一次 |
| `command_table` | `true` | 是否把 `/bidi*` 登记进宿主命令表（`/help` 数据源） |
| `ad_keywords` | `[]` | 广告关键词表；非空 = 整体替换内置词表 |
| `ad_keywords_extra` | `[]` | 追加到（内置或替换后的）词表 |
| `prefixes` | `["/", "!", "！", ".", "#"]` | 命令前缀列表 |

## 使用说明

**安装**：从插件商店安装；或 `fuyubot plugin install ./bidibot`；或把发布包解压到 `plugins/bidibot/` 后重启宿主。

**准备群**：建一个开启话题的超级群（Forum），把 bot 拉进去并给「管理话题」权限；群 id 填 `group_id`（负数），主人 user id 填 `owner_id`。

最小配置：

```toml
[plugin.bidibot]
group_id = -1001234567890    # 论坛群 id，0 = 未配置
owner_id = 123456789         # 主人 user id，0 = 管理命令不可用
ad_mode = "keyword"          # off | keyword | llm
card_image = true            # 有 fuyubot.html2img 时渲染名片图

[roles]
owners = [123456789]         # 必须包含 owner_id，否则宿主会拦下 /bidi*
```

**典型用法**：

1. 用户直接私聊 bot，消息原样转进其专属话题；发命令前缀开头的消息不会被转发。
2. 主人在该话题里直接回复（文本或媒体）即回到用户；General 话题里没有对应关系，用 `/bidisay <id> <文字>`。
3. `/bidilist` 查看黑名单，`/bidiban` 拉黑、`/bidiunban` 解封，`/bidiword` 维护广告词表。

**构建与打包**：

```bash
cargo build --release -p fuyubot-plugin-bidibot
python scripts/package.py --plugin bidibot --platform windows-x86_64
```

**发布**：`git tag bidibot-v0.1.0 && git push origin bidibot-v0.1.0`。

## 项目依赖（软/硬）

| 类型 | 名称 | 说明 |
| --- | --- | --- |
| 硬依赖 | 无 | 不装任何插件即可运行；`fuyubot.host.commands`、`fuyubot.host.cache` 属宿主内建服务，不算插件依赖 |
| 软依赖 | `fuyubot.html2img` | 名片图片卡片；未安装时自动降级为头像 + 文字或纯文字，不影响中转 |
| 外部 API | OpenAI 兼容接口 | 仅 `ad_mode = "llm"` 时需要，插件直接 HTTP 调用，不占宿主服务 |
| 宿主 | ABI v6 | 低于 v6 的宿主无法加载本插件；群必须是已开启话题的超级群 |
