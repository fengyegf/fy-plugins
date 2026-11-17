# bidibot（论坛群话题式中转，v0.2）

陌生人私聊 bot → bot 把消息**转发**进你指定的论坛群，**一个用户一个话题**（话题名 = 用户昵称）；
你在话题里回复，bot 用自己身份把内容发回给那个用户。

- 插件 id：`bidibot`（配置段 `[plugin.bidibot]`，ABI v6）
- 本地库：`data/bidibot/db.json`（话题映射 / 封禁 / 警告 / 用户档案）
- 提示词：内置 `skills/ad.md`、`skills/translate.md`，可用 `skills_dir` 覆盖

## 它保证什么

| 承诺 | 怎么做到的 |
| --- | --- |
| 一个用户一个话题 | 首次来消息 `createForumTopic`（名字 = 昵称），thread id 落库；之后都转发进这个话题 |
| 主人身份不外泄 | 群 → 用户方向只有 `sendMessage`（文本）或 `forwardMessages(send_copy=true)`（媒体，复制内容不带"转发自"头部）；用户看不到主人的账号 / 用户名 |
| 用户消息保真 | 用户 → 群方向是**真转发**，图片 / 文件 / 语音都原样进话题，看得出是哪个用户 |
| 回调不阻塞 | `updateNewMessage` 回调只投递 channel，建话题 / 转发 / 广告判定都在 `bidibot-task` 线程 |
| 拉黑只影响私聊中转 | 被封禁 / 熔断的用户仍能用 `/bidihelp`；群里管理照常 |
| 状态可恢复 | 话题映射、封禁、警告每次变更都原子落盘，重启不丢 |

## 怎么搭

1. 建一个**开启话题**的超级群（Forum），把 bot 拉进去；
2. 给 bot 管理员权限，至少勾上**管理话题**（`can_manage_topics`）——不然建不了话题；
3. 群 id 填进配置（负数，例如 `-1001234567890`）：拿 id 最省事的办法是把群消息转发给
   `@userinfobot` 之类，或者看 `fuyubot` 日志里 `updateNewMessage` 的 `chat_id`。

## 配置

```toml
[plugin.bidibot]
# —— 必填 ——
group_id        = -1001234567890   # 论坛群 id（0 = 未配置，插件只回"暂时不可用"）
owner_id        = 123456789        # 允许在群里下命令 / 回复用户的人（0 = 不校验，谁都能回）
# —— 话题 ——
topic_name_max  = 28               # 话题名截断长度（Telegram 上限 128）
topic_icon_color = 7322096         # 只认 6 个值：0x6FB9F0/0xFFD67E/0xCB86DB/0x8EEE98/0xFF93B2/0xFB6F5F
topic_card      = true             # 新话题里发一条名片（ID / 用户名）
card_image      = true             # 有 fuyubot.html2img 时渲染图片卡片（没有自动降级）
card_width      = 460              # 名片图 CSS 宽度（输出 2 倍图；460 = 紧凑卡片）
# —— 大模型（只用于广告判定：ad_mode = "llm" 时需要）——
ai_base_url     = "https://api.openai.com/v1"
ai_api_key      = ""
ai_model        = "gpt-4o-mini"
ai_timeout_ms   = 20000
ai_temperature  = 0.2
# —— 广告与熔断 ——
ad_mode         = "keyword"        # off | keyword | llm
auto_ban_after  = 3
warn_cooldown_secs = 30
ad_keywords     = []               # 非空 = 替换内置词表
ad_keywords_extra = []             # 追加
# —— 行为 ——
max_relay_chars = 3500
send_timeout_ms = 30000
rate_limit_per_min = 12
notify_blocked_every_secs = 3600
# —— 存储 ——
store_path      = "data/bidibot/db.json"
skills_dir      = "skills/bidibot"
command_table   = true
prefixes        = ["/", "!", "！", ".", "#"]
```

### 跟宿主配置的配合（重要）

主人命令登记为 `role = "owner"`，宿主的命令闸门按 `[roles].owners` 判定：

```toml
[roles]
owners = [123456789]     # 与 [plugin.bidibot].owner_id 一致
```

不填这一项时宿主会把 `/bidi*` 当成无权命令拦掉（日志 `命令被策略拦下：只发 update 通用通道`），
插件收不到。用户侧中转不受影响，只有群里的管理命令会失灵。

## 名片（话题里的第一条消息）

新话题建好后，插件会发一张**用户名片**，分三档降级：

1. 装了 `fuyubot.html2img`（**软依赖**，不用 inject 声明）→ 渲染成图片卡片（**PNG 透明底，图里只有卡片本身，没有页面背景**），只展示三行：
   **头像 / 名称 / @用户名 · ID**；
