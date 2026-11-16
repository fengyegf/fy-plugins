# ABI v6 审计报告：6 个原生插件 + `/help` 联动

审计时间：2026-11-16　范围：`crates/fuyubot-plugin-{epic,kfc,musiccard,ppxfy,system,whois}`

依据：FuyuBot 文档站 `llms.txt` / `llms-full.txt`（ABI 契约、ABI v5→v6、命令表与 /help、插件联动），
以及外部图片版 help 插件 `xiaoqvan/fuyubot-plugins/crates/fuyubot-plugin-help/README.md`。
统一口径落在 [`docs/abi-v6-spec.md`](docs/abi-v6-spec.md)。

## 一、结论

| 契约 | 结论 | 证据 |
| --- | --- | --- |
| ABI 版本 = 6 | 通过（未改动） | workspace `fuyubot-abi = "=0.1.3"`（`ABI_VERSION = 6`），`Cargo.lock` 锁 0.1.3；6 个 `plugin.json` 均为 `"abi": 6` |
| 七个导出符号 | 通过 | `python scripts/check_abi.py target/debug/*.dll` → 6/6 都是 `7/7`（`plugin_init` / `plugin_free_string` 必需，其余五个可选齐全） |
| 回调 `C-unwind` | 通过 | 6 个 `host/worker.rs` 与 `lib.rs` 的 `on_update` 均为 `extern "C-unwind"`；仓内仅存的 `extern "C"` 是**宿主侧** VTable 桩（`fuyubot-plugin-api` 测试、system 的假宿主），按契约本就该是 `extern "C"` |
| 回命令不自己拼 `sendMessage` | **已修** | 6 个 crate 新增 `src/host/command.rs`，`reply_to = Some(id)` 一律走 `fuyubot.host.command.reply`；`inputMessageReplyToMessage` 在源码中已归零（回放里出现的 `reply_to` 由宿主补） |
| 字符串所有权 | 通过 | 宿主返回指针一律当场 `CStr::from_ptr(..).to_string_lossy()` 拷成 `String` / `Value`（门面的 `parse_payload` / `call_service` / `wait_for_update_json`），不 free、不跨调用持有；插件自己分配的字符串只由 `define_plugin!` 生成的 `plugin_free_string` 释放 |

## 二、改了什么

### 1. 命令回复改走宿主（ABI v6 第 4 条）

旧写法自己拼 `sendMessage`：`{chat_id, input_message_content, reply_to}` + `simplify_invoke("send_message")`。
这会让 user 账户下本该「编辑原命令消息」的回复变成发新消息，群里的 ephemeral 命令也没有临时消息。

现在每个 crate 都有 `src/host/command.rs`：

- 入参 `{"chat_id", "message_id", "timeout_ms", "payload":{"input_message_content"}}`；
- 成功回 TDLib 最终 `Message`（取 `id` / `chat_id`），`{"@type":"error",…}` 映射成 `Error::Op`；
- **不再传** `reply_to` / 目标 chat：发新消息、编辑原命令、ephemeral 由宿主决定；
- 同步阻塞，只允许在工作线程调用（回调里只投递 channel，未变）。

保留 simplify 通道的只有两类，且都在代码里写明了原因：主动推送（panic 兜底、进度提示）与
相册 `sendMessageAlbum`（宿主命令回复服务只接 `sendMessage` 参数）。

### 2. `/help` 联动按上游图片版插件 README 适配

上游 README 的关键事实：**命令表归宿主**（0.1.4 起），`fuyubot.help.commands` 只是一个版本的兼容别名；
图片版 `/help` 只读宿主的 `fuyubot.host.commands.list`，按 `role` / `scopes` / `hidden` 过滤、
按宿主判定的 `plugin` 字段分组，出图走 `fuyubot.html2img`，发送走 `fuyubot.host.command.reply`。

对应改动（6 个 crate 一致）：

- `help.rs`：服务名 `fuyubot.help.commands` → `fuyubot.host.commands`；登记项补 `role` / `scopes` / `hidden`；
- `lib.rs`：删掉只为 help 写的 `soft_inject()`（宿主服务启动时就在；ppxfy 只保留可选的 `storage.mongo`）；
- 目录注释、README、单测同步改（单测断言 payload 五个字段齐全、名字 / role / scope 合法）。

登记的命令表（`hidden` 全为 `false`，别名写进 description，不单独占一行）：

| 插件 | 命令 | role | scopes |
| --- | --- | --- | --- |
| epic | `epic`（别名 `/epicfree`） | user | private, group |
| kfc | `kfc` | user | private, group |
| musiccard | `musiccard` | user | private, group |
| ppxfy | `download` / `galgame` / `cancel` / `clearcache`（别名 `d` / `p` 等） | user | private, group |
| system | `system` | user | private, group |
| whois | `whois` | user | private, group |

取值原则：**只按代码里实际的权限判断填**——这 6 个插件的命令目前都没有角色门槛，所以一律 `user` +
`private`/`group`；没有登记 `channel`，因为频道里没有可回复的命令上下文。登记失败只记 warn，不影响命令本身。

### 3. 审计工具

- `scripts/check_abi.py`：读 PE 导出表核对七个 ABI 符号（不 dlopen、不执行插件代码），
  退出码可直接进 CI。用法：`python scripts/check_abi.py target/debug/*.dll`。
- `scripts/replay-smoke.sh`：一键离线回放（见第四节）。
- `docs/abi-v6-spec.md`：本次审计的统一口径，未来新插件照它写即可。

## 三、静态验证

