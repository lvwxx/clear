//! 并行遍历文件系统：不跟随符号链接，硬链接只计一次，无权限的路径跳过并计数。

use std::collections::HashSet;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use jwalk::WalkDir;

/// 扫描过程中的统计，目前只记录因权限等原因跳过的路径数。
#[derive(Default)]
pub struct Stats {
    skipped: AtomicUsize,
}

impl Stats {
    pub fn skip(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }

    pub fn skipped(&self) -> usize {
        self.skipped.load(Ordering::Relaxed)
    }
}

/// 一个普通文件的信息。
pub struct FileEntry {
    pub path: PathBuf,
    /// 文件内容长度
    pub len: u64,
    /// 实际占用磁盘空间
    pub disk: u64,
    pub dev: u64,
    pub ino: u64,
    pub accessed: SystemTime,
    pub modified: SystemTime,
}

/// 实际占用的磁盘空间。稀疏文件（如虚拟机磁盘）的 len 会远大于它。
pub fn disk_bytes(meta: &Metadata) -> u64 {
    meta.blocks() * 512
}

/// 目录遍历时要跳过（不进入）的目录。
#[derive(Clone, Default)]
pub struct Skip {
    pub dirs: Vec<PathBuf>,
    pub git: bool,
}

impl Skip {
    fn matches(&self, path: &Path) -> bool {
        (self.git && path.file_name().is_some_and(|n| n == ".git"))
            || self.dirs.iter().any(|d| d == path)
    }
}

/// 路径的递归磁盘占用。文件或符号链接返回自身占用；读不到的路径返回 0 并计入跳过。
pub fn disk_usage(path: &Path, stats: &Stats) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        stats.skip();
        return 0;
    };
    if !meta.is_dir() {
        return disk_bytes(&meta);
    }
    let mut seen = HashSet::new();
    let mut total = 0;
    for entry in walker(path, meta.dev(), Skip::default()) {
        let Ok(entry) = entry else {
            stats.skip();
            continue;
        };
        if entry.file_type().is_dir() {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            stats.skip();
            continue;
        };
        if meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino())) {
            continue;
        }
        total += disk_bytes(&meta);
    }
    total
}

/// 递归列出 root 下的所有普通文件（不含符号链接），跳过 `skip` 指定的目录。
pub fn files(root: &Path, skip: &Skip, stats: &Stats) -> Vec<FileEntry> {
    let Ok(root_meta) = std::fs::symlink_metadata(root) else {
        stats.skip();
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in walker(root, root_meta.dev(), skip.clone()) {
        let Ok(entry) = entry else {
            stats.skip();
            continue;
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            stats.skip();
            continue;
        };
        out.push(FileEntry {
            len: meta.len(),
            disk: disk_bytes(&meta),
            dev: meta.dev(),
            ino: meta.ino(),
            accessed: meta.accessed().unwrap_or(SystemTime::UNIX_EPOCH),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            path,
        });
    }
    out
}

/// 路径所在的设备号，读不到时返回 None。
pub fn device(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path).ok().map(|m| m.dev())
}

/// 不跟随符号链接、不跨文件系统（类似 `du -x`）的遍历器，
/// 不进入 `skip` 指定的目录。像 ~/OrbStack 这类挂载点里是别的文件系统，不该算进来。
fn walker(root: &Path, root_dev: u64, skip: Skip) -> WalkDir {
    WalkDir::new(root)
        .follow_links(false)
        .skip_hidden(false)
        .process_read_dir(move |_, _, _, children| {
            for child in children.iter_mut().flatten() {
                if !child.file_type().is_dir() {
                    continue;
                }
                let path = child.path();
                if skip.matches(&path) || device(&path).is_some_and(|d| d != root_dev) {
                    child.read_children = None;
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn usage_counts_hardlinks_once_and_ignores_symlink_targets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/a"), vec![1u8; 64 * 1024]).unwrap();
        fs::hard_link(root.join("sub/a"), root.join("b")).unwrap();

        let outside = dir.path().join("outside");
        fs::write(&outside, vec![1u8; 1024 * 1024]).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let stats = Stats::default();
        let one = disk_usage(&root.join("sub/a"), &stats);
        let total = disk_usage(&root, &stats);
        assert!(one >= 64 * 1024);
        // 硬链接只算一次，符号链接指向的 1MB 文件不计入
        assert!(total < one + 64 * 1024, "total={total} one={one}");
        assert_eq!(stats.skipped(), 0);
    }

    #[test]
    fn files_skips_dirs_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("keep")).unwrap();
        fs::create_dir_all(root.join("proj/.git")).unwrap();
        fs::create_dir_all(root.join("skipme")).unwrap();
        fs::write(root.join("keep/a"), "a").unwrap();
        fs::write(root.join("proj/.git/obj"), "g").unwrap();
        fs::write(root.join("skipme/b"), "b").unwrap();
        std::os::unix::fs::symlink(root.join("keep/a"), root.join("link")).unwrap();

        let skip = Skip {
            dirs: vec![root.join("skipme")],
            git: true,
        };
        let names: Vec<_> = files(root, &skip, &Stats::default())
            .into_iter()
            .map(|f| f.path.strip_prefix(root).unwrap().to_path_buf())
            .collect();
        assert_eq!(names, [PathBuf::from("keep/a")]);
    }

    #[test]
    fn missing_path_is_skipped() {
        let stats = Stats::default();
        assert_eq!(disk_usage(Path::new("/definitely/not/here"), &stats), 0);
        assert_eq!(stats.skipped(), 1);
    }
}
