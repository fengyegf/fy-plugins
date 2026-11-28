//! 命令解析。
//!
//! 宿主已按 `[command].prefix` 识别命令头，并统一归一化成 `/name`；这里只拆分
//! 命令名与参数，并剥掉群聊里的 `@机器人名` 后缀。

/// 一条解析出来的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// 命令名（保留原大小写，匹配时统一转小写）。
    pub name: String,
    /// 命令名之后的参数，按空白切分。
    pub args: Vec<String>,
}

/// 解析宿主归一化后的文本命令；不是 `/cmd` 返回 `None`。
pub fn parse(text: &str) -> Option<Command> {
    let mut parts = text.trim().strip_prefix('/')?.split_whitespace();
    let name = parts.next()?.split('@').next()?.to_owned();
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
        let command = parse("/trending@fuyu_bot weekly").expect("应当识别为命令");
        assert_eq!(command.name, "trending");
        assert_eq!(command.args, vec!["weekly".to_owned()]);
    }

    #[test]
    fn only_accepts_host_normalized_slash_prefix() {
        assert!(parse("！trending").is_none());
        assert!(parse("trending").is_none());
    }
}
