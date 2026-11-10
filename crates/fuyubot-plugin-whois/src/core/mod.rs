//! 与宿主无关的能力层：配置、HTTP、数据模型、域名清洗。
//!
//! 这里只保留配置读取所需的宿主调用，其余逻辑均与本机宿主解耦；命令层拿到结果后，
//! 只负责「说什么 / 发什么」。

pub mod config;
pub mod domain;
pub mod favicon;
pub mod fetch;
pub mod model;

pub use config::Config;
