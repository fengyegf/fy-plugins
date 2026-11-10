//! 与宿主无关的能力层：拿文案、洗文本、读配置。
//!
//! 这里不出现 `HostApi`（`config` 除外——配置本来就是宿主给的），
//! 方便单独测试。

pub mod config;
pub mod fetch;
pub mod text;

pub use config::Config;
