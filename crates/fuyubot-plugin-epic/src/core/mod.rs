//! 通用能力：配置、HTTP、Epic 接口。全部不接触宿主 API（除配置读取）。

pub mod config;
pub mod epic;
pub mod fetch;

pub use config::Config;
