//! 解压后的目录树交互：编号选择 → 删除，或选中一个目录进入打包。
//!
//! 交互完全靠「回复那条提示消息」：编号切换选中，`OK` 删除选中项，`ZIP` 打包选中目录，
//! `CANCEL` 结束。每次操作后重绘同一棵编号树（编号会变，所以每次重建）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::host::messenger::Sent;
use crate::host::reply::Wait;
use crate::host::task::Task;
use crate::host::{Ctx, clip};
use crate::util::fs as fsutil;

/// 目录树最大展示层数。
const TREE_DEPTH: usize = 3;
/// 单层最多展示的条目数。
const MAX_ENTRIES: usize = 50;
/// 超过该条目数时自动降到 2 层，避免消息过长。
const DENSE_ENTRIES: usize = 20;
/// 消息长度上限（Telegram 单条 4096）。
const MAX_TEXT: usize = 3900;

const HINT: &str = "编号说明：回复编号切换选中（如 1 1.2 2.3），回复 OK 删除选中项，回复 ZIP 打包选中目录，回复 CANCEL 结束。";

/// 树上的一个条目。
struct Entry {
    /// 层级编号，如 `2.3.1`。
    index: String,
    /// 缩进层级（0 开始）。
    depth: usize,
    /// 展示文本（目录带 `/`）。
    label: String,
    path: PathBuf,
}

/// 展示目录树并处理用户操作；用户选择 `ZIP` 时返回待打包的目录。
pub fn run(ctx: &Arc<Ctx>, task: &Arc<Task>, tips: Sent, output_dir: &Path) -> Option<PathBuf> {
    let mut entries = build_entries(output_dir);
    let mut selected: HashSet<String> = HashSet::new();
    let mut hint = HINT.to_owned();

    loop {
        let text = render(&entries, &selected, &hint);
        ctx.messenger
            .edit_text(tips.chat_id, tips.message_id, &text);

        let reply = match ctx.wait_reply(
            task,
            tips.chat_id,
            tips.message_id,
            ctx.config.listen_timeout,
        ) {
            Wait::Reply(text) => text,
            Wait::Timeout => {
                ctx.messenger.edit_text(
                    tips.chat_id,
                    tips.message_id,
                    "等待操作超时，已结束文件操作。",
                );
                return None;
            }
            Wait::Cancelled => {
                ctx.messenger
                    .edit_text(tips.chat_id, tips.message_id, "已取消文件操作。");
                return None;
            }
            Wait::Shutdown => return None,
        };

        match reply.trim().to_uppercase().as_str() {
            "CANCEL" => {
                ctx.messenger
                    .edit_text(tips.chat_id, tips.message_id, "已结束文件操作。");
                return None;
            }
            "ZIP" => {
                let targets = selected_paths(&entries, &selected);
                match targets.len() {
                    0 => {
                        hint = "请先回复编号选择要打包的目录，再回复 ZIP 进入打包流程。".to_owned();
                    }
                    1 => {
                        let target = targets.into_iter().next().expect("长度已判断");
                        if target.is_dir() {
                            return Some(target);
                        }
                        hint = "仅支持打包目录，请重新选择后回复 ZIP。".to_owned();
                    }
                    _ => {
                        hint = "打包一次仅支持选择 1 个目录，请只选择一项后回复 ZIP。".to_owned();
                    }
                }
            }
            "OK" => {
                let targets = selected_paths(&entries, &selected);
                if targets.is_empty() {
                    hint = "未选择任何项。请回复编号选择后回复 OK 删除，回复 ZIP 打包选中目录，或回复 CANCEL 结束。"
                        .to_owned();
                    continue;
                }
                let report = fsutil::remove_all(&targets);
                ctx.info(&format!(
                    "ppxfy: 删除完成，成功 {} 项，失败 {} 项",
                    report.removed.len(),
                    report.failed.len()
                ));

                entries = build_entries(output_dir);
                selected.clear();
                hint = if report.failed.is_empty() {
                    format!("已删除 {} 项，{}", report.removed.len(), HINT)
                } else {
                    format!(
                        "已删除 {} 项，{} 项删除失败，{}",
                        report.removed.len(),
                        report.failed.len(),
                        HINT
                    )
                };
            }
            other => {
                let mut changed = false;
                for part in other
                    .split([' ', ',', '，'])
                    .filter(|part| !part.is_empty())
                {
                    if !is_index(part) {
                        continue;
                    }
                    if !entries.iter().any(|entry| entry.index == part) {
                        continue;
                    }
                    if !selected.remove(part) {
                        selected.insert(part.to_owned());
                    }
                    changed = true;
                }
                hint = if changed {
                    HINT.to_owned()
                } else {
                    "未识别的编号，请回复有效编号（如 1 1.2），或回复 OK / ZIP / CANCEL。"
                        .to_owned()
                };
            }
        }
    }
}

/// 选中项对应的真实路径。
fn selected_paths(entries: &[Entry], selected: &HashSet<String>) -> Vec<PathBuf> {
    entries
        .iter()
        .filter(|entry| selected.contains(&entry.index))
        .map(|entry| entry.path.clone())
        .collect()
}

/// 构建带层级编号的目录树。
fn build_entries(root: &Path) -> Vec<Entry> {
    fn walk(dir: &Path, current: usize, prefix: &str, out: &mut Vec<Entry>) {
        let items = fsutil::list_entries(dir);
        if items.is_empty() {
            return;
        }
        let max_depth = if items.len() > DENSE_ENTRIES {
            TREE_DEPTH.min(2)
        } else {
            TREE_DEPTH
        };

        for (position, (name, is_dir)) in items.into_iter().take(MAX_ENTRIES).enumerate() {
            let index = if prefix.is_empty() {
                (position + 1).to_string()
            } else {
                format!("{prefix}.{}", position + 1)
            };
            let path = dir.join(&name);
            out.push(Entry {
                index: index.clone(),
                depth: current,
                label: if is_dir { format!("{name}/") } else { name },
                path: path.clone(),
            });
            if is_dir && current + 1 < max_depth {
                walk(&path, current + 1, &index, out);
            }
        }
    }

    let mut entries = Vec::new();
    walk(root, 0, "", &mut entries);
    entries
}

/// 渲染：`{缩进}{编号} {选中标记}  {名称}`。
fn render(entries: &[Entry], selected: &HashSet<String>, hint: &str) -> String {
    let tree = if entries.is_empty() {
        "（当前目录已为空）".to_owned()
    } else {
        entries
            .iter()
            .map(|entry| {
                let mark = if selected.contains(&entry.index) {
                    'x'
                } else {
                    ' '
                };
                format!(
                    "{}{} {mark}  {}",
                    "  ".repeat(entry.depth),
                    entry.index,
                    entry.label
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    clip(
        format!("解压完成，文件结构（仅显示前 {TREE_DEPTH} 层）：\n\n{hint}\n\n```\n{tree}\n```"),
        MAX_TEXT,
    )
}

/// 是否是合法编号：`1` / `1.2` / `1.2.3`。
fn is_index(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}
