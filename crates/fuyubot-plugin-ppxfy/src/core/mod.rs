//! 能力层：一次业务流程用到的东西，一个能力一处，全部与宿主实现无关。
//!
//! - [`fetch`]：阻塞式 HTTP 下载与进度回报
//! - [`archive`]：分卷识别、主包挑选、压缩包清理
//! - [`tree`]：解压后的目录树展示
//! - [`page`]：抓网页（DMM 年龄确认改写、代理、图片字节）
//! - [`config`]：从宿主读配置项
//!
//! 成块的功能不在这里凑：galgame 作品解析见 `crate::galgame`，Telegraph 建页见
//! `crate::telegraph`。

pub mod archive;
pub mod config;
pub mod fetch;
pub mod page;
pub mod tree;

pub use config::Config;
