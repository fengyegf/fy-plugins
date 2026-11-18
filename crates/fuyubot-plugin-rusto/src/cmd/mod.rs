//! 命令层：/ocr 一条命令拆一个文件。
//!
//! 这里只做「把命令翻译成一次识别 + 一次回复」，不直接碰 ABI、也不碰引擎实现细节。

pub mod ocr;
pub mod parser;

use crate::host::Ctx;
use crate::host::photo::Incoming;

/// 一条待执行的命令。
pub struct Job {
    /// 触发它的消息。
    pub incoming: Incoming,
    /// 解析出的命令。
    pub command: parser::Command,
}

/// 执行一条命令（在业务线程里跑，允许阻塞）。
pub fn handle(ctx: std::sync::Arc<Ctx>, job: Job) {
    ocr::handle(ctx, job);
}
