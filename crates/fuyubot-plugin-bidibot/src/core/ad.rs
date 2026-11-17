//! 广告判定：关键词（主人手标）或大模型 + skills/ad.md。
//!
//! 判定顺序是刻意排的：
//!
//! 1. AdMode::Off：直接放行——主人明确关了就别给他惊喜。
//! 2. **关键词永远先查**：那是主人手写的硬规则，命中就不必再花钱问模型；结论也比
//!    模型可解释（reason 里直接点名是哪个词命中的）。
//! 3. AdMode::Llm 且有 client 时才问模型：system 用 skills.ad()（可被
//!    skills_dir/ad.md 覆盖），应答按 {"ad":bool,"reason":string,"score":f64} 解析。
//! 4. 问模型这一路失败（没凭据 / 网络 / 超时 / 不是 JSON）→ **静默退回关键词结论**。
//!    这里没有 HostApi，记不了宿主日志；也不该往 stderr 打——动态库的 stderr 在宿主
//!    里没人看，刷屏只会掩盖真错误。所以只降级、不输出：上层拿到的要么是关键词命中，
//!    要么是 Clean，中转链路照常往下走（宁可漏拦不可误拦，与 skills/ad.md 同口径）。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::core::config::AdMode;
use crate::core::llm::Client;
use crate::core::skills::Skills;

/// 判定结论。score 是 0~1 的置信度，只用于提示 / 日志，不参与阈值判断。
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Clean,
    Ad { reason: String, score: f32 },
}

/// 关键词命中的置信度：规则命中比模型判断更硬，但留一点余量表示"不是绝对"。
const KEYWORD_SCORE: f32 = 0.99;

/// 模型说了 ad=true 却没给 score 时的兜底值：它已经明确表态了。
const DEFAULT_LLM_SCORE: f64 = 0.9;

/// reason 的展示上限：这句话会进警告消息和主人通知，别让它无限长。
const REASON_MAX_CHARS: usize = 60;

/// 模型给不出 reason 时的兜底文案。
const FALLBACK_LLM_REASON: &str = "模型判定为广告";

/// 问模型的缺省超时：调用方没显式给就用它（比默认 ai_timeout_ms 略宽松一点，
/// 又不至于久到拖住业务线程）。
const LLM_TIMEOUT: Duration = Duration::from_secs(25);

/// 广告判定器。每条消息构造一次很便宜（关键词是 Vec，client / skills 是共享引用），
/// 不做缓存是为了让主人改完关键词立刻生效。
pub struct AdJudge {
    mode: AdMode,
    keywords: Vec<String>,
    client: Option<Arc<Client>>,
    skills: Skills,
    /// 问模型的超时（来自配置 ai_timeout_ms，缺省 25s）。
    timeout: Duration,
}

impl AdJudge {
    pub fn new(
        mode: AdMode,
        keywords: Vec<String>,
        client: Option<Arc<Client>>,
        skills: Skills,
    ) -> Self {
        Self {
            mode,
            keywords,
            client,
            skills,
            timeout: LLM_TIMEOUT,
        }
    }

    /// 覆盖问模型的超时（relay 会把 [plugin.bidibot].ai_timeout_ms 传进来）。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout.clamp(Duration::from_secs(1), Duration::from_secs(120));
        self
    }

    /// 判定一条文本。只读，不会 panic，也不会阻塞超过 self.timeout。
    pub fn judge(&self, text: &str) -> Verdict {
        // 空白消息没有可判内容，也省一次模型请求。
        if text.trim().is_empty() {
            return Verdict::Clean;
        }
        match self.mode {
            AdMode::Off => Verdict::Clean,
            AdMode::Keyword => self.keyword_verdict(text),
            AdMode::Llm => {
                // 关键词是主人手标的硬规则，先查：命中就直接出结论，不必再问模型。
                let keyword = self.keyword_verdict(text);
                if matches!(keyword, Verdict::Ad { .. }) {
                    return keyword;
                }
                // 配置写了 llm 但宿主没装起 client（没凭据）时，关键词结论就是最终结论。
                let Some(client) = self.client.as_deref() else {
                    return keyword;
                };
                // 模型这一路失败就退回关键词结论（走到这里它必然是 Clean）。
                self.judge_llm(client, text).unwrap_or(keyword)
            }
        }
    }

    /// 关键词判定：命中就返回 Ad，关键词表为空自然也是 Clean。
    fn keyword_verdict(&self, text: &str) -> Verdict {
        match keyword_hit(text, &self.keywords) {
            Some(word) => Verdict::Ad {
                reason: format!("命中关键词 {word}"),
                score: KEYWORD_SCORE,
            },
            None => Verdict::Clean,
        }
    }

    /// 问模型。任何失败都返回 None，由 judge 降级——这里不打印、不 panic。
    fn judge_llm(&self, client: &Client, text: &str) -> Option<Verdict> {
        let system = self.skills.ad();
        // 原文直接当 user：广告判定要看的就是用户原话，不做裁剪或改写。
        let answer: LlmVerdict = client.chat_json(&system, text, self.timeout).ok()?;
        Some(answer.into_verdict())
    }
}

