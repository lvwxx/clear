//! 大文件与目录占用分析。

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::finding::Finding;
use crate::walk::{self, Skip, Stats};

/// 占用 ≥ `min` 的文件，按大小降序。`older` 过滤掉最近访问过的文件。
pub fn files(
    root: &Path,
    min: u64,
    older: Option<Duration>,
    skip: &Skip,
    stats: &Stats,
) -> Vec<Finding> {
    let cutoff = older.and_then(|d| SystemTime::now().checked_sub(d));
    let mut out: Vec<Finding> = walk::files(root, skip, stats)
        .into_iter()
        .filter(|f| f.disk >= min)
        .filter(|f| cutoff.is_none_or(|c| f.accessed <= c))
        .map(|f| Finding {
            path: f.path,
            size: f.disk,
            category: "large".into(),
            note: format!("{} 天前访问", days_since(f.accessed)),
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.size));
    out
}

/// root 下占用最多的前 `top` 个直接子项（目录或文件）。
pub fn dirs(root: &Path, top: usize, skip: &Skip, stats: &Stats) -> Vec<Finding> {
    let Ok(entries) = std::fs::read_dir(root) else {
        stats.skip();
        return Vec::new();
    };
    let root_dev = walk::device(root);
    let mut out: Vec<Finding> = entries
        .flatten()
        .map(|e| e.path())
        // 不统计挂载在这里的其他文件系统
        .filter(|p| walk::device(p) == root_dev)
        .filter(|p| {
            !(skip.dirs.contains(p) || skip.git && p.file_name().is_some_and(|n| n == ".git"))
        })
        .map(|path| Finding {
            size: walk::disk_usage(&path, stats),
            category: if path.is_dir() { "dir" } else { "file" }.into(),
            note: String::new(),
            path,
        })
        .filter(|f| f.size > 0)
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.size));
    out.truncate(top);
    out
}

fn days_since(t: SystemTime) -> u64 {
    SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, FileTimes};

    #[test]
    fn threshold_and_age() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("big-old"), vec![1u8; 2 << 20]).unwrap();
        fs::write(root.join("big-new"), vec![1u8; 2 << 20]).unwrap();
        fs::write(root.join("small"), vec![1u8; 1024]).unwrap();
        let old = SystemTime::now() - Duration::from_secs(200 * 86_400);
        File::options()
            .write(true)
            .open(root.join("big-old"))
            .unwrap()
            .set_times(FileTimes::new().set_accessed(old))
            .unwrap();

        let stats = Stats::default();
        let all = files(root, 1 << 20, None, &Skip::default(), &stats);
        assert_eq!(all.len(), 2);

        let aged = files(
            root,
            1 << 20,
            Some(Duration::from_secs(90 * 86_400)),
            &Skip::default(),
            &stats,
        );
        assert_eq!(aged.len(), 1);
        assert!(aged[0].path.ends_with("big-old"));
    }

    #[test]
    fn top_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (name, size) in [("a", 3 << 20), ("b", 1 << 20), ("c", 2 << 20)] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("f"), vec![1u8; size]).unwrap();
        }
        let got: Vec<_> = dirs(root, 2, &Skip::default(), &Stats::default())
            .into_iter()
            .map(|f| f.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(got, ["a", "c"]);
    }
}
