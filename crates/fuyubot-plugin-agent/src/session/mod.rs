//! 会话与 JSON 落盘。
//!
//! 会话按 `(chat_id, topic_id)` 归档，一个会话一个 JSON 文件；**文件是唯一真相源**，
//! 进程内不缓存。这样插件重启 / 热重载后历史照旧，worker 与工具处理器并发写同一个
//! 聊天时也不会各自持有一份内存状态互相覆盖。
//!
//! 写入统一走「临时文件 + rename」：同一目录内的 rename 是原子的，读的一方要么看到
//! 旧文件、要么看到完整的新文件，不会读到写了一半的 JSON。临时文件和目标文件放同一
//! 目录，避免跨卷 rename 退化成拷贝（那样就不是原子的了）。
//!
//! 裁剪也在这里做：`max_messages` 只留最后 N 条（模型上下文只向下看），
//! TTL 与 `max_sessions` 由 [`SessionStore::prune`] 清理。

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::{AgentError, AgentResult, ChatKey, ChatMessage, Config};

/// 临时文件序号：同一进程里并发写同一路径也不会撞名。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 一个会话。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// 稳定 id：`<chat_id>__<topic>`。
    pub id: String,
    pub chat_id: i64,
    #[serde(default)]
    pub topic_id: Option<i32>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// 预留：模型 / 元信息。
    #[serde(default)]
    pub meta: Value,
}

impl Session {
    /// 会话 key。
    pub fn key(&self) -> ChatKey {
        ChatKey::new(self.chat_id, self.topic_id)
    }

    /// 列表摘要。
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id.clone(),
            chat_id: self.chat_id,
            topic_id: self.topic_id,
            title: self.title.clone(),
            message_count: self.messages.len(),
            updated_at: self.updated_at,
        }
    }
}

/// 列表里的一条摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub chat_id: i64,
    pub topic_id: Option<i32>,
    pub title: String,
    pub message_count: usize,
    pub updated_at: u64,
}

/// 会话库（进程内共享，文件是唯一真相源）。
pub struct SessionStore {
    dir: PathBuf,
    ttl_days: u64,
    max_sessions: usize,
    max_messages: usize,
}

impl SessionStore {
    /// 打开（自动建目录）。
    pub fn open(config: &Config) -> AgentResult<Self> {
        let dir = config.session_path();
        std::fs::create_dir_all(&dir).map_err(|error| {
            AgentError::Store(format!("建目录 {} 失败：{error}", dir.display()))
        })?;
        Ok(Self {
            dir,
            ttl_days: config.session_ttl_days,
            max_sessions: config.session_max_sessions,
            max_messages: config.session_max_messages,
        })
    }

    /// 目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 读会话（不存在返回 None）。
    pub fn load(&self, key: &ChatKey) -> AgentResult<Option<Session>> {
        self.read_session(&self.path_of(key))
    }

    /// 确保会话存在（不存在则按 title 建）。
    ///
    /// 已存在时只补一个空标题——不覆盖用户 / 上一条消息已经定下的主题。
    pub fn ensure(&self, key: &ChatKey, title: &str) -> AgentResult<Session> {
        if let Some(mut session) = self.load(key)? {
            let title = title.trim();
            if session.title.trim().is_empty() && !title.is_empty() {
                session.title = title.to_owned();
                session.updated_at = crate::unix_now();
                self.write_session(&session)?;
            }
            return Ok(session);
        }
        let now = crate::unix_now();
        let session = Session {
            id: key.file_stem(),
            chat_id: key.chat_id,
            topic_id: key.topic_id,
            title: title.trim().to_owned(),
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            meta: Value::Null,
        };
        self.write_session(&session)?;
        Ok(session)
    }

