//! /ocr 的参数解析。
//!
//! 接受的前缀与仓库其它插件一致（/、!、！、.、#），命令名是 ocr，也接受 rusto（插件名）。
//!
//! 用法：
//! - /ocr              识别（回复图片 / 同条图片 / 私聊直接发图且 auto=true）
//! - /ocr detail       识别并附行级置信度与坐标
//! - /ocr status       引擎状态（模型、并发、最近耗时）
//! - /ocr warm         预加载模型
//! - /ocr path <路径>  识别本机图片（排障用）
//! - /ocr help         用法

/// 命令前缀。
const PREFIXES: [char; 5] = ['/', '!', '！', '.', '#'];

/// 命令别名（ocr 是主名，rusto 是插件名兜底）。
const NAMES: [&str; 2] = ["ocr", "rusto"];

/// 解析出的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// 识别。
    Ocr {
        /// 是否要行级明细。
        detail: bool,
    },
    /// 引擎状态。
    Status,
    /// 预加载模型。
    Warm,
    /// 用法。
    Help,
    /// 识别本机图片。
    Path {
        /// 本机路径。
        path: String,
    },
}

/// 解析一条消息文本；不像本插件命令就返回 None。
pub fn parse(text: &str) -> Option<Command> {
    let text = text.trim();
    let mut chars = text.chars();
    let first = chars.next()?;
    if !PREFIXES.contains(&first) {
        return None;
    }
    let rest = chars.as_str().trim_start();
    let mut parts = rest.split_whitespace();
    let name = parts.next()?.to_lowercase();
    if !NAMES.contains(&name.as_str()) {
        return None;
    }
    let argument = parts.next().unwrap_or_default().to_lowercase();
    let tail = rest
        .splitn(3, char::is_whitespace)
        .nth(2)
        .unwrap_or_default()
        .trim()
        .to_owned();
    Some(match argument.as_str() {
        "" => Command::Ocr { detail: false },
        "detail" | "d" | "详细" => Command::Ocr { detail: true },
        "status" | "状态" => Command::Status,
        "warm" | "预热" => Command::Warm,
        "help" | "-h" | "--help" | "帮助" => Command::Help,
        "path" if !tail.is_empty() => Command::Path { path: tail },
        _ => Command::Ocr { detail: false },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_command_triggers_ocr() {
        assert_eq!(parse("/ocr"), Some(Command::Ocr { detail: false }));
        assert_eq!(parse("！ocr"), Some(Command::Ocr { detail: false }));
    }

    #[test]
    fn detail_and_status_switch() {
        assert_eq!(parse("/ocr detail"), Some(Command::Ocr { detail: true }));
        assert_eq!(parse("/ocr status"), Some(Command::Status));
        assert_eq!(parse("/ocr warm"), Some(Command::Warm));
    }

    #[test]
    fn path_keeps_the_rest_of_the_line() {
        assert_eq!(
            parse("/ocr path C:/tmp/a b.png"),
            Some(Command::Path {
                path: "C:/tmp/a b.png".to_owned()
            })
        );
    }

    #[test]
    fn other_commands_are_ignored() {
        assert_eq!(parse("/whois example.com"), None);
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("ocr"), None, "没有前缀不算命令");
    }
}
