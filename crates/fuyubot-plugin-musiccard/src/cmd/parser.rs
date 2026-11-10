//! 命令解析。
//!
//! 插件接收普通文本消息，因此在这里解析前缀与命令名：支持配置里的多个前缀
//! （缺省 `/ ! ！ . #`）以及 `@机器人名` 后缀。
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

/// 解析文本命令；不是命令（没有前缀）返回 `None`。
///
/// 例：`"/musiccard 晴天"` → `Command { name: "musiccard", args: ["晴天"] }`，
/// `"/musiccard@FuyuBot 347230"` 的 `@FuyuBot` 会被剥掉。
pub fn parse(text: &str, prefixes: &[String]) -> Option<Command> {
    let text = text.trim();
    let prefix = prefixes
        .iter()
        .filter(|prefix| !prefix.is_empty())
        .find(|prefix| text.starts_with(prefix.as_str()))?;

    // 前缀之后可能直接跟命令名，也可能先跟空白；两者都从第一个非空白 token 起算。
    let mut parts = text[prefix.len()..].split_whitespace();
    // `@botname` 后缀剥掉：只认前缀后面的命令名本身。
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
