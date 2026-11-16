//! 命令解析。
//!
//! 插件命令以普通文本消息进入宿主，因此前缀与名称统一在这里解析：支持配置里的
//! 多个前缀（缺省 `/ ! ！ . #`）和 `@机器人名` 后缀。

/// 一条解析出来的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// 命令名（保留原大小写，匹配时统一转小写）。
    pub name: String,
    /// 命令名之后的参数，按空白切分。
    pub args: Vec<String>,
}

/// 解析文本命令；不是命令（没有前缀）返回 `None`。
pub fn parse(text: &str, prefixes: &[String]) -> Option<Command> {
    let text = text.trim();
    let prefix = prefixes
        .iter()
        .filter(|prefix| !prefix.is_empty())
        .find(|prefix| text.starts_with(prefix.as_str()))?;

    let mut parts = text[prefix.len()..].split_whitespace();
    let name = parts.next()?.split('@').next()?.to_owned();
    if name.is_empty() {
        return None;
    }
    Some(Command {
        name,
        args: parts.map(str::to_owned).collect(),
    })
}
