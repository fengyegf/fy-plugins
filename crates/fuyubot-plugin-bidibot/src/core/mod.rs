//! 纯逻辑层：配置、存储、语言、翻译、广告与 LLM 客户端。
//!
//! 这一层**不碰宿主**（除了 Config::load 读一次配置），所以能在 cargo test 里直接跑：
//! 单测只构造数据结构，不需要真 TDLib，也不会发网络请求（HTTP 相关的靠超时与错误分支兜）。

pub mod ad;
pub mod config;
pub mod llm;
pub mod skills;
pub mod store;

pub use config::Config;
