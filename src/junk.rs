//! 按规则库扫描垃圾文件。

use std::collections::HashSet;
use std::path::Path;

use crate::finding::Finding;
use crate::rules::{self, Rule};
use crate::walk::{self, Stats};

/// `categories` 为空时扫描全部类别。同一路径只归入第一条匹配它的规则。
pub fn scan(rules: &[Rule], categories: &[String], home: &Path, stats: &Stats) -> Vec<Finding> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for rule in rules {
        if !categories.is_empty() && !categories.contains(&rule.category) {
            continue;
        }
        stats.stage(if rule.desc.is_empty() {
            rule.id.clone()
        } else {
            rule.desc.clone()
        });
        for pattern in &rule.paths {
            for path in rules::expand(pattern, home) {
                if !seen.insert(path.clone()) {
                    continue;
                }
                stats.at(&path);
                let size = walk::disk_usage(&path, stats);
                if size == 0 {
                    continue;
                }
                out.push(Finding {
                    path,
                    size,
                    category: rule.category.clone(),
                    note: rule.desc.clone(),
                });
            }
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.size));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn rule(id: &str, category: &str, path: &str) -> Rule {
        Rule {
            id: id.into(),
            category: category.into(),
            paths: vec![path.into()],
            desc: id.into(),
        }
    }

    #[test]
    fn first_rule_wins_and_empty_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join("C/brew")).unwrap();
        fs::create_dir_all(home.join("C/app")).unwrap();
        fs::create_dir_all(home.join("C/empty")).unwrap();
        fs::write(home.join("C/brew/pkg"), vec![0u8; 8192]).unwrap();
        fs::write(home.join("C/app/data"), vec![0u8; 4096]).unwrap();

        let rules = [
            rule("brew", "dev", "~/C/brew"),
            rule("all", "cache", "~/C/*"),
        ];
        let found = scan(&rules, &[], home, &Stats::default());
        let got: Vec<_> = found
            .iter()
            .map(|f| {
                (
                    f.path.strip_prefix(home).unwrap().to_str().unwrap(),
                    f.category.as_str(),
                )
            })
            .collect();
        assert_eq!(got, [("C/brew", "dev"), ("C/app", "cache")]);

        let only_cache = scan(&rules, &["cache".into()], home, &Stats::default());
        assert_eq!(only_cache.len(), 2);
        assert!(only_cache.iter().all(|f| f.category == "cache"));
    }
}
