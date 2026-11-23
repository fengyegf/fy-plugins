//! 宿主适配：订阅回调与工作线程（回调里绝不阻塞）。

pub mod worker;

pub use worker::Worker;
