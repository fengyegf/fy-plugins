//! skills 目录：**广告判定**的提示词，文件缺失时用内置兜底。
//!
//! 为什么文件 + 内置两份：
//! - 文件（`root/ad.md⟧）让主人不改代码就能调判定口径；
//! - 内置（`include_str!⟧ 编进二进制）保证插件拷到哪台机器都能跑：目录不存在、
//!   文件被删、文件被清空，一律回落内置，绝不会因为提示词缺失让广告判定变成空 prompt。
//!
//! 为什么构造时就读完并缓存：`Skills⟧ 挂在 `Arc<Ctx>⟧ 上被多条业务线程克隆，
//! 克隆必须 O(1)；提示词用 `Arc<str>⟧ 共享，克隆只加引用计数。
//! 代价是**不热更新**：改完磁盘上的 md 需要重建 `Skills⟧（`Ctx::reload_skills⟧）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const BUILTIN_AD: &str = include_str!("../../skills/ad.md");

/// skills 目录 + 已解析好的广告提示词。
#[derive(Debug, Clone)]
pub struct Skills {
    /// skills 目录。只读：构造后改它不会让缓存失效，请用 [`Skills::new⟧] 重建。
    pub root: PathBuf,
    ad: Arc<str>,
}

impl Skills {
    /// 读一遍目录：有非空文件用文件，否则用内置。构造后不再碰磁盘。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let ad = load(&root, "ad", BUILTIN_AD);
        Self { root, ad }
    }

    /// `root/<name>.md⟧。name 由调用方给固定字面量，不接受外部输入。
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.md"))
    }

    /// 广告判定提示词（skills/ad.md，缺省回落内置）。
    pub fn ad(&self) -> String {
        self.ad.to_string()
    }
}

/// 读一份提示词：文件不存在 / 读不出来 / 只有空白，都回落内置。
fn load(root: &Path, name: &str, fallback: &str) -> Arc<str> {
    let path = root.join(format!("{name}.md"));
    match std::fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => Arc::from(text),
        _ => Arc::from(fallback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// 测试目录放在 workspace 的 `target/⟧ 下：受限环境里系统临时目录可能不可写。
    fn scratch_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("bidibot-test-tmp")
    }

    fn unique_dir() -> PathBuf {
        let path = scratch_root().join(format!(
            "skills-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&path);
        path
    }

    #[test]
    fn builtin_is_used_when_the_file_is_missing() {
        let dir = unique_dir();
        let skills = Skills::new(dir.join("no-such-dir"));
        assert!(skills.ad().contains("广告"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_overrides_builtin() {
        let dir = unique_dir();
        std::fs::write(dir.join("ad.md"), "自定义判定口径").unwrap();
        let skills = Skills::new(dir.clone());
        assert_eq!(skills.ad(), "自定义判定口径");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_file_falls_back_to_builtin() {
        let dir = unique_dir();
        std::fs::write(dir.join("ad.md"), "   \n\t ").unwrap();
        let skills = Skills::new(dir.clone());
        assert_eq!(skills.ad(), BUILTIN_AD);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clone_shares_the_prompt() {
        let dir = unique_dir();
        std::fs::write(dir.join("ad.md"), "共享口径").unwrap();
        let skills = Skills::new(dir.clone());
        let cloned = skills.clone();
        assert!(
            Arc::ptr_eq(&skills.ad, &cloned.ad),
            "克隆应共享同一份提示词"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