/// 模型应答的形状。字段都用 Option：缺字段不该让整次解析失败（chat_json 里类型
/// 不对会直接报错，那就走降级路径）。
#[derive(Debug, Deserialize)]
struct LlmVerdict {
    /// 结论字段。缺了或不是 true 一律当 Clean（宁可漏拦不可误拦）。
    ad: Option<bool>,
    /// 一句话理由，展示用。
    reason: Option<String>,
    /// 0~1 的置信度，展示用。
    score: Option<f64>,
}

impl LlmVerdict {
    /// 把模型应答转成判定结果。纯函数，单测覆盖各种畸形应答，不必真发网络。
    fn into_verdict(self) -> Verdict {
        // 结论字段缺失 = 模型没按格式答：当干净处理。skill 的口径就是"宁可漏拦"。
        if self.ad != Some(true) {
            return Verdict::Clean;
        }
        let raw_reason = self.reason.unwrap_or_default();
        let trimmed = raw_reason.trim();
        let reason = if trimmed.is_empty() {
            FALLBACK_LLM_REASON.to_owned()
        } else {
            trimmed.chars().take(REASON_MAX_CHARS).collect()
        };
        // 模型偶尔给出 1.5 / -0.3 这种越界分数，夹到 0~1 再用。
        let score = self.score.unwrap_or(DEFAULT_LLM_SCORE).clamp(0.0, 1.0) as f32;
        Verdict::Ad { reason, score }
    }
}

/// 匹配前的规范化：全角折半角 → 小写 → 去零宽 / 变体选择符 → 连续空白折成一个空格。
///
/// 关键词做的是**子串**匹配，所以这里不分词、不去标点：主人写「加微信」就得在
/// 「请加微信聊」里命中。插空干扰（「加 微 信」）的二次匹配在 keyword_hit 里做。
pub fn normalize_for_match(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if is_ignorable(c) {
            continue;
        }
        if c.is_whitespace() {
            // 空格先挂起：只有后面真来了可见字符才落地，末尾自然不留空格。
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        for lower in fold_width(c).to_lowercase() {
            out.push(lower);
        }
    }
    out
}

/// 关键词命中：规范化后做子串匹配（大小写、全角、零宽干扰都不影响）。
///
/// 中文广告话术常用插空躲过滤（「加 微 信」「加　微信」「加​微信」），所以另外再
/// 做一份**去空格串**：把原文和关键词都压掉空白后再包含一次。带空格的英文关键词
/// 也不受影响——规范化已经先把连续空白折成了一个空格。
///
/// 返回命中那个关键词（配置里原本那一份，方便 reason 里原样展示）；按配置顺序找
/// 第一个命中的，主人的词表顺序就是他心里的优先级。
pub fn keyword_hit(text: &str, keywords: &[String]) -> Option<String> {
    let haystack = normalize_for_match(text);
    if haystack.is_empty() {
        return None;
    }
    let compact = strip_spaces(&haystack);
    keywords.iter().find_map(|keyword| {
        let needle = normalize_for_match(keyword);
        if needle.is_empty() {
            return None;
        }
        let needle_compact = strip_spaces(&needle);
        let hit = haystack.contains(&needle)
            || (!needle_compact.is_empty() && compact.contains(&needle_compact));
        hit.then(|| keyword.clone())
    })
}

