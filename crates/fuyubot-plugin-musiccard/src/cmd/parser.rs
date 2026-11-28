//! 命令解析。
//!
//! 宿主已按 `[command].prefix` 识别命令头，并统一归一化成 `/name`；这里只拆分
//! 命令名与参数，并剥掉群聊里的 `@机器人名` 后缀。
//!
//! 解析只负责「把这一句拆成命令名 + 参数」，**大小写与命令白名单**在 [`crate::cmd::plan`]
//! 里判断——这样其它插件共用同一套文本时不会互相抢消息。

/// 一条解析出来的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// 命令名（保留原大小写，匹配时统一转小写）。
    pub name: String,
    /// 命令名之后的参数，按空白切分。
    pub args: Vec<String>,
}

/// 解析宿主归一化后的文本命令；不是 `/cmd` 返回 `None`。
///
/// 例：`"/musiccard 晴天"` → `Command { name: "musiccard", args: ["晴天"] }`，
/// `"/musiccard@FuyuBot 347230"` 的 `@FuyuBot` 会被剥掉。
pub fn parse(text: &str) -> Option<Command> {
    let mut parts = text.trim().strip_prefix('/')?.split_whitespace();
    // `@botname` 后缀剥掉：只认命令头后面的命令名本身。
    let name = parts
        .next()?
        .split('@')
        .next()
        .unwrap_or_default()
        .to_owned();
    if name.is_empty() {
        return None;
    }
    Some(Command {
        name,
        args: parts.map(str::to_owned).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normalized_command_with_args() {
        let command = parse("/musiccard@FuyuBot 347230").expect("应当识别为命令");
        assert_eq!(command.name, "musiccard");
        assert_eq!(command.args, vec!["347230".to_owned()]);
    }

    #[test]
    fn only_accepts_host_normalized_slash_prefix() {
        assert!(parse("！musiccard 晴天").is_none());
        assert!(parse("musiccard 晴天").is_none());
    }
}
