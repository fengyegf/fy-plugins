# RustO OCR 打包：在线包与离线包

RustO OCR 的发布物是插件商城的 bundle ZIP：`<id>-<version>-<platform>.zip`，
ZIP 根就是插件目录内容，`plugin.json` 在根。识别需要 PP-OCRv6 的 det / rec / dict
三件套（默认 `ppocrv6-tiny-int8`，约 3.5 MB），所以有两种发布形态：

| 形态 | 包内内容 | 体积 | 适用场景 |
| --- | --- | --- | --- |
| **在线包** | `plugin.json` + `rusto.dll` + `NOTICE.md` | 约 3.2 MiB | 用户机器能访问 HuggingFace，首启自己下模型 |
| **离线包** | 在线包 + `models/ppocrv6-tiny-int8/*` | zip 实测 **4.2 MiB**（解压后 ≈6.7 MB） | 国内网络、内网 / 离线部署、CI 里不想联网 |

两者用同一个 `scripts/package.py`，区别只在 crate 根下有没有 `models/`：

- `package.py` 会把 **crate 根下的 `models/` 与 `NOTICE.md` 自动打进 bundle**
  （存在才带），所以 CI / 本地都不用额外加参数；
- `--assets`（可重复）只是**追加**别的文件，路径相对 crate 根；
- crate 根既没有 `models/` 也没有 `NOTICE.md` 时，行为与以前**完全一致**
  （ZIP 里只有 `plugin.json` + 动态库，校验逻辑不变），其它插件不受影响。

## 前置

1. **版本三处一致**：`--version`（可省略）、`crates/fuyubot-plugin-rusto/Cargo.toml`
   的 `[package].version`、`plugin.json` 的 `version` 必须相同；
2. **目标平台已声明**：`plugin.json` 的 `platforms` 要包含 `--platform`；
3. **动态库已构建**：脚本按 `[lib].name`（这里是 `rusto`）找
   `target/release/rusto.dll`（Windows）/ `librusto.so`（Linux）/
   `librusto.dylib`（macOS）。

```powershell
cargo build --release --locked -p fuyubot-plugin-rusto
./fuyubot-sdk.exe check target/release/rusto.dll
```

## 在线包（不内置模型）

本地没有 `crates/fuyubot-plugin-rusto/models/` 时，跑一次打包就是在线包：

```powershell
python scripts/package.py --plugin rusto --platform windows-x86_64
```

产物（`NOTICE.md` 因为存在于 crate 根，会被自动带入）：

```text
dist/rusto-0.1.0-windows-x86_64.zip   (实测 4.2 MiB，含离线模型)
├── plugin.json
├── rusto.dll
└── NOTICE.md
```

用户装好后首次 `/ocr` 时插件自己下载模型（可用 `[plugin.rusto].mirror` 换源）。

> 合规提醒：MIT 与 Apache-2.0 都要求保留声明，`NOTICE.md` 已自动随包，不用手动加。

## 离线包（自动带入 `models/`）

### 1. 拉模型进 crate

```powershell
python scripts/fetch-models.py
```

默认从 HuggingFace `byrizki/rusto-models` 下载 **`ppocrv6-tiny-int8`** 的
det / rec / dict（优先 `.mnn`；上游只有 `.rten` 时会下 `.rten` 并在输出里提示），
落到 crate 根：

```text
crates/fuyubot-plugin-rusto/models/ppocrv6-tiny-int8/
├── det.mnn        543,168 B
├── rec.mnn      2,912,240 B
├── dict.txt        27,156 B
└── SHA256SUMS
```

想换档位（例如 FP32 的 `ppocrv6-tiny`）用 `--preset`：

```powershell
python scripts/fetch-models.py --preset ppocrv6-tiny
```

国内网络换镜像，重下加 `--force`：

```powershell
python scripts/fetch-models.py --mirror https://hf-mirror.com
python scripts/fetch-models.py --mirror https://hf-mirror.com --force
```

脚本会打印每个文件的 sha256（`det.mnn` / `rec.mnn` 还对已知值做校验），并写一份
`SHA256SUMS`，部署后可以拿它核对包里的模型没被换掉。

### 2. 打离线包

`models/` 已经在 crate 根，直接打包即可（不需要 `--assets`）：

