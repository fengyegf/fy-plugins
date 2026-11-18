//! 本地数据库（封禁 / 警告 / 名片头映射 / 用户档案 / 论坛话题）。
//!
//! 形态很朴素：**一个 JSON 文件 + 一份进程内快照**（`Mutex<Data>`）。
//!
//! 为什么不用 SQLite：本插件要存的东西只有几百条（黑名单、警告计数、用户档案），
//! 引一个带 C 依赖的库不划算；而 JSON 出问题时主人可以用记事本直接修。
//! 代价是每次变更整份重写——文件小（几十 KB）可以接受，换来的是崩溃后
//! 文件不会停在「写了一半」的状态。
//!
//! 落盘策略：
//! 1. 先把完整快照写成 `<path>.tmp`；
//! 2. 再 `rename` 到正式路径——同一目录内 rename 是原子的，读者要么看到旧文件、
//!    要么看到新文件，不会读到半份 JSON；
//! 3. Windows 的 rename 不覆盖已存在的目标，所以失败时先把旧文件挪成
//!    `<path>.bak`（顺手留一份上一版可用数据），再 rename；两次都失败就回滚，
//!    保证不会出现「新旧都没了」。
//!
//! 损坏恢复：文件读出来不是合法 JSON 时，先复制一份 `<path>.bak.<时间戳>`，
//! 再用空库重建并照样返回 `Ok`——一个坏掉的本地库不该让整个 bot 起不来。
//!
//! 安全约束：对外函数的失败都只是 `StoreError`，本文件不 panic、不 unwrap 外部输入
//! （文件内容、用户传进来的 id / 字符串），索引越界也不可能发生（全用 map 的 get）。
//!
//! 落盘失败时**内存里的改动保留**：调用方拿到 `StoreError::Io` 只用于记日志，
//! 不会让运行期状态回退（存储写失败时只丢弃本次写入）。

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 数据文件结构版本号：以后字段有增删时靠它决定迁移方式。
///
/// 现在只有 1 版；读旧文件时缺这个字段就按 1 处理（`serde(default)`）。
const DATA_VERSION: u32 = 1;

/// `reports` 映射最多保留多少条。
///
/// 用户每来一条消息就记一条「主人侧消息 id → 用户 id」，不设上限的话文件会
/// 无限增长（长跑的 bot 尤其明显）。主人私聊里的消息 id 是单调递增的，所以保留
/// 最大的若干个就是保留最近的若干个；被挤掉的旧映射只影响「回复一条非常久远的
/// 中转消息」，用户那边最多是这次回复没送出去，不会串到别人身上。
const MAX_REPORTS: usize = 10_000;

/// 一条拉黑记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanRecord {
    /// 被拉黑的用户 id。
    pub user_id: i64,
    /// 拉黑时刻（unix 秒），`/bidilist` 按它倒序排。
    pub at: u64,
    /// 拉黑原因（广告判定理由 / 主人备注）。
    pub reason: String,
    /// 操作者 user id：主人手动拉黑时是主人 id，自动熔断时也是主人 id。
    pub by: i64,
    /// 是否广告熔断自动触发（`/bidilist` 里要区分开）。
    pub auto: bool,
}

/// 一个用户的警告计数。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WarnRecord {
    /// 累计警告次数（冷却期内的重复触发不计数）。
    pub count: u32,
    /// 最近一次**计数**的时刻（unix 秒），冷却判断用。
    pub last_at: u64,
    /// 最近一次警告原因，方便主人排查。
    pub last_reason: String,
}

/// 用户档案：名片头消息 id 存在这里，后续消息靠它引用名片头。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub user_id: i64,
    /// 昵称（TDLib 给的 first_name + last_name，可能为空）。
    pub name: String,
    /// 用户名（不含 `@`），没有就是 `None`。
    pub username: Option<String>,
    /// 最后一次识别出的语言代码（zh / en …）。
    pub lang: Option<String>,
    /// 第一次见到该用户的时刻（unix 秒）。
    pub first_seen: u64,
    /// 发给主人的那条「名片头」消息 id；0 = 还没发过。
    pub base_message_id: i64,
}

/// 一个用户对应的论坛话题（v0.2：一个用户一个话题，替代 v0.1 的私聊中转）。
///
/// topics 的 key 是 user_id，所以「用户 → 话题」是 BTreeMap 的直接查；
/// 「话题 → 用户」交给 `topic_user()` 线性扫（量小，不值得再维护一张容易失同步的反向表）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicRecord {
    /// 用户 user id（也是 topics 的 key）。
    pub user_id: i64,
    /// TDLib 的 forum_topic_id；schema 里是 int32，这里用 i64 存，少一层转换与溢出判断。
    pub thread_id: i64,
    /// 话题名（建话题时用用户昵称，按 topic_name_max 截断）。
    pub name: String,
    /// 建话题时刻（unix 秒）。
    pub created_at: u64,
    /// 名片消息 id（0 = 没发 / 没拿到）。
    pub card_message_id: i64,
}

