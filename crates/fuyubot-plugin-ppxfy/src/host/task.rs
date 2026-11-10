//! 聊天级任务表：一个聊天同一时间只允许一条流程在跑，`/cancel` 靠它定位。
//!
//! 取消 = 置位 [`Task::cancel_flag`] + 执行注册时给的清理钩子。所有等待点
//! （链接、密码、编号、包名）都会在时间片里观察这个标志，因此取消是即时的。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// 取消时执行的清理逻辑（删下载 / 解压 / 打包的产物）。
pub type Cleanup = Box<dyn FnOnce() + Send>;

/// 一条活跃流程。
pub struct Task {
    chat_id: i64,
    cancelled: AtomicBool,
    /// 当前流程的交互消息 id，取消后仍可用于提示。
    tips_message_id: AtomicI64,
    cleanup: Mutex<Option<Cleanup>>,
}

impl Task {
    fn new(chat_id: i64, cleanup: Cleanup) -> Self {
        Self {
            chat_id,
            cancelled: AtomicBool::new(false),
            tips_message_id: AtomicI64::new(0),
            cleanup: Mutex::new(Some(cleanup)),
        }
    }

    pub fn chat_id(&self) -> i64 {
        self.chat_id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 记录交互消息 id，便于取消时把提示改成「已取消」。
    pub fn set_tips(&self, message_id: i64) {
        self.tips_message_id.store(message_id, Ordering::SeqCst);
    }

    pub fn tips(&self) -> i64 {
        self.tips_message_id.load(Ordering::SeqCst)
    }

    /// 置位取消标志并执行清理钩子（钩子只会执行一次）。
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(cleanup) = cleanup {
            cleanup();
        }
    }
}

/// 任务表。
#[derive(Default)]
pub struct Tasks {
    active: Mutex<HashMap<i64, Arc<Task>>>,
}

impl Tasks {
    /// 登记新任务。同一聊天若已有任务，先把它取消（隐式结束上一条）。
    pub fn register(&self, chat_id: i64, cleanup: Cleanup) -> Arc<Task> {
        let task = Arc::new(Task::new(chat_id, cleanup));
        let previous = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(chat_id, task.clone());
        // 清理可能要删一堆文件，放到锁外做，别把整张表堵住。
        if let Some(previous) = previous {
            previous.cancel();
        }
        task
    }

    /// 正常结束：只移除自己那一条，避免误删后来登记的任务。
    pub fn end(&self, task: &Arc<Task>) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let is_same = active
            .get(&task.chat_id())
            .is_some_and(|current| Arc::ptr_eq(current, task));
        if is_same {
            active.remove(&task.chat_id());
        }
    }

    /// 强制移除登记（不跑清理钩子）；线程 panic 后由兜底逻辑调用。
    pub fn force_end(&self, chat_id: i64) {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&chat_id);
    }

    /// 主动取消；没有活跃任务或已被取消时返回 `None`。
    pub fn cancel(&self, chat_id: i64) -> Option<Arc<Task>> {
        let task = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&chat_id)
            .cloned()?;
        if task.is_cancelled() {
            return None;
        }
        task.cancel();
        Some(task)
    }
}