2. 有头像但没渲染器 → 发头像照片，文字信息当 caption；
3. 都没有 → 纯文字版（没有"新对话"这种标题）。

名片随时可以重发：在某个用户的话题里发 `/bidicard`。

- 头像来自 TDLib `profile_photo.small`，下载后转 data URI 内嵌进卡片（不引外部资源）；
- 模板在 `assets/card.html`（Vue + 内联 CSS，编译进 dll）；
- `card_image = false` 可以强制不用图片卡片；`card_width` 调卡片宽度（默认 720，2 倍图输出）。

## 命令不中转

- **私聊**：任何以命令前缀开头的消息都不转发进群（不只是本插件认得的命令）。
  也就是 `/start`、`/help`、别的插件的命令、打错的 `/xxx` 都不会出现在话题里。
- **群里**：没被命令层认领的命令（其他人的命令 / 打错的命令）也不会转给用户。

## 翻译

插件**不做翻译**：Telegram 客户端自带整段翻译，用户/主人各自点一下就行。


## 命令（在群里用）

| 命令 | 谁能用 | 作用 |
| --- | --- | --- |
| `/bidistart` | 任何人 | 欢迎语（重新看一遍怎么用）——**不叫 `/start`**：那是宿主内建 start 插件的命令，同名登记会被拒 |
| `/bidihelp` | 任何人 | 帮助（私聊里也能用） |
| `/bidistatus` | 主人 | 运行状态：群、话题数、库大小、计数 |
| `/bidiban [id] [原因]` | 主人 | 拉黑；**在某个话题里发就不用带 id**（目标 = 话题主人） |
| `/bidiunban <id>` | 主人 | 解封 |
| `/bidilist` | 主人 | 拉黑列表 |
| `/bidiwarn <id>` / `/bidiclear <id>` | 主人 | 看 / 清零警告 |
| `/bidiword add|del|list` | 主人 | 维护广告关键词 |
| `/bidisay <id> <文字>` | 主人 | 主动给某个用户发一条（General 里也能用） |
| `/bidicard` | 主人 | 在当前话题里重发这个用户的名片 |

**怎么回复用户**：在对应话题里直接说话即可（文本重发、媒体复制），也可以引用某条消息再回；
不需要 @ 谁，话题本身就是"这个用户"。

## 本地库

`data/bidibot/db.json`，每次变更原子落盘；坏文件会备份成 `db.json.bak.<时间戳>` 再重建空库。

```json
{
  "version": 1,
  "topics":   { "<user_id>": { "user_id": 0, "thread_id": 0, "name": "", "created_at": 0, "card_message_id": 0 } },
  "bans":     { "<user_id>": { "user_id": 0, "at": 0, "reason": "", "by": 0, "auto": false } },
  "warns":    { "<user_id>": { "count": 0, "last_at": 0, "last_reason": "" } },
  "profiles": { "<user_id>": { "user_id": 0, "name": "", "username": null, "lang": null, "first_seen": 0, "base_message_id": 0 } },
  "reports":  {},
  "last_notice": { "<user_id>": 0 }
}
```

- `topics` 是"用户 ↔ 话题"的唯一映射；删掉某条再重启，下次该用户来消息会**重新建话题**。
- 主人把话题删了也不用改库：转发失败时插件会自动清记录、重建一次话题。
- `reports` 是 v0.1 的私聊映射遗留字段，v0.2 不再使用，保留只为旧库能直接读。

## 离线回放

```bash
fuyubot.exe run --replay update.json --plugin-dir plugins --replay-wait 6000
```

回放里 `createForumTopic` / `forwardMessages` / `sendMessage` 都只留档不真发，所以：
能看到"要建哪个话题、转发哪条消息、发给谁"，但拿不到真实 thread id（后续消息会再建一次话题）。
要验完整链路（话题复用、话题内回复回到用户）必须上真账号。

## 构建 / 验证

```bash
cargo build --release -p fuyubot-plugin-bidibot
cargo test -p fuyubot-plugin-bidibot
cargo clippy -p fuyubot-plugin-bidibot --all-targets -- -D warnings
python scripts/check_abi.py target/release/bidibot.dll
python scripts/package.py --plugin bidibot --platform windows-x86_64
```

## 已知边界

- 只处理私聊与 `group_id` 指定那一个群；别的群 / 频道一律忽略。
- 匿名管理员（`messageSenderChat`）发的消息无法归属，忽略。
- 主人身份只认 `owner_id` 一个人；群里其他人聊天不会转给用户。
- 话题名在创建时定下，用户后来改昵称不会自动改话题名（需要的话删话题让插件重建）。
- 建话题失败（bot 不是管理员 / 没管理话题权限）会在日志里报错，并告诉用户"暂时送不出去"。