/// 一次 `warn()` 的结果：`counted = false` 表示在冷却期内、这次没计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarnOutcome {
    pub count: u32,
    pub counted: bool,
}

/// 本地库规模（`/bidistatus` 用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoreStats {
    pub bans: usize,
    pub users: usize,
    pub warned: usize,
    /// 已建话题数（一个用户一条）。
    pub topics: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// 读写磁盘失败（目录权限、磁盘满、rename 失败…）。
    #[error("读写本地库失败: {0}")]
    Io(String),
    /// 序列化失败——理论上不会发生（字段全是基础类型），留作兜底。
    #[error("解析本地库失败: {0}")]
    Parse(String),
}

/// 落盘的 JSON 结构。
///
/// 整体带 `#[serde(default)]`：旧版本文件缺字段（或主人手改漏了）时用缺省值补齐，
/// 不要因为少一个键就把整个库当「坏文件」备份掉。多出来的未知键会被 serde 忽略，
/// 这样以后加字段也不会让旧程序读不了新文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    /// 结构版本号。
    #[serde(default = "default_version")]
    version: u32,
    /// 黑名单：user_id → 记录。
    bans: BTreeMap<i64, BanRecord>,
    /// 警告：user_id → 记录。
    warns: BTreeMap<i64, WarnRecord>,
    /// 用户档案：user_id → 档案。
    profiles: BTreeMap<i64, Profile>,
    /// 论坛话题：user_id → 话题记录（v0.2 起；旧文件缺这个键时就是空表）。
    #[serde(default)]
    topics: BTreeMap<i64, TopicRecord>,
    /// 中转映射：主人侧消息 id → 用户 id（主人回复那条消息时靠它找目标）。
    reports: BTreeMap<i64, i64>,
    /// 被拦用户提示的限频时刻：user_id → unix 秒。
    last_notice: BTreeMap<i64, u64>,
}

fn default_version() -> u32 {
    DATA_VERSION
}

impl Default for Data {
    fn default() -> Self {
        Self {
            version: DATA_VERSION,
            bans: BTreeMap::new(),
            warns: BTreeMap::new(),
            profiles: BTreeMap::new(),
            topics: BTreeMap::new(),
            reports: BTreeMap::new(),
            last_notice: BTreeMap::new(),
        }
    }
}

/// 本地数据库：路径 + 内存快照。构造一次后所有线程共享（`Arc<Store>`）。
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    data: Mutex<Data>,
}