```text
# 本机 cargo（ppxfy 需要 CARGO_PROFILE_DEV_DEBUG=0 规避 cl D8050，见「遗留风险」）
cargo check --workspace --all-targets --offline   → Finished
cargo test  --workspace --offline                 → 114 passed, 0 failed（6 个插件 109 + 门面 5）
cargo clippy --workspace --all-targets --offline -- -D warnings → 无告警
cargo fmt --all -- --check                        → 通过（CI 的 fmt 门槛）
cargo build --workspace --offline                 → 6 个 cdylib
python scripts/check_abi.py target/debug/*.dll    → 6/6 都是 7/7 符号
```

## 四、离线回放验证（不接账号 / 不接 TDLib）

宿主用现成安装目录里的 `fuyubot.exe`（**0.1.6，ABI v6**）；插件是本仓刚编出来的 6 个 debug 构建，
外加从宿主目录借的 `fuyubot.html2img`（卡片渲染硬依赖）与可选的 `fuyubot.mongo`。

一键复跑（脚本会自己组装临时插件目录、构建、回放）：

```bash
scripts/replay-smoke.sh <fuyubot 安装目录> docs/replay/new_message.json
```

### 结果

| 观察项 | 结果 |
| --- | --- |
| 加载 | `扫到 8 个，交给内核 8 个（active 8 / pending 0 / failed 0）`——6 个插件全部 ABI 6 通过 |
| 命令表登记 | 6 个插件全部 `{"ok":true, ..., "plugin":"<id>"}`，归属由宿主判定且正确 |
| `/help` 渲染 | 宿主内置文本版 `/help` 按插件分组列出了我们登记的全部命令（见下） |
| 命令回复 | 回放里真实发出的是 `sendMessage` + **宿主补的** `reply_to`，插件没自己拼 |
| 全链路 | `/system`：采集 → `fuyubot.html2img.render` 出图（17 s）→ 发照片回复；`/cancel`：回「当前没有活跃任务可取消。」 |

`/help` 读同一张命令表后渲染出来的样子（节选）：

```text
*Epic（epic）*
/epic — Epic 当前免费 / 即将免费的游戏卡片（别名 /epicfree）
*KFC（kfc）*
/kfc — 点个肯德基：随机一句疯话渲染成推特卡片
*MusicCard（musiccard）*
/musiccard — 生成网易云音乐卡片：歌名、链接或歌曲 ID
*PPXFY（ppxfy）*
/cancel — 取消当前聊天的下载任务
/clearcache — 清理没有任务登记的下载缓存
/download — 下载链接、解压并打包上传（别名 /d）
/galgame — 解析 DMM / DLSite 作品页生成分享消息（别名 /p）
*System（system）*
/system — 查看宿主机系统信息与机器人状态卡片
*Whois（whois）*
/whois — 查询域名注册信息与服务器 IP 归属，渲染成信息卡片
```

命令回复的证据（回放把插件发出的请求原样打出来）：

```text
#…658 sendMessage {"chat_id":7770001,…,"reply_to":{"@type":"inputMessageReplyToMessage","message_id":102}}
#…660 sendMessage {"input_message_content":{"@type":"inputMessagePhoto",…},"reply_to":{"@type":"inputMessageReplyToMessage","message_id":101}}
```

分别是 `/kfc` 拉不到文案时的降级文本（回 102 号命令消息）与 `/system` 的卡片图（回 101）。
`reply_to` 是宿主加的——插件侧源码里已没有 `inputMessageReplyToMessage`，这正是「回命令走宿主」的验证点。

### 回放环境的预期噪声

`--replay` 不接 TDLib，所以 `getOption` / `getMe` 只留档不返回：system 卡片里版本号、commit、bot 名会显示
「未知」；`/kfc` 这类要联网的命令在无网环境会走失败分支并降级成文本。这些都属于文档说明的
「回放证明判断与请求正确，不证明 Telegram 认参数」。

另外实测：`--replay` 下宿主**不会**因为硬依赖缺失把插件停在 Pending（不装 `fuyubot.html2img` 时 6 个插件
照样 active）。所以回放要验出图链路，必须把 `fuyubot.html2img` 一起装进插件目录——`scripts/replay-smoke.sh`
已经这么做了，找不到时只打一条警告。

## 五、遗留风险（按优先级）

1. **宿主版本下限抬到 0.1.4+**：`fuyubot.host.command` 与 `fuyubot.host.commands` 都是 0.1.4 的宿主预置服务。
   装在 0.1.3 宿主上，命令回复会失败（只记日志）。按 ABI v6 契约不做旧路径回退；若要兼容旧宿主，
   需要额外加一次性 fallback 到 simplify（会重新引入 user 账户下的形态问题）。
2. **未真机联调**：本次验证到「编译 + 单测 + 导出符号静态核对 + 离线回放」为止；
   `--replay` 不接 TDLib，所以真实的发消息形态（是否编辑原命令、ephemeral）仍需在有账号的宿主上确认。
3. **命令名冲突**：宿主的命令表跨插件同名会被拒（只 warn）。本仓 6 个插件内部无重名，
   与第三方插件重名需以宿主日志为准。
4. **ppxfy 本机构建**：`zstd-sys` 的 C 编译在本机 MSVC 下报 `cl D8050`（命令行过长/调试记录问题，
   与本次改动无关），构建时用 `CARGO_PROFILE_DEV_DEBUG=0` 绕过；CI/其它机器未复现。
5. **频道**：命令未登记 `channel`，`/help` 在频道里不会列出这些命令（执行本身不受影响）。
6. **版本号**：本次发布把 6 个插件各升一个 patch（见下一个 commit）。
