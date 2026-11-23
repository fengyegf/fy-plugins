//! `fengyegf.agent.send`：以 bot 身份往指定聊天 / 话题发消息。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::PluginState;
use crate::core::{AgentError, AgentResult};
use fuyubot_plugin_api::HostExt;

use super::{clean, parse_args};

/// 工具名。
pub const NAME: &str = "fengyegf.agent.send";

/// 工具说明（注册到宿主工具表）。
pub const DESCRIPTION: &str =
    "以 bot 身份往指定聊天 / 话题发消息（markdown 优先富消息，失败逐级降级到纯文本）。";

/// 入参 schema。
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "chat_id": { "type": "integer" },
            "topic_id": { "type": "integer" },
            "text": { "type": "string" },
            "markdown": { "type": "string" },
            "reply_to": { "type": "integer" }
        },
        "required": ["chat_id"],
        "additionalProperties": false
    })
}

/// 入参。
#[derive(Debug, Clone, Default, Deserialize)]
struct SendArgs {
    #[serde(default)]
    chat_id: Option<i64>,
    #[serde(default)]
    topic_id: Option<i32>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    reply_to: Option<i64>,
}

impl SendArgs {
    fn chat_id(&self) -> AgentResult<i64> {
        self.chat_id
            .ok_or_else(|| AgentError::BadRequest("缺少 chat_id".to_owned()))
    }

    /// 纯文本兜底正文：text 优先，其次 markdown。
    fn plain(&self) -> AgentResult<String> {
        match (clean(self.text.as_deref()), clean(self.markdown.as_deref())) {
            (Some(text), _) => Ok(text),
            (None, Some(markdown)) => Ok(markdown),
            (None, None) => Err(AgentError::BadRequest(
                "text / markdown 至少要有一个非空字符串".to_owned(),
            )),
        }
    }

    fn markdown(&self) -> Option<&str> {
        self.markdown
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }
}

/// 处理：markdown 优先富消息，失败降级 markdown → 纯文本。
pub fn handle(state: &PluginState, input: &Value) -> AgentResult<Value> {
    let args: SendArgs = parse_args(input)?;
    let chat_id = args.chat_id()?;
    let plain = args.plain()?;
    let timeout = state.config.send_timeout_ms;
    let topic_id = args.topic_id;
    let reply_to = args.reply_to;

    let sent = match args.markdown() {
        Some(markdown) => {
            let rich_markdown = if state.config.normalize_math {
                crate::core::normalize_math(markdown)
            } else {
                markdown.to_owned()
            };
            match state
                .td
                .send_rich(chat_id, topic_id, &rich_markdown, reply_to, timeout)
            {
                Ok(sent) => sent,
                Err(rich_error) => {
                    state.host.warn(&format!(
                        "agent: 富消息发送失败，退回 markdown：{rich_error}"
                    ));
                    match state.td.send_text(
                        chat_id,
                        topic_id,
                        markdown,
                        Some("markdown"),
                        reply_to,
                        timeout,
                    ) {
                        Ok(sent) => sent,
                        Err(markdown_error) => {
                            state.host.warn(&format!(
                                "agent: markdown 发送失败，退回纯文本：{markdown_error}"
                            ));
                            state
                                .td
                                .send_text(chat_id, topic_id, &plain, None, reply_to, timeout)?
                        }
                    }
                }
            }
        }
        None => state
            .td
            .send_text(chat_id, topic_id, &plain, None, reply_to, timeout)?,
    };

    Ok(json!({
        "ok": true,
        "chat_id": sent.chat_id,
        "message_id": sent.message_id,
        "offline": sent.offline,
    }))
}