impl Store {
    /// 打开本地库：建父目录、读文件、坏文件备份后重建。
    ///
    /// 只有「读文件本身出错」（权限 / 路径是目录…）才返回 `Err`；
    /// 文件不存在、内容坏掉都属于可恢复情况，返回建好的空库。
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| {
                StoreError::Io(format!("创建本地库目录 {} 失败: {error}", parent.display()))
            })?;
        }

        let (data, rebuild) = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Data>(&bytes) {
                Ok(data) => (data, false),
                Err(error) => {
                    // 坏文件：先留一份带时间戳的备份，再用空库重建。
                    // 备份失败也继续（宁可丢历史，也别让插件起不来）。
                    let backup = with_suffix(&path, &format!(".bak.{}", unix_millis()));
                    match fs::copy(&path, &backup) {
                        Ok(_) => eprintln!(
                            "bidibot: 本地库 {} 不是合法 JSON（{error}），已备份到 {}",
                            path.display(),
                            backup.display()
                        ),
                        Err(io_error) => eprintln!(
                            "bidibot: 本地库 {} 不是合法 JSON（{error}），备份到 {} 失败（{io_error}），直接用空库重建",
                            path.display(),
                            backup.display()
                        ),
                    }
                    (Data::default(), true)
                }
            },
            // 第一次启动：文件还不存在，正常情况，等第一次变更再落盘。
            Err(error) if error.kind() == ErrorKind::NotFound => (Data::default(), false),
            Err(error) => {
                return Err(StoreError::Io(format!(
                    "读取本地库 {} 失败: {error}",
                    path.display()
                )));
            }
        };

        let store = Self {
            path,
            data: Mutex::new(data),
        };
        if rebuild {
            let snapshot = store.lock();
            if let Err(error) = store.save(&snapshot) {
                // 重建失败不阻塞启动：内存态照常可用，之后每次变更都会再试一次落盘。
                eprintln!(
                    "bidibot: 重建本地库 {} 失败（内存态已生效）: {error}",
                    store.path.display()
                );
            }
        }
        Ok(store)
    }

    /// 该用户是否在黑名单里。
    pub fn is_banned(&self, user_id: i64) -> Option<BanRecord> {
        self.snapshot(|data| data.bans.get(&user_id).cloned())
    }

    /// 拉黑 / 覆盖更新拉黑：`at` 取当前时间，`reason` / `by` / `auto` 一起更新。
    ///
    /// 重复拉黑**不**新增条目——设计是「一个用户一条记录」，否则 `/bidilist`
    /// 里同一个人会刷屏。
    pub fn ban(&self, user_id: i64, reason: &str, by: i64, auto: bool) -> Result<(), StoreError> {
        self.mutate(|data| {
            data.bans.insert(
                user_id,
                BanRecord {
                    user_id,
                    at: unix_now(),
                    reason: reason.to_owned(),
                    by,
                    auto,
                },
            );
        })
    }

    /// 解封；返回是否真的删掉了记录（对没拉黑过的用户返回 `false`，不落盘）。
    pub fn unban(&self, user_id: i64) -> Result<bool, StoreError> {
        let mut data = self.lock();
        if data.bans.remove(&user_id).is_none() {
            return Ok(false);
        }
        self.save(&data)?;
        Ok(true)
    }

    /// 黑名单，按拉黑时间**倒序**（最近的在前）；同一秒时按 user_id 升序，保证输出稳定。
    pub fn bans(&self) -> Vec<BanRecord> {
        self.snapshot(|data| {
            let mut list: Vec<BanRecord> = data.bans.values().cloned().collect();
            list.sort_by(|left, right| {
                right
                    .at
                    .cmp(&left.at)
                    .then_with(|| left.user_id.cmp(&right.user_id))
            });
            list
        })
    }

    /// 当前的警告次数（没记录就是 0）。
    pub fn warnings(&self, user_id: i64) -> u32 {
        self.snapshot(|data| data.warns.get(&user_id).map_or(0, |record| record.count))
    }

    /// 计一次警告。
    ///
    /// `cooldown_secs` 内重复触发**不计数、不改记录、也不落盘**（`counted = false`，
    /// `count` 返回当前值）：广告机器人会连续刷屏，冷却期就是用来避免「刷十条就熔断」
    /// 这种误伤，同时省掉没必要的磁盘写。`now` 由调用方传入，方便单测。
    pub fn warn(
        &self,
        user_id: i64,
        reason: &str,
        cooldown_secs: u64,
        now: u64,
    ) -> Result<WarnOutcome, StoreError> {
        let mut data = self.lock();
        let outcome = {
            let record = data.warns.entry(user_id).or_default();
            // count == 0 的记录（理论上不存在）不算进入冷却，避免首条警告被吞掉。
            if record.count > 0 && now.saturating_sub(record.last_at) < cooldown_secs {
                return Ok(WarnOutcome {
                    count: record.count,
                    counted: false,
                });
            }
            record.count = record.count.saturating_add(1);
            record.last_at = now;
            record.last_reason = reason.to_owned();
            WarnOutcome {
                count: record.count,
                counted: true,
            }
        };
        self.save(&data)?;
        Ok(outcome)
    }

    /// 清零警告；返回是否真的删掉了记录。
    pub fn clear_warnings(&self, user_id: i64) -> Result<bool, StoreError> {
        let mut data = self.lock();
        if data.warns.remove(&user_id).is_none() {
            return Ok(false);
        }
        self.save(&data)?;
        Ok(true)
    }

    /// 读用户档案（不存在就是 `None`）。
    pub fn profile(&self, user_id: i64) -> Option<Profile> {
        self.snapshot(|data| data.profiles.get(&user_id).cloned())
    }

    /// 整份写入 / 覆盖用户档案（键用 `profile.user_id`）。
    pub fn put_profile(&self, profile: Profile) -> Result<(), StoreError> {
        self.mutate(|data| {
            let user_id = profile.user_id;
            data.profiles.insert(user_id, profile);
        })
    }

    /// 名片头消息 id；没有（或档案里是 0）就返回 `None`。
    ///
    /// 0 不是合法消息 id：返回 `Some(0)` 会让中转链路以为「名片头已发」，
    /// 然后拿 0 去引用，TDLib 直接报错。所以这里把 0 当成「没有」。
    pub fn base_message(&self, user_id: i64) -> Option<i64> {
        self.snapshot(|data| {
            data.profiles
                .get(&user_id)
                .map(|profile| profile.base_message_id)
                .filter(|message_id| *message_id != 0)
        })
    }

    /// 记下名片头消息 id，**同时同步到用户档案**（没有档案就顺手建一个空的）。
    ///
    /// 两个地方都要有：`base_message()` 给中转链路快速判断「首条消息」，
    /// `Profile` 给 `/bidistatus` 与用户语言判断用；分开写就会出现不一致。
    pub fn set_base_message(&self, user_id: i64, message_id: i64) -> Result<(), StoreError> {
        self.mutate(|data| {
            let profile = data.profiles.entry(user_id).or_insert_with(|| Profile {
                user_id,
                name: String::new(),
                username: None,
                lang: None,
                first_seen: unix_now(),
                base_message_id: 0,
            });
            // 档案可能是外部塞进来的（键值不一致），这里以入参为准。
            profile.user_id = user_id;
            profile.base_message_id = message_id;
        })
    }

    /// 用户对应的话题记录；还没建过话题就是 None。
    pub fn topic(&self, user_id: i64) -> Option<TopicRecord> {
        self.snapshot(|data| data.topics.get(&user_id).cloned())
    }

    /// 写入 / 覆盖用户的话题记录。
    ///
    /// 写之前先清掉**同一个 thread_id 的旧记录**：TDLib 删话题后 id 可能被复用，
    /// 或者话题被换给了别人；不清掉的话反查会出现两个答案，主人回复就会串人。
    pub fn set_topic(&self, topic: TopicRecord) -> Result<(), StoreError> {
        self.mutate(|data| {
            let user_id = topic.user_id;
            let thread_id = topic.thread_id;
            data.topics
                .retain(|_, record| record.thread_id != thread_id);
            data.topics.insert(user_id, topic);
        })
    }

    /// 反查：话题 → 用户（线性扫，量小）。
    ///
    /// 刻意不存反向表：反向表要在 set_topic / remove_topic 两处同步，漏一处就永久失同步；
    /// 而一个用户只有一条记录，几百条扫一遍可以忽略，正确性优先。
    pub fn topic_user(&self, thread_id: i64) -> Option<i64> {
        self.snapshot(|data| {
            data.topics
                .values()
                .find(|record| record.thread_id == thread_id)
                .map(|record| record.user_id)
        })
    }

    /// 删除用户的话题记录（话题被删 / 清理时用）；返回是否真的删掉了。
    pub fn remove_topic(&self, user_id: i64) -> Result<bool, StoreError> {
        let mut data = self.lock();
        if data.topics.remove(&user_id).is_none() {
            return Ok(false);
        }
        self.save(&data)?;
        Ok(true)
    }

    /// 全部话题，按创建时间倒序（最新在前）；同一秒按 user_id 升序，输出稳定。
    pub fn topics(&self) -> Vec<TopicRecord> {
        self.snapshot(|data| {
            let mut list: Vec<TopicRecord> = data.topics.values().cloned().collect();
            list.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.user_id.cmp(&right.user_id))
            });
            list
        })
    }

    /// 记住「主人侧那条中转消息 id → 用户 id」。
    ///
    /// 主人直接回复这条消息时，relay 靠它找到该发给谁。每条都要记，所以有
    /// `MAX_REPORTS` 上限，超出后按消息 id 丢最旧的（消息 id 单调递增）。
    pub fn remember_report(&self, owner_message_id: i64, user_id: i64) -> Result<(), StoreError> {
        self.mutate(|data| {
            data.reports.insert(owner_message_id, user_id);
            if data.reports.len() > MAX_REPORTS {
                let overflow = data.reports.len().saturating_sub(MAX_REPORTS);
                let expired: Vec<i64> = data.reports.keys().take(overflow).copied().collect();
                for key in expired {
                    data.reports.remove(&key);
                }
            }
        })
    }

    /// 主人回复的那条消息对应哪个用户；没记过就是 `None`。
    pub fn report_target(&self, owner_message_id: i64) -> Option<i64> {
        self.snapshot(|data| data.reports.get(&owner_message_id).copied())
    }

    /// 上次给被拦用户发提示的时刻（unix 秒）；没有记录就是 0。
    pub fn last_notice(&self, user_id: i64) -> u64 {
        self.snapshot(|data| data.last_notice.get(&user_id).copied().unwrap_or(0))
    }

    /// 记下给被拦用户发提示的时刻（限频用）。
    pub fn set_last_notice(&self, user_id: i64, at: u64) -> Result<(), StoreError> {
        self.mutate(|data| {
            data.last_notice.insert(user_id, at);
        })
    }

    /// 本地库规模。
    pub fn stats(&self) -> StoreStats {
        self.snapshot(|data| StoreStats {
            bans: data.bans.len(),
            users: data.profiles.len(),
            warned: data
                .warns
                .values()
                .filter(|record| record.count > 0)
                .count(),
            topics: data.topics.len(),
        })
    }

    /// 拿锁；互斥锁被 poison 也继续用——内存数据本身仍是一致的，
    /// 没必要因为一次 panic 让整个插件再也写不了库。
    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 只读快照。
    fn snapshot<T>(&self, project: impl FnOnce(&Data) -> T) -> T {
        let data = self.lock();
        project(&data)
    }

    /// 改内存再落盘。落盘失败返回 `Err`，但**内存改动保留**（见文件头说明）。
    fn mutate<T>(&self, change: impl FnOnce(&mut Data) -> T) -> Result<T, StoreError> {
        let mut data = self.lock();
        let output = change(&mut data);
        self.save(&data)?;
        Ok(output)
    }

    /// 原子写：`<path>.tmp` → rename 覆盖正式路径。
    ///
    /// 调用方必须已经持有锁：写出去的必须是「当前」快照，否则并发写会把别人的
    /// 改动覆盖掉（先写后锁的竞态）。
    fn save(&self, data: &Data) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec_pretty(data)
            .map_err(|error| StoreError::Parse(format!("序列化本地库失败: {error}")))?;
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| {
                StoreError::Io(format!("创建本地库目录 {} 失败: {error}", parent.display()))
            })?;
        }

        let tmp = with_suffix(&self.path, ".tmp");
        fs::write(&tmp, &bytes).map_err(|error| {
            StoreError::Io(format!("写临时文件 {} 失败: {error}", tmp.display()))
        })?;

        match fs::rename(&tmp, &self.path) {
            Ok(()) => Ok(()),
            Err(rename_error) => {
                // Windows：目标已存在时 rename 会失败（不是覆盖语义）。
                // 退一步先把旧文件挪成 .bak（留一份上一版可用数据），再 rename。
                let backup = with_suffix(&self.path, ".bak");
                // 上一轮 Windows 分支可能留下了 .bak；不清掉这次 rename 又会失败。
                let _ = fs::remove_file(&backup);
                let moved = match fs::rename(&self.path, &backup) {
                    Ok(()) => true,
                    // 目标路径不存在（比如第一次写、或刚才被移走）就无需备份。
                    Err(error) if error.kind() == ErrorKind::NotFound => false,
                    Err(error) => {
                        let _ = fs::remove_file(&tmp);
                        return Err(StoreError::Io(format!(
                            "替换本地库 {} 失败: {rename_error}；挪走旧文件也失败: {error}",
                            self.path.display()
                        )));
                    }
                };
                match fs::rename(&tmp, &self.path) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        // 回滚：把 .bak 还原成正式文件，别让可用数据只剩个 .bak。
                        if moved && let Err(restore) = fs::rename(&backup, &self.path) {
                            eprintln!(
                                "bidibot: 回滚本地库 {} 失败，旧数据留在 {}: {restore}",
                                self.path.display(),
                                backup.display()
                            );
                        }
                        let _ = fs::remove_file(&tmp);
                        Err(StoreError::Io(format!(
                            "替换本地库 {} 失败: {rename_error} / {error}",
                            self.path.display()
                        )))
                    }
                }
            }
        }
    }
}

