本仓库包含一份第三方源码副本：

- `crates/fuyubot-plugin-api/`
  - 来源：FuyuBot SDK 的 `fuyubot-plugin-api` 0.1.2 门面（上游仓库
    <https://github.com/CatMoeCircle/FuyuBot-SDK>）
  - 许可：MIT OR Apache-2.0
  - 原因：上游 0.1.x 已从 crates.io yank，且 0.1.3 只剩弃用说明；门面里的
    `HostExt` / `Subscription` 仍被插件使用。这里保留 0.1.2 的门面源码，底层
    ABI 依赖升到 `fuyubot-abi 0.1.3`（ABI v6），与当前 FuyuBot 宿主一致。

除该目录外，各插件 crate 的许可以其自身 `Cargo.toml` 为准。