```powershell
python scripts/package.py --plugin rusto --platform windows-x86_64
```

产物：

```text
dist/rusto-0.1.0-windows-x86_64.zip   (~7 MiB)
├── plugin.json
├── rusto.dll
├── NOTICE.md
└── models/
    └── ppocrv6-tiny-int8/
        ├── det.mnn
        ├── rec.mnn
        ├── dict.txt
        └── SHA256SUMS
```

脚本会在输出里多打两行：`自动带入 crate 根的 models、NOTICE.md` 和资产文件数。
同时把展开后的 bundle 留在 `dist/bundle-<platform>/`，方便肉眼核对解压后的布局。

虽然自动带入已经够用，想显式写出来也可以（不会重复）：

```powershell
python scripts/package.py --plugin rusto --platform windows-x86_64 `
    --assets models --assets NOTICE.md
```

### 自动带入与 `--assets` 的规则

| 规则 | 说明 |
| --- | --- |
| 自动带入 | crate 根的 `models/` 与 `NOTICE.md` 存在就带，不需要参数 |
| `--assets` 追加 | 可重复，路径**相对 crate 根**，例如 `--assets licenses` |
| 目录递归展开 | 目录下所有文件按原相对路径入包，空目录直接报错 |
| ZIP 内路径不变 | 解压后 `models/ppocrv6-tiny-int8/det.mnn` 还在同一位置 |
| 不允许越界 | 绝对路径、`..`、符号链接指向 crate 外，一律报错 |
| 不允许覆盖关键条目 | `plugin.json` 和 `entry`（动态库）不能被资产顶掉 |
| 重复指定只打一次 | 自动带入 + `--assets models` 不会重复写入 |
| 商城上限 | 单包 ≤50 MB、解压 ≤200 MB、≤2000 个文件，超了报错且不留半成品 |

### 3. 装到宿主上

离线包的模型和 dll 在同一个目录里，但 ABI 没有「插件自身目录」接口，所以
`model_dir` 要写**绝对路径**（或者把 `models/` 拷到宿主数据目录下留空 `model_dir`）：

```text
plugins/rusto/
├── plugin.json
├── rusto.dll
├── NOTICE.md
└── models/ppocrv6-tiny-int8/{det.mnn,rec.mnn,dict.txt}
```

```toml
[plugin.rusto]
model_dir = "C:/fuyubot/plugins/rusto/models"   # model 默认就是 ppocrv6-tiny-int8
```

> 只有用了 `--preset` 换成别的档位时才要改 `model`：`model` 就是模型目录名。
> 例如 `--preset ppocrv6-tiny` 生成的目录要配 `model = "ppocrv6-tiny"`。

## 多平台

`plugin.json` 声明了五个平台，逐个切 `--platform`（产物的 `entry` 会按平台改写，
`models/` 与 `NOTICE.md` 自动带入）：

```powershell
python scripts/package.py --plugin rusto --platform windows-x86_64
python scripts/package.py --plugin rusto --platform linux-x86_64
python scripts/package.py --plugin rusto --platform macos-aarch64
python scripts/package.py --plugin rusto --platform android-aarch64
python scripts/package.py --plugin rusto --platform android-x86_64
```

Android 要先装 target、NDK 与 `cargo-ndk`：

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --locked
cargo ndk -t arm64-v8a -t x86_64 --platform 24 build --release --locked -p fuyubot-plugin-rusto
```

模型与平台无关，同一份 `models/` 可以打进所有平台的离线包。

## CI（GitHub Actions）

`.github/workflows/release.yml` 已经可以选 `rusto`（`workflow_dispatch` 的
`plugin` 下拉新增），推 `rusto-v<版本>` tag 也会走同一条流水线。Package 步骤保持
原样（不带 `--assets`），靠 `package.py` 自动带入：

- 仓库里**提交了** `crates/fuyubot-plugin-rusto/models/`（`.gitignore` 没有忽略
  `models/`）→ 流水线产出**离线包**；
- 没提交模型、也没在 CI 里跑 `fetch-models.py` → 产出**在线包**，但 `NOTICE.md`
  仍会自动带上。

想在 CI 里出离线包又不想把模型提交进仓库，在 Package 之前加一步：