/// 在路径后面**追加**后缀（`db.json` → `db.json.tmp`）。
///
/// 不用 `Path::with_extension`：那会吃掉原扩展名（`db.json` → `db.tmp`），
/// 和设计契约里写的 `<path>.tmp` 不一致。
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// 当前 unix 秒；系统时间早于纪元（几乎不可能）时退回 0，不 panic。
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 当前 unix 毫秒：只用于给坏文件备份起唯一名字。
fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 自造临时目录：不引 tempfile，靠「进程 id + 纳秒 + 自增序号」保证不撞名。
    ///
    /// 根目录放在 workspace 的 `target/bidibot-test-tmp`，**不用**
    /// `std::env::temp_dir()`：受限环境（沙箱 / CI）里系统临时目录可能不可写，
    /// Windows 上就是「拒绝访问 os error 5」，单测会直接挂。`target/` 一定是可写的
    /// （cargo 自己就在往里写）而且已被 git 忽略；同一 crate 的 `core::skills`
    /// 单测也是这么做的，保持一致。
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("target")
                .join("bidibot-test-tmp");
            let path = root.join(format!("store-{tag}-{}-{nanos}-{seq}", std::process::id()));
            fs::create_dir_all(&path).expect("应当能建测试临时目录");
            Self { path }
        }

        fn db(&self) -> PathBuf {
            self.path.join("db.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // 测试失败也清理，别在系统临时目录里留垃圾。
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn profile(user_id: i64, name: &str) -> Profile {
        Profile {
            user_id,
            name: name.to_owned(),
            username: Some("zs".to_owned()),
            lang: Some("zh".to_owned()),
            first_seen: 1_000,
            base_message_id: 0,
        }
    }

    fn topic(user_id: i64, thread_id: i64, name: &str) -> TopicRecord {
        TopicRecord {
            user_id,
            thread_id,
            name: name.to_owned(),
            created_at: 1_000 + thread_id as u64,
            card_message_id: 0,
        }
    }

    #[test]
    fn creates_missing_directories_and_opens_empty() {
        let dir = TempDir::new("dirs");
        let db = dir.path.join("nested").join("data").join("db.json");
        assert!(!db.exists());
        let store = Store::open(&db).expect("父目录不存在也要能打开");
        assert!(store.bans().is_empty());
        assert!(store.is_banned(1).is_none());
        assert_eq!(store.stats(), StoreStats::default());
        // 第一次变更时父目录 / 文件才真正创建。
        store.ban(3, "广告", 1, false).expect("拉黑");
        assert!(db.exists());
    }

    #[test]
    fn ban_roundtrip_and_unban() {
        let dir = TempDir::new("ban");
        let store = Store::open(&dir.db()).expect("打开本地库");
        store.ban(7, "广告", 1, false).expect("拉黑");
        let record = store.is_banned(7).expect("应当已拉黑");
        assert_eq!(record.user_id, 7);
        assert_eq!(record.reason, "广告");
        assert_eq!(record.by, 1);
        assert!(!record.auto);
        assert!(record.at > 0);

        // 重复拉黑是覆盖写：原因 / 来源 / auto 更新，条目不会翻倍。
        let first_at = record.at;
        store.ban(7, "刷屏", 9, true).expect("再次拉黑");
        assert_eq!(store.bans().len(), 1);
        let record = store.is_banned(7).expect("应当仍在黑名单");
        assert_eq!(record.reason, "刷屏");
        assert_eq!(record.by, 9);
        assert!(record.auto);
        assert!(record.at >= first_at);

        assert!(store.unban(7).expect("解封"));
        assert!(store.is_banned(7).is_none());
        // 没拉黑过的用户解封返回 false，且不产生写入。
        assert!(!store.unban(7).expect("重复解封返回 false"));
    }

    #[test]
    fn bans_are_newest_first() {
        let dir = TempDir::new("bans-order");
        let store = Store::open(&dir.db()).expect("打开本地库");
        // 白盒塞入固定 at：不依赖真实时钟，确保顺序断言稳定。
        store
            .mutate(|data| {
                for (user_id, at) in [(1_i64, 100_u64), (2, 300), (3, 200)] {
                    data.bans.insert(
                        user_id,
                        BanRecord {
                            user_id,
                            at,
                            reason: String::new(),
                            by: 0,
                            auto: false,
                        },
                    );
                }
            })
            .expect("写入黑名单");
        let ids: Vec<i64> = store.bans().iter().map(|record| record.user_id).collect();
        assert_eq!(ids, vec![2, 3, 1]);
    }

    #[test]
    fn warn_respects_cooldown() {
        let dir = TempDir::new("warn");
        let store = Store::open(&dir.db()).expect("打开本地库");

        assert_eq!(
            store.warn(5, "广告", 30, 1_000).expect("首次警告"),
            WarnOutcome {
                count: 1,
                counted: true
            }
        );
        // 冷却期内：不计数、不改记录。
        assert_eq!(
            store.warn(5, "广告", 30, 1_010).expect("冷却期内"),
            WarnOutcome {
                count: 1,
                counted: false
            }
        );
        assert_eq!(
            store.snapshot(|data| data.warns.get(&5).map(|r| r.last_at)),
            Some(1_000)
        );
        // 冷却结束（elapsed == cooldown）就正常计数。
        assert_eq!(
            store.warn(5, "刷屏", 30, 1_030).expect("冷却结束"),
            WarnOutcome {
                count: 2,
                counted: true
            }
        );
        assert_eq!(store.warnings(5), 2);
        // cooldown = 0 表示不冷却。
        assert_eq!(
            store.warn(5, "再来", 0, 1_030).expect("无冷却"),
            WarnOutcome {
                count: 3,
                counted: true
            }
        );
        // 没记录的用户读出来是 0。
        assert_eq!(store.warnings(99), 0);

        assert!(store.clear_warnings(5).expect("清空警告"));
        assert_eq!(store.warnings(5), 0);
        assert!(!store.clear_warnings(5).expect("重复清空返回 false"));
    }

    #[test]
    fn set_base_message_syncs_profile() {
        let dir = TempDir::new("base-message");
        let store = Store::open(&dir.db()).expect("打开本地库");
        assert!(store.profile(42).is_none());

        store.set_base_message(42, 555).expect("记录名片头");
        let created = store.profile(42).expect("应当顺手建一份档案");
        assert_eq!(created.user_id, 42);
        assert_eq!(created.base_message_id, 555);
        assert_eq!(created.name, "");
        assert!(created.first_seen > 0);
        assert_eq!(store.base_message(42), Some(555));

        // 已有档案只改 base_message_id，其它字段（昵称 / 首次时间）保留。
        store.put_profile(profile(42, "李四")).expect("写档案");
        store.set_base_message(42, 666).expect("更新名片头");
        let updated = store.profile(42).expect("档案还在");
        assert_eq!(updated.name, "李四");
        assert_eq!(updated.first_seen, 1_000);
        assert_eq!(updated.base_message_id, 666);
        assert_eq!(store.base_message(42), Some(666));

        // 0 不是合法消息 id，等价于「还没发过名片头」。
        store.put_profile(profile(43, "王五")).expect("写档案");
        assert_eq!(store.base_message(43), None);
        assert_eq!(store.profile(43).map(|p| p.base_message_id), Some(0));
    }

    #[test]
    fn report_map_is_capped() {
        let dir = TempDir::new("reports");
        let store = Store::open(&dir.db()).expect("打开本地库");
        // 先直接填满（逐条 remember_report 会写盘 1 万次，白盒插入更快）。
        store
            .mutate(|data| {
                for message_id in 1..=MAX_REPORTS as i64 {
                    data.reports.insert(message_id, 1);
                }
            })
            .expect("填满映射");
        store
            .remember_report(MAX_REPORTS as i64 + 1, 2)
            .expect("再记一条");
        assert_eq!(store.snapshot(|data| data.reports.len()), MAX_REPORTS);
        // 最旧的（消息 id 最小）被挤掉，最新的一定在。
        assert_eq!(store.report_target(1), None);
        assert_eq!(store.report_target(MAX_REPORTS as i64 + 1), Some(2));
    }

    #[test]
    fn stats_counts_stored_entities() {
        let dir = TempDir::new("stats");
        let store = Store::open(&dir.db()).expect("打开本地库");
        assert_eq!(store.stats(), StoreStats::default());

        store.ban(1, "广告", 1, false).expect("拉黑");
        store.ban(2, "广告", 1, true).expect("拉黑");
        store.warn(1, "广告", 0, 10).expect("警告");
        store.warn(2, "广告", 0, 10).expect("警告");
        assert!(store.clear_warnings(2).expect("清空警告"));
        store.put_profile(profile(1, "A")).expect("写档案");
        // set_base_message 会顺手建档案，users 因此含用户 3。
        store.set_base_message(3, 9).expect("名片头");

        let stats = store.stats();
        assert_eq!(stats.bans, 2);
        assert_eq!(stats.users, 2);
        assert_eq!(stats.warned, 1);
    }

    #[test]
    fn reopens_and_reads_back() {
        let dir = TempDir::new("persist");
        let db = dir.db();
        {
            let store = Store::open(&db).expect("打开本地库");
            store.ban(11, "广告", 1, false).expect("拉黑");
            store.warn(11, "广告", 0, 500).expect("警告");
            store.put_profile(profile(11, "张三")).expect("档案");
            store.set_base_message(11, 7_001).expect("名片头");
            store.remember_report(7_001, 11).expect("记录映射");
            store.set_last_notice(11, 900).expect("提示时间");
        } // store 在这里 drop，模拟进程重启

        let store = Store::open(&db).expect("重新打开");
        assert_eq!(
            store.is_banned(11).map(|record| record.reason),
            Some("广告".to_owned())
        );
        assert_eq!(store.warnings(11), 1);
        let profile = store.profile(11).expect("档案应当读回");
        assert_eq!(profile.name, "张三");
        assert_eq!(profile.username.as_deref(), Some("zs"));
        assert_eq!(profile.lang.as_deref(), Some("zh"));
        assert_eq!(profile.base_message_id, 7_001);
        assert_eq!(store.base_message(11), Some(7_001));
        assert_eq!(store.report_target(7_001), Some(11));
        assert_eq!(store.last_notice(11), 900);

        // 文件就是设计契约里的扁平 JSON：version 字段要写进去。
        let raw = fs::read_to_string(&db).expect("读本地库文件");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("应当是合法 JSON");
        assert_eq!(value["version"].as_u64(), Some(u64::from(DATA_VERSION)));
        assert_eq!(value["bans"]["11"]["reason"].as_str(), Some("广告"));
        // tmp + rename 落盘：写完不能留下临时文件。
        assert!(!dir.path.join("db.json.tmp").exists());
    }

    #[test]
    fn corrupt_file_is_backed_up_and_rebuilt() {
        let dir = TempDir::new("corrupt");
        let db = dir.db();
        let broken = b"{ this is not json";
        fs::write(&db, broken).expect("写坏文件");

        let store = Store::open(&db).expect("坏文件也要能打开");
        assert!(store.bans().is_empty());
        assert!(store.profile(1).is_none());
        assert_eq!(store.stats(), StoreStats::default());

        // 坏文件被复制成 <path>.bak.<时间戳>，内容原样保留方便排查。
        let backup_name = fs::read_dir(&dir.path)
            .expect("列临时目录")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with("db.json.bak."))
            .expect("应当留下备份文件");
        let backup = fs::read(dir.path.join(&backup_name)).expect("读备份");
        assert_eq!(backup, broken.to_vec());

        // 空库已经写回正式路径：再打开一次不该又多一份备份。
        drop(store);
        let store = Store::open(&db).expect("重开空库");
        store.ban(1, "广告", 1, false).expect("重建后的库可用");
        assert_eq!(store.bans().len(), 1);
        let backups = fs::read_dir(&dir.path)
            .expect("列临时目录")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("db.json.bak.")
            })
            .count();
        assert_eq!(backups, 1);
    }

    // ------------------------------------------------------------ v0.2 话题

    #[test]
    fn topic_roundtrip_and_reverse_lookup() {
        let dir = TempDir::new("topic");
        let db = dir.db();
        {
            let store = Store::open(&db).expect("打开本地库");
            assert!(store.topic(7).is_none());
            assert!(store.topic_user(42).is_none());

            let mut record = topic(7, 42, "张三");
            record.card_message_id = 900;
            store.set_topic(record).expect("写话题");
            assert_eq!(store.topic(7).map(|record| record.thread_id), Some(42));
            assert_eq!(store.topic_user(42), Some(7));
            assert_eq!(store.topics().len(), 1);

            // 落盘的 JSON 里也要真有 topics：主人手查 / 排障都靠它。
            let raw = fs::read_to_string(&db).expect("读本地库文件");
            let value: serde_json::Value = serde_json::from_str(&raw).expect("应当是合法 JSON");
            assert_eq!(value["topics"]["7"]["thread_id"].as_i64(), Some(42));
        }

        // 重开（模拟重启）能读回全部字段。
        let store = Store::open(&db).expect("重新打开");
        let record = store.topic(7).expect("话题应当读回");
        assert_eq!(record.name, "张三");
        assert_eq!(record.card_message_id, 900);
        assert_eq!(record.created_at, 1_042);
        assert_eq!(store.topic_user(42), Some(7));
    }

    #[test]
    fn same_user_new_thread_replaces_old() {
        let dir = TempDir::new("topic-move");
        let store = Store::open(&dir.db()).expect("打开本地库");
        store.set_topic(topic(7, 42, "旧话题")).expect("写旧话题");
        store.set_topic(topic(7, 99, "新话题")).expect("写新话题");

        assert_eq!(store.topic(7).map(|record| record.thread_id), Some(99));
        // 旧 thread 不能再反查到这个用户，否则主人回复会发错地方。
        assert_eq!(store.topic_user(42), None);
        assert_eq!(store.topic_user(99), Some(7));
        assert_eq!(store.topics().len(), 1);
    }

    #[test]
    fn reusing_thread_drops_previous_owner() {
        let dir = TempDir::new("topic-reuse");
        let store = Store::open(&dir.db()).expect("打开本地库");
        store.set_topic(topic(7, 42, "A")).expect("写话题");
        // TDLib 删话题后 id 可能被复用：新用户拿到同一个 thread_id 时，
        // 旧记录必须清掉，topic_user 只能有一个答案。
        store.set_topic(topic(8, 42, "B")).expect("复用话题");
        assert_eq!(store.topic_user(42), Some(8));
        assert!(store.topic(7).is_none());
        assert_eq!(store.topics().len(), 1);
    }

    #[test]
    fn remove_topic_reports_whether_removed() {
        let dir = TempDir::new("topic-remove");
        let store = Store::open(&dir.db()).expect("打开本地库");
        store.set_topic(topic(7, 42, "A")).expect("写话题");
        assert!(store.remove_topic(7).expect("删话题"));
        assert!(store.topic(7).is_none());
        assert_eq!(store.topic_user(42), None);
        assert!(!store.remove_topic(7).expect("重复删除返回 false"));
    }

    #[test]
    fn stats_counts_topics() {
        let dir = TempDir::new("topic-stats");
        let store = Store::open(&dir.db()).expect("打开本地库");
        assert_eq!(store.stats().topics, 0);
        store.set_topic(topic(1, 10, "A")).expect("写话题");
        store.set_topic(topic(2, 20, "B")).expect("写话题");
        assert_eq!(store.stats().topics, 2);
        assert!(store.remove_topic(1).expect("删话题"));
        assert_eq!(store.stats().topics, 1);
    }

    #[test]
    fn old_store_file_without_topics_key_still_loads() {
        let dir = TempDir::new("topic-legacy");
        let db = dir.db();
        // v0.1 的库：没有 topics 键，但有真实数据——绝不能当成坏文件备份掉。
        let legacy = r#"{
  "version": 1,
  "bans": {
    "11": { "user_id": 11, "at": 100, "reason": "广告", "by": 1, "auto": false }
  },
  "warns": {},
  "profiles": {},
  "reports": {},
  "last_notice": {}
}"#;
        fs::write(&db, legacy).expect("写旧格式库");
        let store = Store::open(&db).expect("旧库必须能直接读");
        assert!(store.topics().is_empty());
        assert!(store.topic(11).is_none());
        assert_eq!(store.stats().topics, 0);
        assert_eq!(
            store.is_banned(11).map(|record| record.reason),
            Some("广告".to_owned())
        );
        // 没有生成 .bak.*，说明 open 认为文件是合法的（只有重建才会备份）。
        let backups = fs::read_dir(&dir.path)
            .expect("列临时目录")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("db.json.bak.")
            })
            .count();
        assert_eq!(backups, 0);

        // 给旧用户补上话题再落盘，重开后旧数据与新话题共存。
        store
            .set_topic(topic(11, 42, "张三"))
            .expect("给旧用户建话题");
        drop(store);
        let store = Store::open(&db).expect("重新打开");
        assert_eq!(store.topic_user(42), Some(11));
        assert!(store.is_banned(11).is_some());
    }
}
