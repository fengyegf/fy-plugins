//! 命令文本解析。
//!
//! 这里只做"文本 → 命令名 + 参数"的机械翻译，不认任何具体命令名。命令头由宿主
//! 按 `[command].prefix` 识别并统一归一化成 `/cmd`，分派留给 [`crate::cmd`]。
//!
//! 为什么按 `/` 命令头解析而不是按 Telegram 的 bot_command entity：宿主投给插件的是
//! 纯文本消息，用户手打 `/bidihelp` 和点菜单发出来的命令都要能认；统一处理还能让
//! 宿主配置的命令头改了立刻生效，不用改代码。
//!
//! 输入是外部内容，所以所有"空 / 超长 / 缺名字 / 引号没闭合"的情况都只返回 `None`
//! 或做宽容处理，绝不让一条坏消息把业务线程打挂。

/// 单条命令文本的字符上限。
///
/// 正常命令几十个字符就够；超过这个数只可能是广告或误触，直接不当命令——
/// 省掉整条长文本的分词开销，也避免把"以 / 开头的长广告"误当命令吞掉。
const MAX_COMMAND_CHARS: usize = 512;

/// 一条解析出来的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// 命令名：已去掉 `@botname` 后缀并转成小写。
    pub name: String,
    /// 命令名之后的参数；带引号的参数保留内部空白。
    pub args: Vec<String>,
}

/// 解析宿主归一化后的文本命令；不是 `/cmd` 返回 `None`。
pub fn parse(text: &str) -> Option<Command> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_COMMAND_CHARS {
        return None;
    }
    let rest = text.strip_prefix('/')?.trim_start();
    let mut tokens = tokenize(rest).into_iter();
    let raw = tokens.next()?;
    // `/bidihelp@fuyu_bot`：@ 之后是"喊哪个 bot"，不归本插件解释，截掉即可。
    let name = raw.split('@').next().unwrap_or_default().trim();
    if name.is_empty() {
        return None;
    }
    Some(Command {
        name: name.to_lowercase(),
        args: tokens.collect(),
    })
}

/// 把"命令头之后的部分"切成 token。
///
/// 分隔符是空白；token 可以用成对引号包起来，这样 `/bidisay 42 "你好 世界"`
/// 和 `/bidiword add 『加 微信』` 能带上空格。中英文四种引号都认——中文输入法下
/// 很容易打出「」『』，只认 ASCII 双引号会让用户反复改引号。
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        let Some(first) = chars.peek().copied() else {
            break;
        };
        let mut token = String::new();
        if let Some(close) = closing_of(first) {
            chars.next();
            // 引号没闭合也接受（读到结尾为止）：命令少写一个引号不该整条失效。
            for ch in chars.by_ref() {
                if ch == close {
                    break;
                }
                token.push(ch);
            }
            // 引号之后如果紧跟非空白字符（如 "a"b），一起算进同一个 token。
            while let Some(ch) = chars.peek().copied() {
                if ch.is_whitespace() {
                    break;
                }
                token.push(ch);
                chars.next();
            }
        } else {
            while let Some(ch) = chars.peek().copied() {
                if ch.is_whitespace() {
                    break;
                }
                token.push(ch);
                chars.next();
            }
        }
        // 空 token（如 `""`）直接丢掉：参数位上的空串没有意义，留着只会让
        // 后续的"必填参数"判断更绕。
        if !token.is_empty() {
            tokens.push(token);
        }
    }
    tokens
}

/// 引号 → 对应的闭合引号；不是开引号返回 `None`。
fn closing_of(open: char) -> Option<char> {
    match open {
        '"' => Some('"'),
        '\'' => Some('\''),
        '「' => Some('」'),
        '『' => Some('』'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_command() {
        let command = parse("/bidistatus").expect("应当识别为命令");
        assert_eq!(command.name, "bidistatus");
        assert!(command.args.is_empty());
    }

    #[test]
    fn lowercases_name_and_strips_bot_suffix() {
        let command = parse("/BIDIHelp@Fuyu_Bot").expect("应当识别为命令");
        assert_eq!(command.name, "bidihelp");
        assert!(command.args.is_empty());
    }

    #[test]
    fn quoted_arguments_keep_their_spaces() {
        let cases = [
            ("/bidisay 42 \"你好 世界\"", "你好 世界"),
            ("/bidisay 42 'hello world'", "hello world"),
            ("/bidisay 42 「中文 引号」", "中文 引号"),
            (
                "/bidisay 42 『带\"英文\"引号 的 字』",
                "带\"英文\"引号 的 字",
            ),
        ];
        for (text, expected) in cases {
            let command = parse(text).expect("应当识别为命令");
            assert_eq!(command.name, "bidisay", "{text}");
            assert_eq!(
                command.args,
                vec!["42".to_owned(), expected.to_owned()],
                "{text}"
            );
        }
    }

    #[test]
    fn unclosed_quote_is_tolerated() {
        let command = parse("/bidisay 42 \"没关引号").expect("应当识别为命令");
        assert_eq!(command.args, vec!["42".to_owned(), "没关引号".to_owned()]);
    }

    #[test]
    fn quoted_command_name_is_accepted() {
        let command = parse("/『bidilist』").expect("应当识别为命令");
        assert_eq!(command.name, "bidilist");
    }

    #[test]
    fn rejects_plain_text_and_bare_prefixes() {
        for text in [
            "",
            "   ",
            "你好",
            "你好 /bidihelp",
            "/",
            "/   ",
            "/@fuyu_bot",
        ] {
            assert!(parse(text).is_none(), "{text:?} 不该被当成命令");
        }
        for text in ["!bidihelp", "！bidihelp", ".bidihelp", "#bidihelp"] {
            assert!(parse(text).is_none(), "{text:?} 应等待宿主先归一化成 /cmd");
        }
    }

    #[test]
    fn rejects_overlong_text() {
        let text = format!("/bidihelp {}", "啊".repeat(MAX_COMMAND_CHARS));
        assert!(parse(&text).is_none());
    }

    #[test]
    fn extra_whitespace_between_prefix_and_name_is_ok() {
        let command = parse("/  bidihelp").expect("应当识别为命令");
        assert_eq!(command.name, "bidihelp");
    }
}
