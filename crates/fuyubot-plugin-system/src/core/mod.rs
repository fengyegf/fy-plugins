//! 与宿主无关的能力层：配置、系统信息采集、TDLib 查询、数据模型。
//!
//! 这一层不订阅更新、不发消息，只做「取数据」；命令层负责把它们拼成 [`model::CardData`]。

pub mod config;
pub mod model;
pub mod sysinfo;
pub mod td;

pub use config::Config;

/// 把字节数格式化成 `12.34 GB`（与 TS 版 `formatGB` 完全一致）。
pub fn format_gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
}

/// 百分比：`used / total`，四舍五入后夹到 0–100；`total == 0` 时返回 0。
pub fn percent(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((used as f64 / total as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_gb_like_typescript() {
        assert_eq!(format_gb(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_gb(0), "0.00 GB");
        // 与 `(bytes / 1024 / 1024 / 1024).toFixed(2)` 一致：四舍五入到两位。
        assert_eq!(
            format_gb(12 * 1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "12.50 GB"
        );
    }

    #[test]
    fn percent_rounds_and_clamps() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(2, 3), 67);
        assert_eq!(percent(5, 4), 100);
    }
}
