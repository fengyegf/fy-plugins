# FuyuBot 插件集（fy-plugins）

## 项目介绍

fy-plugins 是一个 Cargo workspace，包含 10 个可独立构建、独立发布、独立加载的 FuyuBot 原生插件，
全部用 Rust 编写（一个插件 = 一个 cdylib）。插件通过 FuyuBot 宿主的 Native ABI v6 导出符号与
宿主通信，卡片类插件的界面统一交给宿主上的 `fuyubot.html2img` 服务渲染，插件自身不带渲染引擎。

发布产物遵循 FuyuBot 插件分发包规范：每个平台一个 ZIP，包根就是插件目录（`plugin.json` +
`bin/` 下的动态库），命名为 `<id>-<version>-<platform>.zip`，由
[CatMoeCircle/FuyuBot-plugins](https://github.com/CatMoeCircle/FuyuBot-plugins) 收录进插件商城。

- 仓库：<https://github.com/fengyegf/fy-plugins>
- 分发包规范：<https://catmoecircle.github.io/FuyuBot-docs/docs/plugin/bundle/>
- 宿主 ABI：v6

## 目录结构

```text
.
├── crates/
│   ├── fuyubot-plugin-api/          # 宿主 SDK 门面（vendored，ABI v6）
│   └── fuyubot-plugin-<id>/         # 每个插件一个 crate
│       ├── Cargo.toml
│       ├── plugin.json              # 插件清单：id / entry / platforms / depends / homepage
│       ├── config.default.toml      # 默认配置（打包时随包带入，优先级最低）
│       ├── README.md                # 本插件的命令、配置与使用说明
│       ├── assets/                  # 卡片模板等编译期资产（如有）
│       └── src/                     # 插件源码（host / cmd / core / render 分层）
├── scripts/
│   ├── package.py                   # 打 bundle ZIP（校验清单、平台、体积与命名）
│   ├── plan.py                      # 生成发布矩阵
│   ├── check_abi.py                 # 静态检查导出符号
│   └── fetch-models.py              # 可选：下载离线模型（rusto）
├── docs/replay/                     # 离线回放夹具（宿主 --replay 用）
├── dist/                            # 本地打包产物（不入库）
├── .github/workflows/               # ci / release / verify-platforms
└── Cargo.toml                       # workspace 与统一依赖
```

安装到宿主后的插件目录：

```text
plugins/<id>/
├── plugin.json
├── config.default.toml   # 可选
└── bin/
    └── lib<lib>.so       # Windows <lib>.dll，macOS lib<lib>.dylib
```

## 插件一览

| 插件 id | 名称 | 版本 | 平台 | 依赖 | 说明 |
| --- | --- | --- | --- | --- | --- |
| [`fengyegf.whois`](crates/fuyubot-plugin-whois/README.md) | Whois | 0.1.6 | 全平台 | html2img（硬） | 查询域名注册信息与 IP 归属，渲染成信息卡片 |
| [`fengyegf.musiccard`](crates/fuyubot-plugin-musiccard/README.md) | MusicCard | 0.1.6 | 全平台 | html2img（硬） | 生成网易云音乐卡片 |
| [`fengyegf.epic`](crates/fuyubot-plugin-epic/README.md) | Epic | 0.1.6 | 全平台 | html2img（硬） | 生成 Epic 免费游戏卡片 |
| [`fengyegf.kfc`](crates/fuyubot-plugin-kfc/README.md) | KFC | 0.1.7 | 全平台 | html2img（硬） | 抓一句肯德基疯话，渲染成推特卡片 |
| [`fengyegf.system`](crates/fuyubot-plugin-system/README.md) | System | 0.1.7 | 全平台 | html2img（硬） | 查看宿主机与机器人状态，渲染成信息卡片 |
| [`fengyegf.trending`](crates/fuyubot-plugin-trending/README.md) | GitHub Trending | 0.1.2 | 全平台 | html2img（硬） | 抓 GitHub Trending 榜单，渲染成卡片墙 |
| [`fengyegf.ppxfy`](crates/fuyubot-plugin-ppxfy/README.md) | PPXFY | 0.1.6 | 桌面三平台 | storage.mongo（软） | 从 DMM / DLSite 下载并解压作品，记录与 Telegraph 上传 |
| [`fengyegf.bidibot`](crates/fuyubot-plugin-bidibot/README.md) | Bidibot 双向私聊 | 0.1.2 | 全平台 | html2img（软） | 论坛群话题式中转：私聊进话题、主人在话题里回复回用户 |
| [`fengyegf.rusto`](crates/fuyubot-plugin-rusto/README.md) | RustO OCR | 0.1.3 | 全平台 | — | 纯 Rust OCR，发图出文本，并提供 `rusto.ocr` 服务 |
| [`fengyegf.llm`](crates/fuyubot-plugin-llm/README.md) | LLM Gateway | 0.1.0 | 全平台 | — | 零 AI SDK 的三大协议网关（OpenAI Chat / Responses / Anthropic Messages），提供工具与服务 |

平台口径：全平台 = `windows-x86_64` / `linux-x86_64` / `macos-aarch64` / `android-aarch64` /
`android-x86_64`；桌面三平台 = 不含 Android。唯一来源是各 crate 的 `plugin.json`。
html2img 指宿主上的 `fuyubot.html2img` 插件（服务名同名），卡片类插件没有它不会出图。
插件 id 采用 **作者.插件名** 命名：作者是仓库 owner `fengyegf`，所以 id 形如 `fengyegf.whois`，
安装目录是 `plugins/<id>/`，配置段是 `[plugin.<id>]`（即 `[plugin.fengyegf.whois]`）。

## 使用说明

### 安装

从商店安装（推荐）：`fuyubot run --tui` → 「商店」→ 搜索插件 → 安装 → 重启实例。

本地目录 / 包安装：

```bash
fuyubot plugin install ./fengyegf.whois    # 目录（目录名 = 插件 id）
fuyubot plugin uninstall whois            # 卸载（配置与 data/ 保留）
```

手动安装：把 ZIP 解压到 `plugins/<id>/`（ZIP 根就是插件目录，不要多套一层），重启后用
`fuyubot plugin list` 确认版本、平台与依赖。

### 配置

所有插件配置都在 `fuyubot.toml` 的 `[plugin.<id>]` 段，字段见各插件 README 的「配置一览」。
插件目录里的 `config.default.toml` 是打包方给的默认值，优先级最低：运行时落盘与用户配置里
已有的键不会被覆盖。

### 构建与打包

```bash
cargo build --release --locked -p fuyubot-plugin-whois
python scripts/package.py --plugin whois --platform windows-x86_64
```

产物在 `dist/<id>-<version>-<platform>.zip`。Android 需要先装 target、NDK 与 `cargo-ndk`，
再用 `--platform android-aarch64` / `android-x86_64` 打包。打包脚本会校验 `plugin.json`、
条目命名、符号链接、单包 50 MB / 解压 200 MB / 2000 文件上限。

Linux x86_64 还有一条硬约束：动态库里的 `GLIBC_x.y` 符号版本由**构建机**决定，用太新的
发行版构建会在老机器上 `dlopen` 失败，而宿主只报一句 `dlopen failed`，真实原因类似：

```text
/lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.43' not found (required by librusto.so)
```

所以 Linux x86_64 的发布基线是 **glibc 2.28**，CI 用 `cargo zigbuild --target
x86_64-unknown-linux-gnu.2.28` 构建；`scripts/package.py` 打包前会用
`scripts/check_glibc.py` 静态检查，超过基线直接失败。本地在 Ubuntu 24.04+ 上直接
`cargo build` 出来的 `.so` 会被拦下（需要别的构建环境，或确认目标机器够新后加
`--max-glibc 2.39` 放宽）。

### 发布

一个插件一个 tag，推 tag 触发 `.github/workflows/release.yml` 构建全部声明平台并挂到 Release：

```bash
git tag fengyegf.whois-v0.1.6
git push origin fengyegf.whois-v0.1.6
```

### 构建证明与 Release 不可变性

发布资产会同时生成**构建来源证明**（`actions/attest-build-provenance`），可校验某个 zip
确实是本仓库 CI 构建的：

```bash
gh attestation verify dist/fengyegf.whois-0.1.6-windows-x86_64.zip --repo fengyegf/fy-plugins
```

仓库已开启 **Release 不可变性**（Settings → Releases → Release immutability）：已发布
Release 的资产与标签不能再修改/替换，GitHub 还会自动生成 release attestation，插件商店才能
信任下载到的资产。发布流程已改成「先建 draft → 传完全部资产 → 再发布」，避免发布后补资产。

### 依赖

- 宿主：FuyuBot（ABI v6）与新版 `fuyubot-plugin-api`；`fuyubot-sdk check` 需与宿主同版本。
- 卡片类插件：必须装 `fuyubot.html2img`（硬依赖，缺了停在 Pending）；`bidibot` 为软依赖，
  没装自动降级文字名片。
- `ppxfy`：入库依赖 `storage.mongo`（mongo-plugin 提供），没装只是不入库。