    /// 追加若干消息并落盘（自动裁剪到上限）。
    pub fn append(&self, key: &ChatKey, messages: &[ChatMessage]) -> AgentResult<Session> {
        let mut session = self.ensure(key, "")?;
        session.messages.extend_from_slice(messages);
        // 只保留最后 N 条：丢弃最旧的，越界保护在 0 配置上也成立。
        let limit = self.max_messages.max(1);
        if session.messages.len() > limit {
            let drop_count = session.messages.len() - limit;
            session.messages.drain(..drop_count);
        }
        session.updated_at = crate::unix_now();
        self.write_session(&session)?;
        Ok(session)
    }

    /// 清空一个会话的消息（保留会话本身）。
    pub fn reset(&self, key: &ChatKey) -> AgentResult<()> {
        let mut session = self.ensure(key, "")?;
        session.messages.clear();
        session.updated_at = crate::unix_now();
        self.write_session(&session)
    }

    /// 删除会话文件；返回是否真的删了。
    pub fn delete(&self, key: &ChatKey) -> AgentResult<bool> {
        let path = self.path_of(key);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(AgentError::Store(format!(
                "删除 {} 失败：{error}",
                path.display()
            ))),
        }
    }

    /// 重命名会话（只改落盘标题，不动消息历史）；返回是否找到。
    pub fn rename(&self, key: &ChatKey, title: &str) -> AgentResult<bool> {
        let Some(mut session) = self.load(key)? else {
            return Ok(false);
        };
        session.title = title.to_owned();
        self.write_session(&session)?;
        Ok(true)
    }

    /// 列出某个 chat 的会话（按 updated_at 降序）。
    pub fn list(&self, chat_id: i64) -> AgentResult<Vec<SessionSummary>> {
        let mut sessions = self.load_all()?;
        sessions.retain(|session| session.chat_id == chat_id);
        sort_newest_first(&mut sessions);
        Ok(sessions.iter().map(Session::summary).collect())
    }

    /// 列出全部会话的摘要（按 updated_at 降序；API 的 `session list` 不带 chat_id 时用）。
    pub fn list_all(&self) -> AgentResult<Vec<SessionSummary>> {
        let mut sessions = self.load_all()?;
        sort_newest_first(&mut sessions);
        Ok(sessions.iter().map(Session::summary).collect())
    }

    /// 按 id 找会话。
    ///
    /// 两种写法：完整文件主干（`-100123__12`）按文件名精确找；纯数字（`12`）按
    /// 「话题号」找，匹配所有以 `__12` 结尾的会话，命中多个时取最新的。
    pub fn find(&self, id: &str) -> AgentResult<Option<Session>> {
        let id = id.trim();
        if id.is_empty() {
            return Ok(None);
        }
        if let Some(path) = self.path_of_id(id)
            && let Some(session) = self.read_session(&path)?
        {
            return Ok(Some(session));
        }
        if !id.chars().all(|ch| ch.is_ascii_digit()) {
            return Ok(None);
        }
        let suffix = format!("__{id}");
        let mut matches: Vec<Session> = self
            .load_all()?
            .into_iter()
            .filter(|session| session.id.ends_with(&suffix))
            .collect();
        sort_newest_first(&mut matches);
        Ok(matches.into_iter().next())
    }

    /// 会话总数。
    pub fn count(&self) -> AgentResult<usize> {
        Ok(self.load_all()?.len())
    }

    /// 清理过期会话（TTL / 数量上限），返回删除数。
    ///
    /// `session_ttl_days = 0` 表示永久保留：不按时间也不按数量删除。
    pub fn prune(&self) -> AgentResult<usize> {
        if self.ttl_days == 0 {
            return Ok(0);
        }
        let now = crate::unix_now();
        let ttl_secs = self.ttl_days.saturating_mul(86_400);
        let limit = self.max_sessions;
        let mut sessions = self.load_all()?;
        sort_newest_first(&mut sessions);

        let mut removed = 0usize;
        let mut kept = 0usize;
        for session in sessions {
            let expired = ttl_secs > 0 && now.saturating_sub(session.updated_at) > ttl_secs;
            let over_limit = limit > 0 && kept >= limit;
            if expired || over_limit {
                if self.remove_id(&session.id)? {
                    removed += 1;
                }
            } else {
                kept += 1;
            }
        }
        Ok(removed)
    }

    /// 落盘目录里的会话文件数（诊断用）。
    pub fn file_count(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .count()
    }

    // ── 内部 ────────────────────────────────────────────────────────

    /// 会话文件路径。
    fn path_of(&self, key: &ChatKey) -> PathBuf {
        self.dir.join(format!("{}.json", key.file_stem()))
    }

    /// 按 id 拼路径；会挡掉路径分隔符与 `..`，工具 / 服务的入参来自外部，不能直接信。
    fn path_of_id(&self, id: &str) -> Option<PathBuf> {
        let id = id.trim();
        let invalid = id.is_empty()
            || id.contains('/')
            || id.contains('\\')
            || id.contains("..")
            || id.contains('\0');
        (!invalid).then(|| self.dir.join(format!("{id}.json")))
    }

    /// 读一个文件；不存在 = None，存在但不是合法 JSON = Store 错误（不静默吞掉数据损坏）。
    fn read_session(&self, path: &Path) -> AgentResult<Option<Session>> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<Session>(&bytes)
                .map(Some)
                .map_err(|error| {
                    AgentError::Store(format!("解析 {} 失败：{error}", path.display()))
                }),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AgentError::Store(format!(
                "读 {} 失败：{error}",
                path.display()
            ))),
        }
    }

    /// 原子写：同目录临时文件 → rename 覆盖。
    fn write_session(&self, session: &Session) -> AgentResult<()> {
        let path = self
            .path_of_id(&session.id)
            .ok_or_else(|| AgentError::Store(format!("非法会话 id：{}", session.id)))?;
        let bytes = serde_json::to_vec_pretty(session).map_err(|error| {
            AgentError::Store(format!("序列化会话 {} 失败：{error}", session.id))
        })?;
        atomic_write(&path, &bytes)
    }

    /// 读目录里的全部合法会话（排序交给调用方）。
    ///
    /// 单个坏文件只跳过、不拖垮整表：会话是旁路数据，不能因为它让 `list` / `prune`
    /// 整体失败；真正要读某个坏文件时会走 [`Self::load`] 报错。
    fn load_all(&self) -> AgentResult<Vec<Session>> {
        let entries = fs::read_dir(&self.dir).map_err(|error| {
            AgentError::Store(format!("读目录 {} 失败：{error}", self.dir.display()))
        })?;
        let mut sessions = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(Some(session)) = self.read_session(&path) {
                sessions.push(session);
            }
        }
        Ok(sessions)
    }

    /// 按 id 删文件；返回是否真的删了。
    fn remove_id(&self, id: &str) -> AgentResult<bool> {
        let Some(path) = self.path_of_id(id) else {
            return Ok(false);
        };
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(AgentError::Store(format!(
                "删除 {} 失败：{error}",
                path.display()
            ))),
        }
    }
}

/// 最新的排前面；时间相同按 id 排，保证输出稳定。
fn sort_newest_first(sessions: &mut [Session]) {
    sessions.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// 原子写：先写同目录临时文件，再 rename 覆盖目标。
///
/// Windows 上 `rename` 底层是 `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，目标存在也能替换；
/// 写失败时删掉临时文件，不给下一次读留下垃圾。
fn atomic_write(path: &Path, bytes: &[u8]) -> AgentResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| AgentError::Store(format!("建目录 {} 失败：{error}", parent.display())))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("session.json");
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{file_name}.{}.{seq}.tmp", std::process::id()));

    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        // 先落盘再 rename：崩溃时要么是旧文件，要么是完整的新文件。
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&tmp);
        return Err(AgentError::Store(format!(
            "写 {} 失败：{error}",
            path.display()
        )));
    }
    Ok(())
}
