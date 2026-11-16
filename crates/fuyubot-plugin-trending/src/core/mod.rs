//! 与宿主无关的能力：抓取 GitHub Trending 页面、把 HTML 解析成榜单、数据模型与配置。
//!
//! 这里不碰 TDLib，也不碰 html2img：输入是「周期 + HTTP」，输出是 [`model::CardData`]。

pub mod avatar;
pub mod config;
pub mod fetch;
pub mod model;
pub mod parse;

pub use config::Config;
pub use model::{CardData, LanguageStat, Period, TrendingItem};
