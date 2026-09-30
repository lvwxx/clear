//! 重复文件查找：大小分组 → 头部 4KB 哈希 → 全量 BLAKE3 哈希。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use rayon::prelude::*;

use crate::finding::Finding;
use crate::walk::{self, FileEntry, Skip, Stats};

const HEAD: usize = 4096;

pub fn scan(root: &Path, min: u64, skip: &Skip, stats: &Stats) -> Vec<Finding> {
    // 1. 按大小分组；同一 inode（硬链接）只保留一份
    stats.stage("遍历文件");
    let mut by_len: HashMap<u64, Vec<FileEntry>> = HashMap::new();
    let mut inodes = HashSet::new();
    for f in walk::files(root, skip, stats) {
        if f.len >= min.max(1) && inodes.insert((f.dev, f.ino)) {
            by_len.entry(f.len).or_default().push(f);
        }
    }
    let candidates: Vec<Vec<FileEntry>> = by_len.into_values().filter(|g| g.len() > 1).collect();

    // 2. 头部哈希；3. 全量哈希（文件不超过 4KB 时头部哈希就是全量）
    stats.restart("比较文件内容");
    let groups: Vec<Vec<FileEntry>> = candidates
        .into_par_iter()
        .flat_map(|group| split_by(group, |f| hash_head(&f.path), stats))
        .flat_map(|group| {
            if group[0].len as usize <= HEAD {
                vec![group]
            } else {
                split_by(group, |f| hash_full(&f.path), stats)
            }
        })
        .collect();

    let mut groups: Vec<(u64, Vec<FileEntry>)> = groups
        .into_iter()
        .map(|mut g| {
            g.sort_by(keep_order);
            let reclaim = g[1..].iter().map(|f| f.disk).sum();
            (reclaim, g)
        })
        .collect();
    groups.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1[0].path.cmp(&b.1[0].path)));

    let mut out = Vec::new();
    for (n, (_, group)) in groups.into_iter().enumerate() {
        let keep = group[0].path.display().to_string();
        for f in group.into_iter().skip(1) {
            out.push(Finding {
                path: f.path,
                size: f.disk,
                category: "dupe".into(),
                note: format!("组 {}，保留 {keep}", n + 1),
            });
        }
    }
    out
}

/// 保留规则：路径最短者优先，其次修改时间最早，最后按路径字典序保证结果稳定。
fn keep_order(a: &FileEntry, b: &FileEntry) -> std::cmp::Ordering {
    let len = |f: &FileEntry| f.path.as_os_str().len();
    len(a)
        .cmp(&len(b))
        .then_with(|| a.modified.cmp(&b.modified))
        .then_with(|| a.path.cmp(&b.path))
}

/// 按哈希把一组文件拆成若干组，只保留成员多于一个的组。读失败的文件计入跳过。
fn split_by(
    group: Vec<FileEntry>,
    hash: impl Fn(&FileEntry) -> std::io::Result<blake3::Hash> + Sync,
    stats: &Stats,
) -> Vec<Vec<FileEntry>> {
    let mut by_hash: HashMap<blake3::Hash, Vec<FileEntry>> = HashMap::new();
    for f in group {
        stats.at(&f.path);
        stats.file();
        match hash(&f) {
            Ok(h) => by_hash.entry(h).or_default().push(f),
            Err(_) => stats.skip(),
        }
    }
    by_hash.into_values().filter(|g| g.len() > 1).collect()
}

fn hash_head(path: &Path) -> std::io::Result<blake3::Hash> {
    let mut buf = Vec::with_capacity(HEAD);
    File::open(path)?.take(HEAD as u64).read_to_end(&mut buf)?;
    Ok(blake3::hash(&buf))
}

fn hash_full(path: &Path) -> std::io::Result<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(File::open(path)?)?;
    Ok(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_true_duplicates_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let content = vec![7u8; 10_000];
        let mut other = content.clone();
        *other.last_mut().unwrap() = 8; // 大小与头部相同，只有尾部不同

        fs::create_dir_all(root.join("deep/er")).unwrap();
        fs::write(root.join("a.bin"), &content).unwrap();
        fs::write(root.join("deep/er/a-copy.bin"), &content).unwrap();
        fs::write(root.join("near.bin"), &other).unwrap();
        fs::hard_link(root.join("a.bin"), root.join("hard.bin")).unwrap();
        fs::write(root.join("tiny1"), "same").unwrap();
        fs::write(root.join("tiny2"), "same").unwrap();

        let found = scan(root, 1024, &Skip::default(), &Stats::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].path.ends_with("deep/er/a-copy.bin"));
        assert!(!found[0].note.contains("a-copy"));

        // 不设下限时，小文件也参与比较
        let all = scan(root, 0, &Skip::default(), &Stats::default());
        assert_eq!(all.len(), 2);
    }
}
