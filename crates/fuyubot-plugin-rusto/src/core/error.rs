//! 插件内部错误：只用于业务线程里往上抛，绝不跨 C ABI。

use crate::engine::EngineError;

/// 命令/服务处理过程中的失败。
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// 调宿主（TDLib / simplify / 共享服务）失败。
    #[error("宿主调用失败：{0}")]
    Host(String),
    /// 图片下载失败。
    #[error("图片下载失败：{0}")]
    Download(String),
    /// 引擎侧失败。
    #[error("识别失败：{0}")]
    Engine(#[from] EngineError),
    /// 回复失败。
    #[error("回复失败：{0}")]
    Reply(String),
    /// 入参不合法。
    #[error("入参不合法：{0}")]
    BadInput(String),
}

/// 插件内部结果。
pub type Result<T> = std::result::Result<T, PluginError>;
