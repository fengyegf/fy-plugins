//! SSE（`text/event-stream`）解析器。
//!
//! 三个协议都返回 SSE，但事件形状不同：
//! - OpenAI Chat Completions：只有 `data:`，最后一行 `data: [DONE]`；
//! - OpenAI Responses：有 `event:` 名，`data:` 是 JSON；
//! - Anthropic Messages：有 `event:` 名，`data:` 是 JSON。
//!
//! 这里只做「按 SSE 规范拆事件」，不解释业务语义；协议模块自己把 `data` 反序列化成类型。

use serde_json::Value;

use crate::core::{LlmError, LlmResult};

/// 一条解析后的 SSE 事件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` 字段（未提供时为 `None`）。
    pub event: Option<String>,
    /// 多行 `data:` 用 `\n` 拼接后的文本。
    pub data: String,
    /// `id:` 字段。
    pub id: Option<String>,
    /// `retry:` 字段（毫秒）。
    pub retry: Option<u64>,
}

impl SseEvent {
    /// 事件名；无名事件按 SSE 规范叫 `message`。
    pub fn event_type(&self) -> &str {
        self.event.as_deref().unwrap_or("message")
    }

    /// 是否是 OpenAI Chat Completions 的结束标记。
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }

    /// 把 `data` 解析成 JSON。
    pub fn json(&self) -> LlmResult<Value> {
        serde_json::from_str(&self.data).map_err(|error| {
            let preview: String = self.data.chars().take(300).collect();
            LlmError::Sse(format!("事件数据不是 JSON：{error}；数据：{preview}"))
        })
    }
}

/// 逐事件读取器。
pub struct SseReader<R: std::io::BufRead> {
    reader: R,
    finished: bool,
}

impl<R: std::io::BufRead> SseReader<R> {
    /// 包装一个阻塞读取器。
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            finished: false,
        }
    }

    /// 读下一条事件；流结束时返回 `Ok(None)`。
    ///
    /// 空行是事件分隔符；以 `:` 开头的行是注释；未知字段按规范忽略。
    pub fn next_event(&mut self) -> LlmResult<Option<SseEvent>> {
        if self.finished {
            return Ok(None);
        }

        let mut event = None;
        let mut data = String::new();
        let mut id = None;
        let mut retry = None;
        let mut has_fields = false;

        loop {
            let mut raw = String::new();
            let read = self
                .reader
                .read_line(&mut raw)
                .map_err(|error| LlmError::Sse(format!("读取 SSE 流失败：{error}")))?;

            if read == 0 {
                self.finished = true;
                break;
            }

            let line = raw.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if has_fields {
                    return Ok(Some(SseEvent {
                        event,
                        data,
                        id,
                        retry,
                    }));
                }
                continue;
            }

            if line.starts_with(':') {
                // 注释 / keep-alive。
                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };

            match field {
                "event" => {
                    event = Some(value.to_owned());
                    has_fields = true;
                }
                "data" => {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(value);
                    has_fields = true;
                }
                "id" => {
                    id = Some(value.to_owned());
                    has_fields = true;
                }
                "retry" => {
                    if let Ok(millis) = value.parse::<u64>() {
                        retry = Some(millis);
                    }
                    has_fields = true;
                }
                _ => {}
            }
        }

        if has_fields {
            return Ok(Some(SseEvent {
                event,
                data,
                id,
                retry,
            }));
        }
        Ok(None)
    }
}

impl<R: std::io::BufRead> Iterator for SseReader<R> {
    type Item = LlmResult<SseEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_event() {
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => None,
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}

/// 从一段完整文本里解析**第一条**事件（测试 / 夹具用）。
pub fn parse_sse_block(block: &str) -> Option<SseEvent> {
    let mut reader = SseReader::new(std::io::Cursor::new(block.as_bytes().to_vec()));
    reader.next_event().ok().flatten()
}
