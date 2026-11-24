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
//!
//! 磁盘**跨平台**采集：Windows 用盘符（`C:\` → 「磁盘C」），Linux / macOS / Android
//! 用挂载点（`/` → 「根目录」，Android 的 `/data`、`/storage/emulated/0` →
//! 「内部存储」）。伪文件系统（proc / sysfs / tmpfs …）与 Android 的只读系统镜像
//! 分区会被过滤掉；同一文件系统被挂载多次时只留最有代表性的那一个。

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
    /// 磁盘占用（按重要性排序，最多 [`MAX_DISKS`] 块）。
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

/// 一块待排序 / 去重的候选磁盘。
struct Candidate {
    /// 卡片上直接显示的标签（「磁盘C」/「根目录」/「内部存储」…）。
    label: String,
    /// 挂载点原文，用于排序、去重与调试日志。
    mount: String,
    /// 设备名（Linux `/proc/mounts` 第一列）；Windows 上是卷 GUID。
    device: String,
    total: u64,
    available: u64,
    /// 越小越靠前。
    rank: u8,
}

/// 从 sysinfo 抄下来的一条原始挂载记录（与平台无关，便于单测）。
struct RawDisk {
    mount: String,
    device: String,
    fs: String,
    total: u64,
    available: u64,
}

/// 采集磁盘：只留「容量 > 0、不是伪文件系统」的卷，按重要性排序后最多取 4 块。
fn collect_disks() -> Vec<DiskUsage> {
    let disks = Disks::new_with_refreshed_list();
    let raw: Vec<RawDisk> = disks
        .list()
        .iter()
        .map(|disk| RawDisk {
            mount: disk.mount_point().to_string_lossy().into_owned(),
            device: disk.name().to_string_lossy().into_owned(),
            fs: disk.file_system().to_string_lossy().into_owned(),
            total: disk.total_space(),
            available: disk.available_space(),
        })
        .collect();
    build_disks(raw, unix_flavor())
}

/// 过滤 → 排序 → 去重 → 取前 [`MAX_DISKS`] 块 → 格式化。
///
/// 抽成纯函数是为了能在任意平台上用假挂载表验证 Linux / Android 的规则。
fn build_disks(raw: Vec<RawDisk>, flavor: UnixFlavor) -> Vec<DiskUsage> {
    let mut candidates: Vec<Candidate> = raw
        .into_iter()
        .filter(|disk| disk.total > 0 && keeps_mount(&disk.mount, &disk.fs))
        .map(|disk| Candidate {
            label: disk_label_text(flavor, &disk.mount),
            rank: mount_rank(flavor, &disk.mount),
            available: disk.available.min(disk.total),
            mount: disk.mount,
            device: disk.device,
            total: disk.total,
        })
        .collect();

    // 先按「重要性 + 挂载点」排序，再去重：同一文件系统的代表挂载点先出现、被保留。
    candidates.sort_by(|a, b| (a.rank, &a.mount).cmp(&(b.rank, &b.mount)));
    let mut kept: Vec<Candidate> = Vec::with_capacity(MAX_DISKS);
    for candidate in candidates {
        if kept
            .iter()
            .any(|existing| same_volume(existing, &candidate))
        {
            continue;
        }
        kept.push(candidate);
        if kept.len() == MAX_DISKS {
            break;
        }
    }

    kept.into_iter()
        .map(|candidate| {
            let used = candidate.total - candidate.available;
            DiskUsage {
                label: candidate.label,
                used: format_gb(used),
                total: format_gb(candidate.total),
                usage_percent: percent(used, candidate.total),
            }
        })
        .collect()
}

/// 两个挂载点是否其实是同一块文件系统。
///
/// 两种判据：
/// - 设备名与容量都相同（同一个设备被 bind mount 到多处）；
/// - Android 的 `/data` 与 `/storage/emulated/0`（`/sdcard`）：同一块用户数据分区，
///   设备名一个是 `/dev/block/...`、一个是 `/dev/fuse`，但空间数字完全一致。
///
/// 不按「空间数字相同」无条件合并，避免把两块同型号、同样空的硬盘认成一块。
fn same_volume(a: &Candidate, b: &Candidate) -> bool {
    if !a.device.is_empty() && a.device == b.device && a.total == b.total {
        return true;
    }
    a.total == b.total
        && a.available == b.available
        && ((a.mount == "/data" && is_internal_storage(&b.mount))
            || (b.mount == "/data" && is_internal_storage(&a.mount)))
}

/// Android 的「内部存储」视图（模拟存储 / `/sdcard`）。
fn is_internal_storage(mount: &str) -> bool {
    mount.starts_with("/storage/emulated")
        || mount.starts_with("/sdcard")
        || mount.starts_with("/mnt/user")
}

/// 伪文件系统：容量可能非 0，但和「磁盘还剩多少」无关。
fn is_pseudo_fs(fs: &str) -> bool {
    matches!(
        fs,
        "proc"
            | "sysfs"
            | "devtmpfs"
            | "devpts"
            | "cgroup"
            | "cgroup2"
            | "pstore"
            | "squashfs"
            | "ramfs"
            | "rootfs"
            | "tmpfs"
            | "mqueue"
            | "hugetlbfs"
            | "securityfs"
            | "debugfs"
            | "tracefs"
            | "configfs"
            | "binfmt_misc"
            | "autofs"
            | "efivarfs"
            | "bpf"
            | "nsfs"
            | "rpc_pipefs"
            | "fusectl"
    )
}

