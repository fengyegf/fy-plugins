//! 提示词组装：系统提示 +（可选）压缩摘要 + **全量历史** + 本轮输入。
//!
//! 这里只负责**形状**（OpenAI messages 语义），不下发协议特定字段：协议差异由
//! `GatewayClient::chat` 按 `gateway_method` 处理（例如 Anthropic 的 `system` 要提到
//! 顶层）。这样提示词逻辑跟协议解耦，换网关方法不用重写历史裁剪。
//!
//! 历史默认全量携带：Agent 应该记得这个会话里发生过什么，而不是只记得最近 N 条。
//! 上下文太长时由用户（或模型）显式 `/agent compress`，把历史压成摘要；
//! 摘要以一条 system 消息插在历史前面，后续回合照常带全量消息。

use serde_json::{Value, json};

use crate::core::{ChatMessage, Config};

/// 给模型的输出格式说明（Telegram 富消息 Markdown）。
///
/// 改这份说明时的两条原则：
/// 1. **只写「怎么写」，不写「别怎么写」**——模型会把「不要用 X」「普通美元符号要写成 \\$」
///    这类注意事项当成知识点复述给用户（真机上整段漏出过：「注意：普通美元符号要写成 \\$；
///    不支持 HTML 标签和按钮，公式里别用 align 环境」）。约束改成正确写法示例，就没得复述。
/// 2. 语法必须与**宿主的** `rich_from_markdown` 子集一致（`fuyubot-simplify/src/rich.rs`）：
///    它认 `||剧透||`、`==高亮==`、`<u>` / `<sub>` / `<sup>`、`<details>` 等；旧说明里的
///    「不要输出 HTML 标签」与实现相反，已删掉。
pub const FORMAT_GUIDE: &str = r#"

## 输出格式（Telegram 富消息 Markdown）
下面的写法会渲染成 Telegram 富消息。这些只是排版约定，回复里不需要提及或解释它们；用户问起「你能用什么格式」时举一两个例子即可。

- 强调：**粗体**、*斜体*、~~删除线~~、==高亮==、||剧透||、`行内代码`
- 结构：## 小标题、- 无序列表、1. 有序列表、- [ ] 待办、> 引用、--- 分隔线
- 代码块：三反引号围栏，围栏后标语言（例如 python）
- 表格：| 列 | 列 |，下一行 |:---|:---:|，再往下是数据行；单元格里只放短文字（不放 | 或 ||）
- 链接：[文字](https://…)、[文字](mailto:…)、[文字](tel:…)；裸 URL、邮箱、@用户名、#话题、/命令 由客户端自动识别
- 图片：单独一行 ![](https://…/x.jpg "说明文字")
- 公式：行内 $x^2+y^2$；块级单独一行 $$E=mc^2$$；多行推导写成 $$\\begin{aligned} a&=b+c\\\\ d&=e-f \\end{aligned}$$
- 下划线 / 上下标：<u>下划线</u>、<sub>下标</sub>、<sup>上标</sup>
- 折叠块：<details><summary>标题</summary> 正文 </details>
"#;

/// 系统提示 + 格式说明（始终附加；格式说明只教「怎么写」）。
pub fn with_guide(system: &str) -> String {
    if !system.trim().is_empty() {
        format!("{system}{FORMAT_GUIDE}")
    } else {
        String::new()
    }
}

/// 压缩摘要注入时的前缀。
const SUMMARY_HEADER: &str = "【会话压缩摘要】此前的对话已被压缩成下面这份摘要；继续对话时以它为准，把它当作已经确认过的上下文。\n";

/// 提示词组装器。
pub struct PromptBuilder {
    /// 系统提示词；空串 = 不注入 system 消息。
    pub system: String,
}

impl PromptBuilder {
    /// 按配置建。
    pub fn from_config(config: &Config) -> Self {
        Self {
            system: with_guide(&config.system_prompt),
        }
    }

    /// 组装 OpenAI 形状的 messages。
    ///
    /// 顺序：系统提示 → 压缩摘要（有才发） → 全量历史 → 本轮输入。
    pub fn messages(&self, history: &[ChatMessage], summary: &str, user_text: &str) -> Vec<Value> {
        let mut messages = Vec::with_capacity(history.len() + 3);
        // 系统提示词为空就不发 system 消息：有的网关对空 system 会直接 400。
        if !self.system.trim().is_empty() {
            messages.push(json!({ "role": "system", "content": self.system.as_str() }));
        }
        let summary = summary.trim();
        if !summary.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": format!("{SUMMARY_HEADER}{summary}"),
            }));
        }
        for message in history {
            if message.content.trim().is_empty() {
                continue;
            }
            // 落盘里只会有 user / assistant；万一出现别的角色按 user 处理，
            // 免得网关因为非法 role 直接拒绝整轮请求。
            let role = if message.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            messages.push(json!({ "role": role, "content": message.content.as_str() }));
        }
        messages.push(json!({ "role": "user", "content": user_text }));
        messages
    }
}
