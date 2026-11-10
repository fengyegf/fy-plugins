//! 域名清洗：把用户输入（可能是完整 URL）压成 whois 接口要的裸域名。
//!
//! 清洗规则覆盖用户输入里最常见的几类噪声：
//!
//! - 大小写归一（`WWW.Example.COM` → `example.com`）；
//! - 去掉末尾的点（`example.com.` 是合法的 FQDN 写法）；
//! - 去掉 `:端口`（`example.com:8080` 直接丢给接口只会得到“域名不合法”）；
//! - 去掉 `user@` 前缀与 `?query` / `#fragment`。
//!
//! 不做「必须含点」之类的强校验：`localhost`、`例子.中国` 都交给接口判定，
//! 这里只拒绝**明显为空**的输入。

/// 把用户输入压成裸域名；空输入返回 `None`。
///
/// ```text
/// https://www.example.com/path?a=1  -> example.com
/// WWW.Example.COM.                  -> example.com
/// example.com:8080                  -> example.com
/// 例子.中国                          -> 例子.中国
/// ```
pub fn normalize(input: &str) -> Option<String> {
    let mut text = input.trim();
    if text.is_empty() {
        return None;
    }

    // 1. 协议前缀（大小写不敏感）。
    if let Some(rest) = strip_prefix_ignore_ascii_case(text, "https://")
        .or_else(|| strip_prefix_ignore_ascii_case(text, "http://"))
    {
        text = rest;
    }

    // 2. authority 里的 userinfo：只在第一个 `/ ? #` 之前找 `@`，
    //    避免把路径里的 `@` 当分隔符。
    let authority_end = text.find(['/', '?', '#']).unwrap_or(text.len());
    if let Some(at) = text[..authority_end].rfind('@') {
        text = &text[at + 1..];
    }

    // 3. 取 host 部分（第一个 `/ ? #` 之前）。
    let host = text.split(['/', '?', '#']).next().unwrap_or("");

    // 4. 去掉端口：只处理「恰好一个冒号且后面全是数字」，IPv6 字面量不在此列。
    let host = match host.split_once(':') {
        Some((name, port)) if !name.contains(':') && port.chars().all(|ch| ch.is_ascii_digit()) => {
            name
        }
        _ => host,
    };

    // 5. 去掉开头的一层 `www.`。
    let host = strip_prefix_ignore_ascii_case(host, "www.").unwrap_or(host);

    // 6. 去末尾的根点，统一小写。
    let host = host.trim_end_matches('.').to_ascii_lowercase();

    if host.is_empty() || host.contains(char::is_whitespace) || host.contains('/') {
        return None;
    }
    Some(host)
}

/// 大小写不敏感地剥前缀，返回剩余部分（保留原大小写）。
fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}