/// 不该出现在卡片上的挂载点：运行时目录、发行版快照，以及 Android 的只读系统镜像。
fn keeps_mount(mount: &str, fs: &str) -> bool {
    if is_pseudo_fs(fs) {
        return false;
    }
    if mount.is_empty() || mount == "/" {
        return true;
    }
    // `/run/media` 是发行版挂 U 盘的位置，其它 `/run` 下都是运行时文件系统。
    if mount == "/run/media" || mount.starts_with("/run/media/") {
        return true;
    }
    // macOS 的系统辅助卷（Preboot / VM / Update …）不是用户关心的存储，
    // 真正的用户数据在 `/System/Volumes/Data`。
    if mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data" {
        return false;
    }
    // Android 的 `/vendor`、`/product`、`/odm` 等是只读系统镜像，用户并不关心。
    const HIDDEN: &[&str] = &[
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/snap",
        "/vendor",
        "/product",
        "/odm",
        "/system_ext",
        "/apex",
        "/metadata",
        "/linkerconfig",
        "/bootstrap-apex",
        "/debug_ramdisk",
        "/mnt/vendor",
        "/mnt/installer",
        "/mnt/pass_through",
        "/mnt/product",
        "/mnt/odm",
        "/mnt/system",
        "/mnt/apex",
    ];
    !HIDDEN
        .iter()
        .any(|hidden| mount == *hidden || mount.starts_with(&format!("{hidden}/")))
}

/// 排序用：数字越小越值得出现在卡片上。
fn mount_rank(flavor: UnixFlavor, mount: &str) -> u8 {
    if windows_drive_letter(mount).is_some() {
        return 0;
    }
    unix_rank(flavor, mount)
}

/// Unix 侧的排序规则（抽成纯函数，任何平台上都能单测）。
fn unix_rank(flavor: UnixFlavor, mount: &str) -> u8 {
    match flavor {
        UnixFlavor::Android => {
            if mount == "/data" {
                0
            } else if is_internal_storage(mount) {
                1
            } else if mount == "/" {
                2
            } else if mount.starts_with("/storage/")
                || mount.starts_with("/mnt/media_rw")
                || mount.starts_with("/media/")
            {
                3
            } else {
                4
            }
        }
        UnixFlavor::MacOs => {
            if mount == "/System/Volumes/Data" {
                0
            } else if mount.starts_with("/Volumes/") {
                1
            } else if mount == "/" {
                2
            } else {
                3
            }
        }
        UnixFlavor::Linux => {
            if mount == "/" {
                0
            } else if mount == "/home" {
                1
            } else if mount.starts_with("/mnt/")
                || mount.starts_with("/media/")
                || mount.starts_with("/storage/")
                || mount.starts_with("/run/media/")
            {
                2
            } else {
                3
            }
        }
    }
}

/// 当前 Unix 分支（Android / macOS / 其它）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnixFlavor {
    Android,
    MacOs,
    Linux,
}

fn unix_flavor() -> UnixFlavor {
    if cfg!(target_os = "android") {
        UnixFlavor::Android
    } else if cfg!(target_os = "macos") {
        UnixFlavor::MacOs
    } else {
        UnixFlavor::Linux
    }
}

/// 卡片上的磁盘标签：Windows 是「磁盘C」，其余平台按挂载点给出可读名字。
fn disk_label_text(flavor: UnixFlavor, mount: &str) -> String {
    match windows_drive_letter(mount) {
        Some(letter) => format!("磁盘{letter}"),
        None => unix_disk_label(flavor, mount),
    }
}

/// Windows 的卷挂载点形如盘符加冒号；只有「字母 + 冒号」才算盘符。
fn windows_drive_letter(mount: &str) -> Option<char> {
    let mut chars = mount.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    (letter.is_ascii_alphabetic() && chars.next() == Some(':')).then_some(letter)
}

/// Unix 侧的显示标签（纯函数，便于任何平台单测）。
fn unix_disk_label(flavor: UnixFlavor, mount: &str) -> String {
    match flavor {
        UnixFlavor::Android => {
            if mount == "/" {
                "系统分区".to_owned()
            } else if mount == "/data" || is_internal_storage(mount) {
                "内部存储".to_owned()
            } else if mount.starts_with("/storage/")
                || mount.starts_with("/mnt/media_rw")
                || mount.starts_with("/media/")
            {
                "外部存储".to_owned()
            } else {
                mount.to_owned()
            }
        }
        UnixFlavor::MacOs => {
            if mount == "/System/Volumes/Data" {
                "系统磁盘".to_owned()
            } else if mount == "/" {
                "系统卷".to_owned()
            } else if let Some(name) = mount.strip_prefix("/Volumes/") {
                name.to_owned()
            } else {
                mount.to_owned()
            }
        }
        UnixFlavor::Linux => {
            if mount == "/" {
                "根目录".to_owned()
            } else if mount == "/home" {
                "用户目录".to_owned()
            } else {
                mount.to_owned()
            }
        }
    }
}

/// 采集一次、格式化后返回「耗时多少毫秒」，给命令层打日志用。
pub fn collect_timed() -> (Snapshot, u128) {
    let started = Instant::now();
    let snapshot = collect();
    (snapshot, started.elapsed().as_millis())
}
