//! 清理规则：解析内置与用户规则、合并、展开 `~` 和 glob。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

const BUILTIN: &str = include_str!("rules.toml");

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Rule {
    pub id: String,
    pub category: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub desc: String,
}

#[derive(Deserialize)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

pub fn parse(text: &str) -> Result<Vec<Rule>> {
    Ok(toml::from_str::<RuleFile>(text)?.rule)
}

/// 用户规则按 `id` 覆盖内置规则（保留原位置），新 `id` 追加到末尾。
pub fn merge(mut base: Vec<Rule>, extra: Vec<Rule>) -> Vec<Rule> {
    for rule in extra {
        match base.iter_mut().find(|r| r.id == rule.id) {
            Some(existing) => *existing = rule,
            None => base.push(rule),
        }
    }
    base
}

/// 内置规则，加上 `<home>/.config/clear/rules.toml`（如果存在）。
pub fn load(home: &Path) -> Result<Vec<Rule>> {
    let builtin = parse(BUILTIN).context("内置规则解析失败")?;
    let user_file = home.join(".config/clear/rules.toml");
    if !user_file.exists() {
        return Ok(builtin);
    }
    let text = std::fs::read_to_string(&user_file)
        .with_context(|| format!("读取 {} 失败", user_file.display()))?;
    let extra = parse(&text).with_context(|| format!("解析 {} 失败", user_file.display()))?;
    Ok(merge(builtin, extra))
}

/// 把规则里的一条路径模式展开成实际存在的路径。HOME 部分会被转义，
/// 避免 HOME 里的 `[`、`*` 等字符被当作 glob 语法。
pub fn expand(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let full = match pattern.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", glob::Pattern::escape(&home.to_string_lossy())),
        None => pattern.to_string(),
    };
    let Ok(paths) = glob::glob(&full) else {
        return Vec::new();
    };
    paths.flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, category: &str) -> Rule {
        Rule {
            id: id.into(),
            category: category.into(),
            paths: vec![],
            desc: String::new(),
        }
    }

    #[test]
    fn builtin_parses() {
        let rules = parse(BUILTIN).unwrap();
        assert!(rules.iter().any(|r| r.id == "xcode-derived"));
        // 泛化的缓存规则必须排在最后，否则会抢走具体规则的路径
        assert_eq!(rules.last().unwrap().id, "user-caches");
    }

    #[test]
    fn merge_overrides_and_appends() {
        let base = vec![rule("a", "x"), rule("b", "x")];
        let merged = merge(base, vec![rule("a", "y"), rule("c", "z")]);
        let ids: Vec<_> = merged
            .iter()
            .map(|r| (r.id.as_str(), r.category.as_str()))
            .collect();
        assert_eq!(ids, [("a", "y"), ("b", "x"), ("c", "z")]);
    }

    #[test]
    fn glob_expands_and_escapes_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("we[ird]");
        std::fs::create_dir_all(home.join("c/one")).unwrap();
        std::fs::create_dir_all(home.join("c/two")).unwrap();
        let mut found = expand("~/c/*", &home);
        found.sort();
        assert_eq!(found, [home.join("c/one"), home.join("c/two")]);
        assert!(expand("~/missing/*", &home).is_empty());
    }
}
