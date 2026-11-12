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
| `whois` | `crates/fuyubot-plugin-whois` | 0.1.2 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `whois.dll` |
| `musiccard` | `crates/fuyubot-plugin-musiccard` | 0.1.2 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `bin/musiccard.dll` |
| `epic` | `crates/fuyubot-plugin-epic` | 0.1.2 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `epic.dll` |
| `kfc` | `crates/fuyubot-plugin-kfc` | 0.1.2 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `kfc.dll` |
| `ppxfy` | `crates/fuyubot-plugin-ppxfy` | 0.1.1 | Windows / Linux / macOS（**不含 Android**） | `ppxfy.dll` |
| `system` | `crates/fuyubot-plugin-system` | 0.1.2 | Windows / Linux / macOS / Android arm64 / Android x86_64 | `system.dll` |

平台列表的唯一来源是各 crate 的 `plugin.json`；发布流水线按它生成构建矩阵。

> **图像生成**：`epic` / `kfc` / `musiccard` / `system` / `whois` 的卡片图统一由宿主上的
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

验证记录：<https://github.com/fengyegf/fy-plugins/actions/runs/42824916393>。
上面的验证是 ABI v5 时期跑的；迁到 ABI v6 后，只有构建/打包结论仍然成立，宿主加载
需要新版（ABI v6）的 `fuyubot-sdk` / FuyuBot 才能验证——本机附带的
`fuyubot-sdk.exe` 还是 ABI v5，不能再用来 check ABI v6 插件。

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
