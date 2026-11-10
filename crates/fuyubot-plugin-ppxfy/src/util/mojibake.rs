//! 乱码文件名修复。
//!
//! 场景：RAR4 这类没有 Unicode 名字字段的压缩包，文件名是「本地代码页」的裸字节
//! （日文游戏就是 Shift-JIS）。7-Zip 在 Windows 上用**系统 OEM 代码页**去解，
//! 若服务器「非 Unicode 程序的语言」是英文，OEM 代码页就是 CP437，
//! 于是日文名被逐字节映射成 `é╖é╞ü`…` 这种 DOS 制表符乱码。
//!
//! 修复是这条链的逆运算：把名字按 CP437 还原成字节 → 用 Shift-JIS / GBK 重解。
//! 只有「全部字符都在 CP437 高位表里」且「重解无错、结果像正常文字」时才动手，
//! 已经正常的名字编码不回 CP437，天然不会被误改。

use std::fs;
use std::path::Path;

use encoding_rs::{GBK, SHIFT_JIS};

use super::fs as fsutil;

/// CP437 的 `0x80..=0xFF` 段（下标 0 对应字节 0x80）。
const CP437_HIGH: [char; 128] = [
    '\u{00c7}', '\u{00fc}', '\u{00e9}', '\u{00e2}', '\u{00e4}', '\u{00e0}', '\u{00e5}', '\u{00e7}',
    '\u{00ea}', '\u{00eb}', '\u{00e8}', '\u{00ef}', '\u{00ee}', '\u{00ec}', '\u{00c4}', '\u{00c5}',
    '\u{00c9}', '\u{00e6}', '\u{00c6}', '\u{00f4}', '\u{00f6}', '\u{00f2}', '\u{00fb}', '\u{00f9}',
    '\u{00ff}', '\u{00d6}', '\u{00dc}', '\u{00a2}', '\u{00a3}', '\u{00a5}', '\u{20a7}', '\u{0192}',
    '\u{00e1}', '\u{00ed}', '\u{00f3}', '\u{00fa}', '\u{00f1}', '\u{00d1}', '\u{00aa}', '\u{00ba}',
    '\u{00bf}', '\u{2310}', '\u{00ac}', '\u{00bd}', '\u{00bc}', '\u{00a1}', '\u{00ab}', '\u{00bb}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255d}', '\u{255c}', '\u{255b}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252c}', '\u{251c}', '\u{2500}', '\u{253c}', '\u{255e}', '\u{255f}',
    '\u{255a}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256c}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256b}',
    '\u{256a}', '\u{2518}', '\u{250c}', '\u{2588}', '\u{2584}', '\u{258c}', '\u{2590}', '\u{2580}',
    '\u{03b1}', '\u{00df}', '\u{0393}', '\u{03c0}', '\u{03a3}', '\u{03c3}', '\u{00b5}', '\u{03c4}',
    '\u{03a6}', '\u{0398}', '\u{03a9}', '\u{03b4}', '\u{221e}', '\u{03c6}', '\u{03b5}', '\u{2229}',
    '\u{2261}', '\u{00b1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00f7}', '\u{2248}',
    '\u{00b0}', '\u{2219}', '\u{00b7}', '\u{221a}', '\u{207f}', '\u{00b2}', '\u{25a0}', '\u{00a0}',
];

/// 递归修复 `root` 下的乱码文件名，返回改名个数。
///
/// 自底向上处理：先修子项，再修父目录，避免父目录改名后子路径失效。
pub fn repair_tree(root: &Path) -> usize {
    let entries = fsutil::list_entries(root);

    let mut repaired = 0;
    for (name, is_dir) in &entries {
        if *is_dir {
            repaired += repair_tree(&root.join(name));
        }
    }

    for (name, _) in &entries {
        let Some(fixed) = repair_name(name) else {
            continue;
        };
        let from = root.join(name);
        let to = root.join(&fixed);
        // 目标已存在就不动，宁可留着乱码也不覆盖别人的文件。
        if to.exists() {
            continue;
        }
        if fs::rename(&from, &to).is_ok() {
            repaired += 1;
        }
    }
    repaired
}

/// 尝试把一个疑似乱码的文件名还原；不是乱码返回 `None`。
pub fn repair_name(name: &str) -> Option<String> {
    if name.is_ascii() {
        return None;
    }
    let bytes = cp437_bytes(name)?;

    // 日文游戏最常见，其次简体中文；两者的裸字节都能通过 CP437 被错误显示。
    for encoding in [SHIFT_JIS, GBK] {
        let (decoded, _, had_errors) = encoding.decode(&bytes);
        if had_errors {
            continue;
        }
        let decoded = decoded.trim_end_matches(['.', ' ']).trim();
        if decoded.is_empty() || decoded == name {
            continue;
        }
        if looks_like_text(decoded) {
            return Some(decoded.to_owned());
        }
    }
    None
}

/// 名字 → CP437 字节；有任何字符不在 CP437 里就返回 `None`。
fn cp437_bytes(name: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii() {
            bytes.push(ch as u8);
            continue;
        }
        let index = CP437_HIGH.iter().position(|high| *high == ch)?;
        bytes.push(0x80 + index as u8);
    }
    Some(bytes)
}

/// 解码结果是否像正常文件名：含假名/汉字，且不含 CP437 的制表符与方块。
fn looks_like_text(text: &str) -> bool {
    let mut has_cjk = false;
    for ch in text.chars() {
        match ch as u32 {
            // DOS 制表符 / 方块：正常文件名不会出现，出现说明还原得不对。
            0x2500..=0x259F => return false,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xFF66..=0xFF9F => has_cjk = true,
            _ => {}
        }
    }
    has_cjk
}
