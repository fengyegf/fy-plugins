//! 与宿主、引擎都无关的东西：配置与错误。

pub mod config;
pub mod error;

pub use config::Config;
pub use error::{PluginError, Result};