```yaml
- name: Fetch models
  run: python scripts/fetch-models.py
```

## 验证

脚本自己会做这些校验（少一条就 `SystemExit`，不产出半成品）：

1. crate 目录名 `crates/fuyubot-plugin-<id>` 与 `[package].name` 一致；
2. `plugin.json`：`schema == 1`、`id` 与 `--plugin` 一致、`kind == "native"`、
   `abi ∈ {5, 6}`；
3. `--version` / `Cargo.toml` / `plugin.json` 三处版本一致；
4. `--platform` 在 `plugin.json.platforms` 里；
5. `entry` 的目录前缀合法（拒绝绝对路径和 `..`），动态库存在；
6. 资产（自动带入 + `--assets`）都在 crate 内、不与 `plugin.json` / 动态库冲突；
7. 文件数 ≤2000、解压 ≤200 MB、压缩包 ≤50 MB；
8. 重新打开 ZIP 核对条目集合正好是「`plugin.json` + 动态库 + 资产」，且
   `version` / `platforms` / `entry` 与目标平台一致。

打包后手工再核一遍：

```powershell
python -c "import zipfile; print('\n'.join(sorted(zipfile.ZipFile('dist/rusto-0.1.0-windows-x86_64.zip').namelist())))"
Get-FileHash dist/rusto-0.1.0-windows-x86_64.zip -Algorithm SHA256
```

脚本最后会打印产物路径、体积、自动带入/资产文件数和 zip 的 sha256，发版记录里
带上它即可。

## 体积预算

| 组成 | 体积 |
| --- | --- |
| `rusto.dll` | 3,324,416 B（约 3.2 MiB） |
| `ppocrv6-tiny-int8` 三件套 | 约 3.5 MB（3.3 MiB） |
| `plugin.json` + `NOTICE.md` + `SHA256SUMS` | < 10 KB |
| **离线包合计** | 解压后 ≈6.7 MB / zip 实测 4.2 MiB（商城上限 50 MB） |

余量还很足：想换 FP32 `ppocrv6-tiny`（约 6.2 MB）或 `-small` / `-medium` 档位
也有空间，但默认的 INT8 已经够读截图和聊天记录，还更快。

## 常见报错

| 报错 | 原因 | 处理 |
| --- | --- | --- |
| `找不到 …/target/release/rusto.dll；请先执行 cargo build --release` | 没构建，或构建到了别的 platform | 先 `cargo build --release -p fuyubot-plugin-rusto` |
| `plugin.json 没有声明 <platform>` | `platforms` 少写了 | 补声明，或换 `--platform` |
| `命令行版本 … 与 …/Cargo.toml 的 … 不一致` | 版本没同步 | 三处一起改 |
| `--assets 必须是 crate 内的相对路径` | 传了绝对路径或 `..` | 用相对 crate 根的路径 |
| `--assets 指向的资产不存在` | 指定的文件不在 crate 里 | 检查路径；模型用 `fetch-models.py` 下 |
| `--assets 里的 … 与 plugin.json / 动态库冲突` | 资产路径和关键条目重名 | 换个目录名 / 文件名 |
| `bundle 解压后 … 超过商城上限 200 MiB` | 资产太大 | 只带默认 int8 模型，别带整份模型仓库 |
| `ZIP 内容不正确` | 包内条目与预期集合不一致 | 一般是脚本 bug 或磁盘有残留，删 `dist/bundle-*` 重打 |

## 发布检查清单

- [ ] `cargo build --release` 通过，`fuyubot-sdk.exe check` 七个 ABI 符号齐全；
- [ ] 三个版本号一致，`plugin.json` 声明了要发布的平台；
- [ ] `/ocr`、`/ocr detail`、`/ocr status`、`/ocr warm` 在真机/回放里跑通；
- [ ] 在线包 / 离线包二选一（或都发），包内带 `NOTICE.md`；
- [ ] `fetch-models.py` 打印的 sha256 与 `models/ppocrv6-tiny-int8/SHA256SUMS` 一致；
- [ ] 记下 `package.py` 输出的 zip sha256，随 Release 一起发布；
- [ ] 离线包在没网的机器上 `/ocr warm` 一次，确认 `loaded = true`。