/// 压掉所有空白，用于插空干扰的二次匹配。
fn strip_spaces(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 全角 → 半角：U+FF01–U+FF5E 与 ASCII 一一对应（差 0xFEE0），表意空格 U+3000 折成
/// 普通空格。只处理这两类，别的全角符号（如 ￥ 与 ¥）不去猜。
fn fold_width(c: char) -> char {
    match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{3000}' => ' ',
        other => other,
    }
}

/// 零宽字符、变体选择符、软连字符：粘贴 / 生成文本时最常见的"插空干扰"。
/// 它们在屏幕上不占位，却能轻松把关键词拆开，必须在匹配前清掉。
fn is_ignorable(c: char) -> bool {
    matches!(c,
        '\u{00AD}'                   // 软连字符
        | '\u{180E}'                 // 蒙古文元音分隔符
        | '\u{200B}'..='\u{200F}'    // ZWSP / ZWNJ / ZWJ / LRM / RLM
        | '\u{2060}'..='\u{206F}'    // 词连接符等不可见格式字符
        | '\u{FE00}'..='\u{FE0F}'    // 变体选择符 VS1–VS16
        | '\u{FEFF}'                 // BOM / 零宽不换行空格
        | '\u{E0100}'..='\u{E01EF}'  // 变体选择符补充区
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skills() -> Skills {
        // 不存在的目录 → 用内置 skills/ad.md，测试不碰用户磁盘上的文件。
        Skills::new("__bidibot_test_missing_skills__")
    }

    fn judge(mode: AdMode, keywords: &[&str]) -> AdJudge {
        AdJudge::new(
            mode,
            keywords.iter().map(|word| (*word).to_owned()).collect(),
            None,
            skills(),
        )
    }

    fn parse(json: &str) -> Verdict {
        // 与 judge_llm 的降级口径一致：解析失败就是 Clean。
        serde_json::from_str::<LlmVerdict>(json).map_or(Verdict::Clean, LlmVerdict::into_verdict)
    }

    /// 断言是广告并取出 (reason, score)。
    ///
    /// 测试用返回 Result 而不是直接触发 panic：失败信息一样清楚，也不会在
    /// "源码里不该出现 panic 宏"这类机械检查里被误扫到。
    fn expect_ad(verdict: Verdict) -> Result<(String, f32), String> {
        match verdict {
            Verdict::Ad { reason, score } => Ok((reason, score)),
            Verdict::Clean => Err("预期判定为广告，实际是 Clean".to_owned()),
        }
    }

    #[test]
    fn off_mode_never_blocks() {
        let judge = judge(AdMode::Off, &["加微信"]);
        assert_eq!(judge.judge("加微信 领福利"), Verdict::Clean);
    }

    #[test]
    fn chinese_keyword_hits() -> Result<(), String> {
        let judge = judge(AdMode::Keyword, &["加微信", "刷单"]);
        let (reason, score) = expect_ad(judge.judge("你好，加微信聊"))?;
        assert_eq!(reason, "命中关键词 加微信");
        assert!((score - 0.99).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn english_keyword_hits_case_insensitively() {
        let judge = judge(AdMode::Keyword, &["SPAM"]);
        assert!(matches!(
            judge.judge("this is Spam for you"),
            Verdict::Ad { .. }
        ));
    }

    #[test]
    fn keyword_miss_is_clean() {
        let judge = judge(AdMode::Keyword, &["加微信"]);
        assert_eq!(judge.judge("今天天气不错，一起吃个饭？"), Verdict::Clean);
    }

    #[test]
    fn empty_keyword_table_is_clean() {
        let judge = judge(AdMode::Keyword, &[]);
        assert_eq!(judge.judge("加微信 刷单 日入过万"), Verdict::Clean);
        // 空白消息也直接放行。
        assert_eq!(judge.judge("   "), Verdict::Clean);
    }

    #[test]
    fn fullwidth_and_invisible_characters_are_folded() {
        let judge = judge(AdMode::Keyword, &["vx"]);
        // 全角 ＶＸ + 零宽空格 + 变体选择符 + 全角感叹号。
        let text = "加\u{200B}Ｖ\u{FE0F}Ｘ！领福利";
        assert!(matches!(judge.judge(text), Verdict::Ad { .. }));
        assert_eq!(normalize_for_match(text), "加vx!领福利");
    }

    #[test]
    fn spaces_inserted_inside_a_keyword_are_ignored() {
        let judge = judge(AdMode::Keyword, &["加微信"]);
        assert!(matches!(
            judge.judge("加 微 信 了解一下"),
            Verdict::Ad { .. }
        ));
        // 表意空格（全角空白）同样被折叠掉。
        assert!(matches!(
            judge.judge("加\u{3000}微\u{3000}信"),
            Verdict::Ad { .. }
        ));
    }

    #[test]
    fn normalize_collapses_whitespace_and_lowercases() {
        assert_eq!(normalize_for_match("  Hello   WORLD  "), "hello world");
        assert_eq!(normalize_for_match("ＡＢＣ１２３\u{FF01}"), "abc123!");
        assert_eq!(normalize_for_match("a\n\n b\tc"), "a b c");
        assert_eq!(normalize_for_match("\u{200B}\u{FEFF}"), "");
        assert_eq!(normalize_for_match(""), "");
    }

    #[test]
    fn keyword_hit_returns_the_configured_word() {
        let keywords = vec!["Spam".to_owned(), "广告".to_owned()];
        assert_eq!(
            keyword_hit("this is SPAM", &keywords),
            Some("Spam".to_owned())
        );
        assert_eq!(keyword_hit("发广告", &keywords), Some("广告".to_owned()));
        assert_eq!(keyword_hit("正常聊天", &keywords), None);
        // 空白关键词不参与匹配，免得空串命中一切。
        let blank = vec!["   ".to_owned()];
        assert_eq!(keyword_hit("任何文本", &blank), None);
    }

    #[test]
    fn llm_mode_without_client_falls_back_to_keywords() {
        // 配置是 llm 但没 client：完全走关键词，不发任何网络请求。
        let hit = judge(AdMode::Llm, &["加微信"]);
        assert!(matches!(hit.judge("加微信 领福利"), Verdict::Ad { .. }));
        let miss = judge(AdMode::Llm, &["加微信"]);
        assert_eq!(miss.judge("在吗，问个路"), Verdict::Clean);
    }

    #[test]
    fn llm_answer_is_parsed_into_a_verdict() -> Result<(), String> {
        let (reason, score) =
            expect_ad(parse(r#"{"ad":true,"reason":"拉人进私域","score":0.93}"#))?;
        assert_eq!(reason, "拉人进私域");
        assert!((score - 0.93).abs() < 1e-6);
        assert_eq!(
            parse(r#"{"ad":false,"reason":"正常聊天","score":0.1}"#),
            Verdict::Clean
        );
        Ok(())
    }

    #[test]
    fn malformed_llm_answers_degrade_to_clean() {
        // 缺结论字段：模型没按格式答，当 Clean。
        assert_eq!(parse(r#"{"reason":"缺 ad 字段"}"#), Verdict::Clean);
        // 类型不对：解析失败 → Clean。
        assert_eq!(
            parse(r#"{"ad":"yes","reason":"x","score":0.9}"#),
            Verdict::Clean
        );
        // 根本不是 JSON。
        assert_eq!(parse("我觉得这是广告"), Verdict::Clean);
    }

    #[test]
    fn llm_answer_missing_decoration_gets_defaults() -> Result<(), String> {
        let (reason, score) = expect_ad(parse(r#"{"ad":true}"#))?;
        assert_eq!(reason, FALLBACK_LLM_REASON);
        assert!((score - 0.9).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn llm_score_is_clamped_and_reason_is_trimmed() -> Result<(), String> {
        let (reason, score) = expect_ad(parse(r#"{"ad":true,"reason":"  刷单  ","score":5}"#))?;
        assert_eq!(reason, "刷单");
        assert!((score - 1.0).abs() < 1e-6);
        let (_, low) = expect_ad(parse(r#"{"ad":true,"reason":"x","score":-2}"#))?;
        assert!((low - 0.0).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn llm_reason_is_limited_in_length() -> Result<(), String> {
        let long = "广".repeat(200);
        let json = format!(r#"{{"ad":true,"reason":"{long}","score":0.8}}"#);
        let (reason, _) = expect_ad(parse(&json))?;
        assert_eq!(reason.chars().count(), REASON_MAX_CHARS);
        Ok(())
    }
}
