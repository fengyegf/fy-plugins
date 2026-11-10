//! 系统信息采集：CPU / 内存 / 磁盘。
//!
//! 对应 TS 版 `systeminformation` 的 `currentLoad()` / `mem()` / `fsSize()`：
//!
//! | TS | 这里 |
//! | --- | --- |
//! | `si.currentLoad().currentLoad` | [`sysinfo::System::global_cpu_usage`] |
//! | `mem.used / mem.total` | [`sysinfo::System::used_memory`] / `total_memory` |
//! | `si.fsSize()` | [`sysinfo::Disks`] |
//!
//! SWAP 不在卡片上展示（用户要求去掉）：卡片只保留 CPU / RAM 两条使用率。
//!
//! CPU 使用率是**两次采样之差**：sysinfo 第一次刷新只能拿到 0，必须隔
//! [`MINIMUM_CPU_UPDATE_INTERVAL`] 再刷一次；这个 200ms 的等待放在业务线程里，
//! 不阻塞 TDLib 接收线程。

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Instant;

use sysinfo::{
    CpuRefreshKind, Disks, MINIMUM_CPU_UPDATE_INTERVAL, MemoryRefreshKind, RefreshKind, System,
};

use crate::core::model::{DiskUsage, Gauge};
use crate::core::{format_gb, percent};

/// 磁盘最多展示几块：卡片的右列只有那么大，多余的在渲染前就砍掉。
pub const MAX_DISKS: usize = 4;

/// 一次采集的结果。
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// CPU / RAM 两条。
    pub gauges: Vec<Gauge>,
    /// 磁盘占用（按盘符排序，最多 [`MAX_DISKS`] 块）。
    pub disks: Vec<DiskUsage>,
}

/// 采集一次系统信息。总耗时约 200–260ms（CPU 两段采样）。
pub fn collect() -> Snapshot {
    let mut system = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram()),
    );

    // CPU：先刷一次，等最小采样间隔，再刷一次拿真实使用率。
    system.refresh_cpu_usage();
    sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_cpu_usage();
    system.refresh_memory();

    let mut gauges = vec![Gauge {
        label: "CPU".to_owned(),
        usage: system.global_cpu_usage().round().clamp(0.0, 100.0) as u8,
    }];

    let total_memory = system.total_memory();
    let used_memory = system.used_memory();
    gauges.push(Gauge {
        label: "RAM".to_owned(),
        usage: percent(used_memory, total_memory),
    });

    let disks = collect_disks();
    Snapshot { gauges, disks }
}

/// 采集磁盘：只留「有盘符、容量 > 0」的本地卷，按盘符排序去重。
fn collect_disks() -> Vec<DiskUsage> {
    let disks = Disks::new_with_refreshed_list();
    // BTreeMap 顺便给了盘符排序；同一盘符被列两次时保留容量大的那块。
    let mut by_letter: BTreeMap<String, DiskUsage> = BTreeMap::new();
    for disk in disks.list() {
        let Some(letter) = drive_letter(disk.mount_point()) else {
            continue;
        };
        let total = disk.total_space();
        if total == 0 {
            continue;
        }
        let available = disk.available_space().min(total);
        let used = total - available;
        let candidate = DiskUsage {
            label: letter.clone(),
            used: format_gb(used),
            total: format_gb(total),
            usage_percent: percent(used, total),
        };
        match by_letter.get(&letter) {
            Some(existing) if existing.total >= candidate.total => {}
            _ => {
                by_letter.insert(letter, candidate);
            }
        }
    }
    by_letter.into_values().take(MAX_DISKS).collect()
}

/// Windows 的卷挂载点形如 `C:\`；取第一个 ASCII 字母当盘符。
fn drive_letter(mount_point: &std::path::Path) -> Option<String> {
    let text = mount_point.to_string_lossy();
    let first = text.chars().next()?.to_ascii_uppercase();
    first.is_ascii_alphabetic().then(|| first.to_string())
}

/// 采集一次、格式化后返回「耗时多少毫秒」，给命令层打日志用。
pub fn collect_timed() -> (Snapshot, u128) {
    let started = Instant::now();
    let snapshot = collect();
    (snapshot, started.elapsed().as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_letter_reads_windows_mount_points() {
        assert_eq!(
            drive_letter(std::path::Path::new(r"C:\")),
            Some("C".to_owned())
        );
        assert_eq!(
            drive_letter(std::path::Path::new(r"d:\data")),
            Some("D".to_owned())
        );
        assert_eq!(drive_letter(std::path::Path::new("/tmp")), None);
    }

    #[test]
    fn collect_returns_exactly_cpu_and_ram() {
        let snapshot = collect();
        assert_eq!(snapshot.gauges.len(), 2, "SWAP 已去掉，只剩 CPU / RAM");
        assert_eq!(snapshot.gauges[0].label, "CPU");
        assert_eq!(snapshot.gauges[1].label, "RAM");
        for gauge in &snapshot.gauges {
            assert!(gauge.usage <= 100, "使用率必须夹在 0–100");
        }
    }
}
