# NOTICE — RustO OCR 插件第三方声明

本插件把以下第三方代码与模型权重静态链接或随发布包分发。打 bundle 时请把本文件
一起带上（`python scripts/package.py --plugin rusto --platform <p> --assets NOTICE.md`）。

## RustO!（`rusto-rs` 0.4.1）

- 上游：<https://github.com/byrizki/rusto-rs>（crates.io 同名 crate）
- 许可：**MIT**（Copyright (c) 2024 RustO Contributors），全文见上游 `LICENSE`
- 引入方式：`rusto-rs = { version = "=0.4.1", default-features = false, features = ["parallel"] }`，
  静态链接进 `rusto.dll` / `librusto.so` / `librusto.dylib`
- 说明：本插件**不修改**上游源码，只在外面包一层 FuyuBot 适配层（`src/engine/`）。

## PP-OCRv6 权重（默认模型 `ppocrv6-tiny-int8`）

- 来源：**PaddleOCR**（百度）的 PP-OCRv6 检测 / 识别模型
- 许可：**Apache-2.0**（权重可商用再分发；保留 PaddleOCR 版权与 Apache-2.0 声明）
- 分发：HuggingFace 仓库 `byrizki/rusto-models`
- 默认预设：`ppocrv6-tiny-int8`，三件套 `det.mnn`（543,168 B）、
  `rec.mnn`（2,912,240 B）、`dict.txt`（27,156 B），合计约 3.5 MB；该目录下
  det / rec 同时提供 `.mnn` 与 `.rten` 两种格式，插件优先 `.mnn`
- 变体：同仓库还有 `ppocrv6-tiny`（FP32，约 6.2 MB）/ `-fp16` / `-small` /
  `-medium` 等档位，权重许可同为 Apache-2.0

## 不使用 Indra 权重

同仓库的 `indraocr/`（Indra OCR）权重是 **modified OpenRAIL-M**，对使用场景和
组织规模有限制（含「不得用于与 Datalab OCR API 竞争的产品」等条款）。本插件
**默认不启用、也不随包分发** Indra 权重；`scripts/fetch-models.py` 默认只下载
`ppocrv6-tiny-int8`（`--preset` 可换 PP-OCRv6 档位）。要启用需自行评估并遵守其许可。

## 模型格式说明（Alibaba MNN）

- `.mnn` 是**阿里巴巴 MNN** 推理框架的模型格式（<https://github.com/alibaba/MNN>，
  Apache-2.0）。本插件里的 `.mnn` 文件由 PP-OCRv6 权重转换而来，**权重本身仍是
  Apache-2.0**，转换不改变许可。
- 运行 `.mnn` 的是 **`rusto-nn`**：RustO! 自带的纯 Rust MNN 运行时实现
  （MIT OR Apache-2.0）。插件**不链接**阿里巴巴 MNN 的 C++ 运行时，也不引入其
  代码，因此没有额外的 C++ / OpenCV / Python 运行时依赖。
- 备选的 `.rten` 是 RTen 模型格式（`rten-model-file`，MIT OR Apache-2.0），只在
  上游目录没有 `.mnn` 时使用；两条路径都不影响上面的权重许可。

## 其它依赖

`fuyubot-plugin-api`（本地 vendored，MIT OR Apache-2.0）、`image`、
`base64`、`serde_json`、`thiserror` 等 Rust crate 均为 MIT 或
MIT OR Apache-2.0，完整清单见仓库根目录 `Cargo.lock` 与各 crate 自带的
`LICENSE`。本插件自身的许可与仓库其它插件一致，以 `Cargo.toml` 为准。
