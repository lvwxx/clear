//! 项目构建产物：target、node_modules、.venv 等。只有旁边有对应的项目标志文件时才算，
//! 避免误删碰巧同名的普通目录。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use jwalk::WalkDir;

use crate::finding::Finding;
use crate::walk::{self, Skip, Stats};

enum Marker {
    /// 同级目录里有其中任一文件
    Sibling(&'static [&'static str]),
    /// 目录自身里有这个文件
    Inside(&'static str),
}

struct Kind {
    category: &'static str,
    dirs: &'static [&'static str],
    marker: Marker,
}

const KINDS: &[Kind] = &[
    Kind {
        category: "rust",
        dirs: &["target"],
        marker: Marker::Sibling(&["Cargo.toml"]),
    },
    Kind {
        category: "maven",
        dirs: &["target"],
        marker: Marker::Sibling(&["pom.xml"]),
    },
    Kind {
        category: "node",
        dirs: &[
            "node_modules",
            ".next",
            ".nuxt",
            ".svelte-kit",
            ".turbo",
            ".parcel-cache",
        ],
        marker: Marker::Sibling(&["package.json"]),
    },
    Kind {
        category: "python",
        dirs: &[".venv", "venv"],
        marker: Marker::Inside("pyvenv.cfg"),
    },
    Kind {
        category: "gradle",
        dirs: &["build", ".gradle"],
        marker: Marker::Sibling(&["build.gradle", "build.gradle.kts"]),
    },
    Kind {
        category: "swift",
        dirs: &[".build"],
        marker: Marker::Sibling(&["Package.swift"]),
    },
    Kind {
        category: "cocoapods",
        dirs: &["Pods"],
        marker: Marker::Sibling(&["Podfile"]),
    },
    Kind {
        category: "zig",
        dirs: &["zig-cache", ".zig-cache"],
        marker: Marker::Sibling(&["build.zig"]),
    },
    Kind {
        category: "elixir",
        dirs: &["_build"],
        marker: Marker::Sibling(&["mix.exs"]),
    },
];

/// 在 root 下查找构建产物，按大小降序。`older` 只保留这么久没改动过的项目。
pub fn scan(root: &Path, older: Option<Duration>, skip: &Skip, stats: &Stats) -> Vec<Finding> {
    stats.stage("查找项目");
    let found = find(root, skip, stats);

    stats.restart("统计构建产物大小");
    let cutoff = older.and_then(|d| SystemTime::now().checked_sub(d));
    let mut out = Vec::new();
    for (path, category) in found {
        let project = path.parent().unwrap_or(&path);
        let active = last_modified(project, &path);
        if cutoff.is_some_and(|c| active > c) {
            continue;
        }
        stats.at(&path);
        let size = walk::disk_usage(&path, stats);
        if size == 0 {
            continue;
        }
        let project_name = project.file_name().unwrap_or_default().to_string_lossy();
        out.push(Finding {
            size,
            category: category.into(),
            note: format!("{project_name} · {} 天未改动", days_since(active)),
            path,
        });
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.size));
    out
}

/// 遍历目录树，读每个目录时根据子项名字判断有没有构建产物；找到的不再往里走。
fn find(root: &Path, skip: &Skip, stats: &Stats) -> Vec<(PathBuf, &'static str)> {
    let Some(root_dev) = walk::device(root) else {
        stats.skip();
        return Vec::new();
    };
    let found: Arc<Mutex<Vec<(PathBuf, &'static str)>>> = Arc::default();
    let sink = found.clone();
    let skip = skip.clone();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .skip_hidden(false)
        .process_read_dir(move |_, _, _, children| {
            let names: Vec<String> = children
                .iter()
                .flatten()
                .map(|c| c.file_name().to_string_lossy().into_owned())
                .collect();
            for child in children.iter_mut().flatten() {
                if !child.file_type().is_dir() {
                    continue;
                }
                let path = child.path();
                let name = child.file_name().to_string_lossy();
                if let Some(category) = detect(&name, &names, &path) {
                    sink.lock().unwrap().push((path, category));
                    child.read_children = None;
                } else if skip.matches(&path) || walk::device(&path).is_some_and(|d| d != root_dev)
                {
                    child.read_children = None;
                }
            }
        });
    for entry in walker {
        match entry {
            Ok(e) if e.file_type().is_dir() => stats.at(&e.path()),
            Ok(_) => stats.file(),
            Err(_) => stats.skip(),
        }
    }
    // jwalk 的后台线程可能还持有 Arc 的副本，不能 try_unwrap，直接把内容取走
    std::mem::take(&mut *found.lock().unwrap())
}

fn detect(name: &str, siblings: &[String], path: &Path) -> Option<&'static str> {
    KINDS.iter().find_map(|k| {
        if !k.dirs.contains(&name) {
            return None;
        }
        let ok = match k.marker {
            Marker::Sibling(files) => files.iter().any(|f| siblings.iter().any(|s| s == f)),
            Marker::Inside(file) => path.join(file).is_file(),
        };
        ok.then_some(k.category)
    })
}

/// 项目最近一次改动：项目目录下直接子项（除构建产物本身）的最新修改时间。
fn last_modified(project: &Path, artifact: &Path) -> SystemTime {
    std::fs::read_dir(project)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path() != artifact)
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
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
    use std::fs;

    fn touch(root: &Path, rel: &str, bytes: usize) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn detects_only_with_markers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "rs/Cargo.toml", 10);
        touch(root, "rs/target/debug/app", 5000);
        touch(root, "web/package.json", 10);
        touch(root, "web/node_modules/a/index.js", 5000);
        touch(root, "web/node_modules/a/node_modules/b/index.js", 5000);
        touch(root, "py/.venv/pyvenv.cfg", 10);
        touch(root, "py/.venv/lib/x.py", 5000);
        // 没有标志文件的同名目录
        touch(root, "notes/target/keep.txt", 5000);
        touch(root, "misc/node_modules/keep.js", 5000);
        touch(root, "misc/venv/keep.py", 5000);

        let mut got: Vec<String> = scan(root, None, &Skip::default(), &Stats::default())
            .into_iter()
            .map(|f| {
                format!(
                    "{} {}",
                    f.path.strip_prefix(root).unwrap().display(),
                    f.category
                )
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            ["py/.venv python", "rs/target rust", "web/node_modules node"]
        );
    }

    #[test]
    fn older_filters_active_projects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "rs/Cargo.toml", 10);
        touch(root, "rs/target/debug/app", 5000);
        let recent = scan(
            root,
            Some(Duration::from_secs(86_400)),
            &Skip::default(),
            &Stats::default(),
        );
        assert!(recent.is_empty());
        assert_eq!(
            scan(root, None, &Skip::default(), &Stats::default()).len(),
            1
        );
    }
}
