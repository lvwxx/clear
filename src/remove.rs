//! 删除方式：移到废纸篓，或永久删除。

use std::path::Path;

use anyhow::{Context, Result};

pub trait Remover {
    fn remove(&self, path: &Path) -> Result<()>;
}

/// 移到废纸篓，走 Finder 接口，支持「放回原处」。
///
/// 设置了环境变量 `CLR_TRASH_DIR` 时改为移动到该目录，供集成测试使用，避免碰真实废纸篓。
pub struct TrashRemover;

impl Remover for TrashRemover {
    fn remove(&self, path: &Path) -> Result<()> {
        if let Some(dir) = std::env::var_os("CLR_TRASH_DIR") {
            let name = path.file_name().context("路径没有文件名")?;
            let mut dest = Path::new(&dir).join(name);
            let mut n = 1;
            while dest.symlink_metadata().is_ok() {
                dest = Path::new(&dir).join(format!("{}.{n}", name.to_string_lossy()));
                n += 1;
            }
            return std::fs::rename(path, &dest).map_err(Into::into);
        }
        trash::delete(path).map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// 永久删除。目录用 remove_dir_all（它不会跟随内部的符号链接），符号链接只删链接本身。
pub struct PermanentRemover;

impl Remover for PermanentRemover {
    fn remove(&self, path: &Path) -> Result<()> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn permanent_removes_link_not_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep"), "x").unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(&tree).unwrap();
        std::os::unix::fs::symlink(&target, tree.join("link")).unwrap();

        PermanentRemover.remove(&tree).unwrap();
        assert!(!tree.exists());
        assert!(target.join("keep").exists());
    }
}
