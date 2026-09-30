//! 删除选中的项目：逐项执行，单项失败不影响其他项。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::finding::Finding;
use crate::remove::Remover;
use crate::safety;

#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    pub removed: usize,
    pub freed: u64,
    pub failed: Vec<Failure>,
}

#[derive(Debug, Serialize)]
pub struct Failure {
    pub path: PathBuf,
    pub error: String,
}

impl Outcome {
    /// 有 /Library 下的项目删除失败时，提示用 sudo 重跑。
    pub fn needs_sudo(&self) -> bool {
        self.failed.iter().any(|f| f.path.starts_with("/Library/"))
    }

    /// 应用的数据容器受 macOS 保护，只有「完全磁盘访问权限」能删，sudo 也不行。
    pub fn needs_full_disk_access(&self, home: &Path) -> bool {
        let library = home.join("Library");
        self.failed.iter().any(|f| {
            f.path.starts_with(library.join("Containers"))
                || f.path.starts_with(library.join("Group Containers"))
        })
    }
}

pub fn execute(items: &[&Finding], remover: &dyn Remover, home: &Path) -> Outcome {
    let mut outcome = Outcome::default();
    for item in items {
        let path = &item.path;
        if safety::is_protected_resolved(path, home) {
            outcome.failed.push(Failure {
                path: path.clone(),
                error: "受保护路径，拒绝删除".into(),
            });
            continue;
        }
        // 已被前面删除的父目录一并带走
        if path.symlink_metadata().is_err() {
            continue;
        }
        match remover.remove(path) {
            Ok(()) => {
                outcome.removed += 1;
                outcome.freed += item.size;
            }
            Err(e) => outcome.failed.push(Failure {
                path: path.clone(),
                error: format!("{e:#}"),
            }),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<PathBuf>>);

    impl Remover for Recorder {
        fn remove(&self, path: &Path) -> anyhow::Result<()> {
            if path.ends_with("fail") {
                anyhow::bail!("boom");
            }
            self.0.borrow_mut().push(path.to_path_buf());
            Ok(())
        }
    }

    fn finding(path: PathBuf, size: u64) -> Finding {
        Finding {
            path,
            size,
            category: "t".into(),
            note: String::new(),
        }
    }

    #[test]
    fn removes_blocks_protected_and_continues_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for name in ["a", "fail", "b"] {
            std::fs::write(home.join(name), "x").unwrap();
        }
        std::fs::create_dir_all(home.join("Documents")).unwrap();

        let items = [
            finding(home.join("a"), 10),
            finding(home.join("Documents"), 99),
            finding(home.join("fail"), 20),
            finding(home.join("gone"), 5),
            finding(home.join("b"), 30),
        ];
        let refs: Vec<&Finding> = items.iter().collect();
        let recorder = Recorder::default();
        let outcome = execute(&refs, &recorder, home);

        assert_eq!(*recorder.0.borrow(), [home.join("a"), home.join("b")]);
        assert_eq!(outcome.removed, 2);
        assert_eq!(outcome.freed, 40);
        let failed: Vec<_> = outcome.failed.iter().map(|f| f.path.clone()).collect();
        assert_eq!(failed, [home.join("Documents"), home.join("fail")]);
        assert!(!outcome.needs_sudo());
    }
}
