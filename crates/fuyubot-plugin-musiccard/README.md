# MusicCard

插件 id `fengyegf.musiccard` · 版本 `0.1.5` · ABI v6 · 平台 windows-x86_64 / linux-x86_64 / macos-aarch64 / android-aarch64 / android-x86_64

## 项目介绍

`/musiccard` 接收歌曲 ID、网易云歌曲链接或关键词，取歌名、歌手、专辑、封面和歌词（自动匹配中英译文），把数据映射进 `assets/card.html` 的 Vue 模板，交给宿主的 `fuyubot.html2img` 服务（`render`，`vue: true`）渲染成 650×1220 JPEG，再以照片回复原消息。

工作链路：`/musiccard` → 网易云接口（搜索 / 详情 / 歌词）→ 选歌词 → 二维码 + 模板 data → `fuyubot.html2img` → 发照片 → 清理进度消息与渲染产物。插件只做数据映射：封面用网易云 CDN 公网 URL，二维码以 `data:image/png;base64,…` 内联，不拼 HTML、不做位图合成。

![预览](preview.jpg)

## 目录结构

源码 crate：

```text
crates/fuyubot-plugin-musiccard/
├── Cargo.toml            # cdylib + rlib
├── plugin.json           # id / 版本 / entry = bin/musiccard.dll
├── config.default.toml   # 随包发布的默认配置
├── README.md
├── preview.jpg           # 预览图
├── assets/card.html      # Vue 模板 + 内联 CSS（编译期嵌入）
├── examples/             # full_card / netease_probe / render_card_musiccard
└── src/
    ├── lib.rs            # Plugin 生命周期、inject 声明
    ├── config.rs         # [plugin.fengyegf.musiccard] 配置
    ├── help.rs           # 把 /musiccard 登记进宿主命令表
    ├── model.rs          # 歌词行等共享模型
    ├── cmd/              # mod / musiccard / parser
    ├── netease/          # mod / client / lyric
    ├── render/           # mod / html2img / qr
    ├── host/             # command / messenger / worker / mod
    └── util/mod.rs       # 文本截断等纯函数
```

部署后的 bundle：

```text
plugins/fengyegf.musiccard/
├── plugin.json           # entry 指向 bin/musiccard.dll
└── bin/musiccard.dll     # Linux 为 libmusiccard.so，macOS 为 libmusiccard.dylib
```

## 命令一览

| 命令 | 权限/范围 | 作用 |
| --- | --- | --- |
| `/musiccard <歌曲ID / 链接 / 关键词>` | `user`；私聊 + 群 | 取歌、选歌词、渲染 650×1220 音乐卡片并以照片回复；无参数时回用法提示 |

命令前缀来自宿主顶层 `[config].PREFIXES`，缺省为 `/`、`!`、`！`、`.`、`#`；`[plugin.fengyegf.musiccard]` 没有 `prefixes` 键。命令名大小写不敏感，支持 `/musiccard@机器人名`，关键词可按空格分隔。

## 配置一览

配置段 `[plugin.fengyegf.musiccard]`，全部可选：

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `request_timeout_ms` | `15000` | 普通接口超时（毫秒，最小 1）；当前取数层固定用 15s，该字段读取后只写入启动日志，修改它暂不改变实际请求 |

`config.default.toml` 固化上表默认值，随发布包带入，按最低优先级并入 `[plugin.fengyegf.musiccard]`，可被用户配置覆盖。命令前缀不属于插件段，读宿主 `[config].PREFIXES`；老配置里残留的 `temp_dir` 会被安静忽略。

## 使用说明

**安装**：商店里搜索 `musiccard` 安装；或 `fuyubot plugin install ./musiccard`（指向插件目录或解压后的包）；或把 zip 直接解压到 `plugins/fengyegf.musiccard/`，确认根上是 `plugin.json` 与 `bin/`，然后重启 `fuyubot run`。

**最小配置**（`fuyubot.toml`）：

```toml
[plugin.fengyegf.musiccard]
request_timeout_ms = 15000

[config]
PREFIXES = ["/", "!", "！", ".", "#"]
```

**典型用法**：

```text
/musiccard 347230
/musiccard https://music.163.com/song?id=347230
/musiccard 周杰伦 晴天
```

**构建与打包**：

```bash
cargo build --release -p fuyubot-plugin-musiccard
python scripts/package.py --plugin musiccard --platform windows-x86_64
```

产物名为 `<id>-<version>-<platform>.zip`，ZIP 根就是插件目录内容（`plugin.json` + `bin/musiccard.dll`）；打包脚本会把 `plugin.json` 的 `config` 指向的默认配置（本插件为 `config.default.toml`）随包带入。

**发布**：

```bash
git tag fengyegf.musiccard-v0.1.5
git push origin fengyegf.musiccard-v0.1.5
```

## 项目依赖（软/硬）

| 类型 | 服务名 / 要求 | 说明 |
| --- | --- | --- |
| 硬依赖 | `fuyubot.html2img` | `Plugin::inject` 声明；未安装或渲染服务未就绪时插件停在 Pending，`/musiccard` 不可用 |
| 硬依赖（外部网络） | `music.163.com`、`interface3.music.163.com` | 搜索、歌曲详情、歌词接口；封面 URL 由 html2img 服务下载（单张上限 8 MiB） |
| 软依赖 | 无 | 不依赖其它插件；命令表登记失败只影响 `/help` 展示 |
| 宿主内建（不算插件依赖） | `fuyubot.host.command`、`fuyubot.host.commands` | 回复命令消息、登记 `/help` 行；宿主预置服务，无需安装 |

宿主要求：ABI v6（FuyuBot 0.1.5+），支持 `Plugin::inject`、`call_service` 与 `updateNewMessage` 订阅；运行机器需能访问网易云接口与 CDN。
