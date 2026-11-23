//! LaTeX / 富消息公式适配。
//!
//! 官方 Rich Markdown（Bot API「Rich Message Formatting Options → Rich Markdown style」）里
//! 公式有两种写法：
//! - 行内：$x^2 + y^2$
//! - 块级：独立一行 $$E = mc^2$$，或围栏 ```math 代码块
//!
//! 但实测宿主 rich_from_markdown + TDLib 1.8.70：
//! - 行内 $...$ → richTextMathematicalExpression；
//! - 块级必须是独立 $$ 行 → inputPageBlockMathematicalExpression；
//! - 单行 $$...$$ 夹在句子里会被当成两个行内 $（多出裸 $）；
//! - 反斜杠括号 \\(...\\) / \\[...\\] 不被识别；
//! - 围栏 ```math 可以，但 ```latex 被当成普通代码块。
//!
//! 所以这里统一归一：
//! 1. \\(...\\) → 行内 $...$；
//! 2. \\[...\\] → 独立 $$ 块；
//! 3. 单行 $$...$$ → 独立 $$ 块；
//! 4. 围栏 math / latex / tex / katex / stex → 独立 $$ 块（去掉围栏）；
//! 5. align / align* / eqnarray / gather / equation 等环境 → KaTeX 友好的等价环境；
//! 6. 普通代码围栏（含裸 $）原样保留，不参与替换，也不影响「公式是否闭合」判断。
//!
//! 另外提供 math_open：流式时草稿可能停在半个公式 / 未闭合的公式围栏里，
//! 调用方据此跳过这一帧，避免把半截公式发给客户端。

/// 输入切段：普通文本 / 普通代码围栏 / 公式围栏。
enum Seg<'a> {
    Text(&'a str),
    Code(&'a str),
    Formula(&'a str),
    /// 还没闭合的公式围栏：流式时视为「公式未完成」。
    FormulaOpen(&'a str),
}

/// 把 LaTeX / 富消息公式写法归一成宿主能解析的 Markdown。
pub fn normalize_math(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len() + 32);
    for segment in split_segments(markdown) {
        match segment {
            Seg::Text(text) => out.push_str(&normalize_segment(text)),
            Seg::Formula(expr) | Seg::FormulaOpen(expr) => {
                out.push_str("\n\n$$\n");
                out.push_str(&normalize_env(expr));
                out.push_str("\n$$\n\n");
            }
            Seg::Code(code) => out.push_str(code),
        }
    }
    out
}

/// 当前文本是否停在未闭合的公式里（流式草稿用）。
pub fn math_open(markdown: &str) -> bool {
    split_segments(markdown)
        .into_iter()
        .any(|segment| match segment {
            Seg::Text(text) => unclosed_dollar(&normalize_segment(text)),
            Seg::FormulaOpen(_) => true,
            Seg::Formula(_) | Seg::Code(_) => false,
        })
}

/// 按代码围栏切段；围栏带 math 类语言标记时按公式段处理。
fn split_segments(input: &str) -> Vec<Seg<'_>> {
    let mut segments = Vec::new();
    let mut cursor = 0usize;
    while cursor < input.len() {
        let Some(offset) = input[cursor..].find("\u{60}\u{60}\u{60}") else {
            segments.push(Seg::Text(&input[cursor..]));
            break;
        };
        let fence_start = cursor + offset;
        if fence_start > cursor {
            segments.push(Seg::Text(&input[cursor..fence_start]));
        }
        let Some(newline) = input[fence_start..].find('\n') else {
            // 只有围栏头没有换行：当普通代码处理。
            segments.push(Seg::Code(&input[fence_start..]));
            break;
        };
        let info_end = fence_start + newline + 1;
        let info = input[fence_start + 3..info_end - 1]
            .trim()
            .to_ascii_lowercase();
        let is_math = matches!(info.as_str(), "math" | "latex" | "tex" | "katex" | "stex");
        match input[info_end..].find("\u{60}\u{60}\u{60}") {
            Some(close_offset) => {
                let body_end = info_end + close_offset;
                let block_end = body_end + 3;
                if is_math {
                    segments.push(Seg::Formula(&input[info_end..body_end]));
                } else {
                    segments.push(Seg::Code(&input[fence_start..block_end]));
                }
                cursor = block_end;
            }
            None => {
                if is_math {
                    segments.push(Seg::FormulaOpen(&input[info_end..]));
                } else {
                    segments.push(Seg::Code(&input[fence_start..]));
                }
                break;
            }
        }
    }
    segments
}

/// 非代码段的替换。
fn normalize_segment(segment: &str) -> String {
    // 1) 反斜杠方括号 → 独立块公式。
    let segment = replace_between(segment, r"\[", r"\]", |inner| {
        format!("\n\n$$\n{}\n$$\n\n", normalize_env(inner))
    });
    // 2) 反斜杠圆括号 → 行内公式。
    let segment = replace_between(&segment, r"\(", r"\)", |inner| {
        format!("{}{}{}", '$', inner.trim(), '$')
    });
    // 3) 单行 $$...$$ → 独立块公式（多行的已经是块，不再动）。
    replace_between(&segment, "$$", "$$", |inner| {
        if inner.contains('\n') {
            format!("{}{}{}", "$$", inner, "$$")
        } else {
            format!("\n\n$$\n{}\n$$\n\n", normalize_env(inner))
        }
    })
}

/// 把 TeX 环境换成 Telegram/KaTeX 更稳的等价写法。
fn normalize_env(expr: &str) -> String {
    let mut out = expr.trim().to_owned();
    for (from, to) in [
        (r"\begin{align*}", r"\begin{aligned}"),
        (r"\end{align*}", r"\end{aligned}"),
        (r"\begin{align}", r"\begin{aligned}"),
        (r"\end{align}", r"\end{aligned}"),
        (r"\begin{eqnarray*}", r"\begin{aligned}"),
        (r"\end{eqnarray*}", r"\end{aligned}"),
        (r"\begin{eqnarray}", r"\begin{aligned}"),
        (r"\end{eqnarray}", r"\end{aligned}"),
        (r"\begin{gather*}", r"\begin{gathered}"),
        (r"\end{gather*}", r"\end{gathered}"),
        (r"\begin{gather}", r"\begin{gathered}"),
        (r"\end{gather}", r"\end{gathered}"),
    ] {
        out = out.replace(from, to);
    }
    for env in ["equation", "equation*", "displaymath"] {
        out = out
            .replace(&format!("\\begin{{{env}}}"), "")
            .replace(&format!("\\end{{{env}}}"), "");
    }
    out.trim().to_owned()
}

/// 把 open..close 之间的内容交给 render 替换；找不到收尾就原样保留。
fn replace_between(
    segment: &str,
    open: &str,
    close: &str,
    render: impl Fn(&str) -> String,
) -> String {
    let mut out = String::new();
    let mut rest = segment;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        match after.find(close) {
            Some(end) => {
                out.push_str(&render(&after[..end]));
                rest = &after[end + close.len()..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 归一后的文本里是否还有没闭合的 $ / $$。
fn unclosed_dollar(markdown: &str) -> bool {
    let mut inline = false;
    let mut block = false;
    let mut chars = markdown.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
            continue;
        }
        if c != '$' {
            continue;
        }
        if !inline && chars.peek() == Some(&'$') {
            chars.next();
            block = !block;
        } else if !block {
            inline = !inline;
        }
    }
    inline || block
}
