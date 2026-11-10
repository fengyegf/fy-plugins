//! 进度板：**一条消息**里画多行进度条，由一个 ticker 线程统一刷新。
//!
//! 为什么是「一条」：Telegram 对同一条消息的高频 edit 会返回 `MESSAGE_NOT_MODIFIED`，
//! 多条消息各自刷新还会撞限流。所以这里所有任务共用一个消息 + 一个刷新线程，
//! 任务只往原子计数里累加，不发任何消息：
//!
//! ```text
//! 下载中
//!
//! game.part1.rar  [############        ]  60.0%
//! game.part2.rar  [#####               ]  25.0%
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::messenger::{Messenger, Sent};
use crate::util::fs::human_bytes;

/// 进度条宽度（字符数）。
const BAR_WIDTH: usize = 20;
/// 文件名最多占用的显示宽度，超出截断，保证进度条不被挤走。
const NAME_WIDTH: usize = 26;
/// ticker 的最小睡眠粒度：让 `finish` 不用陪着睡完整个周期。
const TICK_SLICE: Duration = Duration::from_millis(50);

/// 进度板上的一行。
struct Row {
    label: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
}

/// 一行任务的句柄（可克隆，交给下载 / 解压线程推进）。
#[derive(Clone)]
pub struct RowHandle {
    row: Arc<Row>,
}

impl RowHandle {
    /// 累加已处理字节数。
    pub fn add(&self, bytes: u64) {
        self.row.done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// 设置总量（未知时保持 0）。
    pub fn set_total(&self, bytes: u64) {
        self.row.total.store(bytes, Ordering::Relaxed);
    }

    /// 改名（真实文件名要到响应头回来才知道）。
    pub fn rename(&self, label: impl Into<String>) {
        *self
            .row
            .label
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = label.into();
    }

    /// 复用这一行做下一个任务：改名并清零。
    pub fn reset(&self, label: impl Into<String>) {
        self.rename(label);
        self.row.done.store(0, Ordering::Relaxed);
        self.row.total.store(0, Ordering::Relaxed);
    }

    /// 标记完成：已知总量时把进度条填满。
    pub fn complete(&self) {
        let total = self.row.total.load(Ordering::Relaxed);
        if total > 0 {
            self.row.done.store(total, Ordering::Relaxed);
        }
    }
}

/// 一块进度板。
pub struct ProgressBoard {
    board: Arc<Board>,
    thread: Option<JoinHandle<()>>,
}

struct Board {
    messenger: Messenger,
    message: Sent,
    title: Mutex<String>,
    rows: Mutex<Vec<Arc<Row>>>,
    interval: Duration,
    stop: AtomicBool,
}

impl ProgressBoard {
    /// 在 `message` 上开一块进度板，每 `interval` 刷新一次。
    pub fn start(
        messenger: Messenger,
        message: Sent,
        title: impl Into<String>,
        interval: Duration,
    ) -> Self {
        let board = Arc::new(Board {
            messenger,
            message,
            title: Mutex::new(title.into()),
            rows: Mutex::new(Vec::new()),
            interval,
            stop: AtomicBool::new(false),
        });

        let ticker = board.clone();
        let thread = std::thread::Builder::new()
            .name("ppxfy-progress".to_owned())
            .stack_size(128 * 1024)
            .spawn(move || {
                while wait_tick(&ticker) {
                    flush_board(&ticker);
                }
            })
            .ok();

        Self { board, thread }
    }

    /// 登记一行任务，返回可跨线程推进的句柄。
    pub fn row(&self, label: impl Into<String>) -> RowHandle {
        let row = Arc::new(Row {
            label: Mutex::new(label.into()),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
        });
        self.board
            .rows
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(row.clone());
        RowHandle { row }
    }

    /// 停止刷新；之后的最终文案由调用方自己编辑。
    pub fn finish(mut self) {
        self.stop_ticker();
    }

    fn stop_ticker(&mut self) {
        self.board.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ProgressBoard {
    fn drop(&mut self) {
        self.stop_ticker();
    }
}

/// tick 一次：把整块板画成一段文本并编辑消息。
///
/// 一行进展都没有时跳过——例如走 7z 兜底拿不到进度，此时屏幕上应保持上一条提示。
fn flush_board(board: &Board) {
    let rows = board.rows.lock().unwrap_or_else(|error| error.into_inner());
    if rows.is_empty() {
        return;
    }
    let snapshots: Vec<(String, u64, u64)> = rows.iter().map(|row| snapshot(row)).collect();
    drop(rows);

    let started = snapshots
        .iter()
        .any(|(_, done, total)| *done > 0 || *total > 0);
    if !started {
        return;
    }

    let title = board
        .title
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let lines: Vec<String> = snapshots
        .iter()
        .map(|(label, done, total)| render_line(label, *done, *total))
        .collect();

    board.messenger.edit_text(
        board.message.chat_id,
        board.message.message_id,
        &format!("{title}\n\n{}", lines.join("\n")),
    );
}

/// 睡满一个刷新周期；期间被要求停止就立刻返回 `false`。
fn wait_tick(board: &Board) -> bool {
    let deadline = Instant::now() + board.interval;
    while Instant::now() < deadline {
        if board.stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(TICK_SLICE);
    }
    !board.stop.load(Ordering::Relaxed)
}

fn snapshot(row: &Row) -> (String, u64, u64) {
    let label = row
        .label
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    (
        label,
        row.done.load(Ordering::Relaxed),
        row.total.load(Ordering::Relaxed),
    )
}

/// 一行文案：`名字  [####      ]  42.0%`；总量未知时只报已处理字节。
fn render_line(label: &str, done: u64, total: u64) -> String {
    let name = pad_name(label);
    if total == 0 {
        return format!("{name}  {}", human_bytes(done));
    }
    let ratio = (done as f64 / total as f64).clamp(0.0, 1.0);
    let filled = (ratio * BAR_WIDTH as f64).round() as usize;
    format!(
        "{name}  [{}{}] {:>5.1}%",
        "#".repeat(filled),
        " ".repeat(BAR_WIDTH.saturating_sub(filled)),
        ratio * 100.0
    )
}

/// 按**显示宽度**截断并补空格，让中英文混排时进度条也能对齐。
fn pad_name(label: &str) -> String {
    let chars: Vec<char> = label.chars().collect();
    let mut taken: Vec<char> = Vec::new();
    let mut width = 0usize;
    for ch in &chars {
        let step = if is_wide(*ch) { 2 } else { 1 };
        if width + step > NAME_WIDTH {
            break;
        }
        width += step;
        taken.push(*ch);
    }

    if taken.len() < chars.len() {
        // 被截断了：腾出一格放省略号。
        while let Some(last) = taken.pop() {
            width -= if is_wide(last) { 2 } else { 1 };
            if width < NAME_WIDTH {
                break;
            }
        }
        taken.push('…');
    }

    let name: String = taken.into_iter().collect();
    let padding = NAME_WIDTH.saturating_sub(display_width(&name));
    format!("{name}{}", " ".repeat(padding))
}

/// 粗略的显示宽度：CJK 与全角字符按 2 列算。
fn display_width(text: &str) -> usize {
    text.chars().map(|ch| if is_wide(ch) { 2 } else { 1 }).sum()
}

fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x20000..=0x3FFFD
    )
}
