本仓库包含一份第三方源码副本：

- `crates/fuyubot-plugin-api/`
  - 来源：FuyuBot SDK 的 `fuyubot-plugin-api` 0.1.2（上游仓库
    <https://github.com/CatMoeCircle/FuyuBot-SDK>）
  - 许可：MIT OR Apache-2.0
  - 原因：上游 0.1.x 已从 crates.io yank，且 0.1.3 起 ABI 升到 v6，与当前宿主的
    ABI v5 不兼容；这里保留 0.1.2 的源码以便插件继续构建。

除该目录外，各插件 crate 的许可以其自身 `Cargo.toml` 为准。
