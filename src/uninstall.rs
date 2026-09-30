//! 应用卸载：定位 .app，读 Bundle ID，精确匹配 Library 下的残留文件。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::finding::Finding;
use crate::walk::{self, Stats};

/// 在这些 `~/Library` 子目录里按 Bundle ID 匹配残留。
const USER_LIBRARY_DIRS: &[&str] = &[
    "Application Support",
    "Preferences",
    "Preferences/ByHost",
    "Caches",
    "Containers",
    "Group Containers",
    "Saved Application State",
    "LaunchAgents",
    "HTTPStorages",
    "WebKit",
];

/// 这些目录里还会额外按 App 名称匹配（很多 App 用名称而非 Bundle ID 建目录）。
const NAME_MATCH_DIRS: &[&str] = &["Application Support", "Caches"];

const SYSTEM_DIRS: &[&str] = &["/Library/LaunchAgents", "/Library/LaunchDaemons"];

pub struct App {
    pub path: PathBuf,
    pub name: String,
    pub bundle_id: String,
}

/// 参数是路径（含 `/` 或以 `.app` 结尾）时直接使用，否则在
/// `/Applications` 和 `~/Applications` 中按名称查找（忽略大小写，可省略 `.app`）。
pub fn locate(arg: &str, home: &Path) -> Result<App> {
    let path = if arg.contains('/') || arg.ends_with(".app") && Path::new(arg).exists() {
        std::path::absolute(arg)?
    } else {
        find_by_name(arg, home)?
    };
    if !path.join("Contents/Info.plist").is_file() {
        bail!(
            "{} 不是有效的应用（缺少 Contents/Info.plist）",
            path.display()
        );
    }
    let name = path
        .file_stem()
        .context("无效的应用路径")?
        .to_string_lossy()
        .into_owned();
    let bundle_id = read_bundle_id(&path)?;
    Ok(App {
        path,
        name,
        bundle_id,
    })
}

fn find_by_name(arg: &str, home: &Path) -> Result<PathBuf> {
    let wanted = arg.trim_end_matches(".app").to_lowercase();
    for dir in [PathBuf::from("/Applications"), home.join("Applications")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_match = path.extension().is_some_and(|e| e == "app")
                && path
                    .file_stem()
                    .is_some_and(|s| s.to_string_lossy().to_lowercase() == wanted);
            if is_match {
                return Ok(path);
            }
        }
    }
    bail!("在 /Applications 和 ~/Applications 中找不到应用「{arg}」")
}

fn read_bundle_id(app: &Path) -> Result<String> {
    let plist_path = app.join("Contents/Info.plist");
    let value = plist::Value::from_file(&plist_path)
        .with_context(|| format!("解析 {} 失败", plist_path.display()))?;
    value
        .as_dictionary()
        .and_then(|d| d.get("CFBundleIdentifier"))
        .and_then(|v| v.as_string())
        .map(str::to_owned)
        .with_context(|| format!("{} 中没有 CFBundleIdentifier", plist_path.display()))
}

pub fn is_running(app: &App) -> bool {
    let pattern = format!(
        "{}/Contents/MacOS/",
        regex_escape(&app.path.to_string_lossy())
    );
    Command::new("pgrep")
        .args(["-f", &pattern])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 应用本体加上所有残留文件。
pub fn scan(app: &App, home: &Path, stats: &Stats) -> Vec<Finding> {
    let mut out = vec![Finding {
        path: app.path.clone(),
        size: walk::disk_usage(&app.path, stats),
        category: "app".into(),
        note: app.bundle_id.clone(),
    }];
    let library = home.join("Library");
    let dirs = USER_LIBRARY_DIRS
        .iter()
        .map(|d| (library.join(d), NAME_MATCH_DIRS.contains(d)))
        .chain(SYSTEM_DIRS.iter().map(|d| (PathBuf::from(d), false)));
    for (dir, by_name) in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut matched: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                is_leftover(&name, &app.bundle_id, by_name.then_some(app.name.as_str()))
            })
            .collect();
        matched.sort();
        for path in matched {
            out.push(Finding {
                size: walk::disk_usage(&path, stats),
                category: "app-leftover".into(),
                note: dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path,
            });
        }
    }
    out
}

/// 文件名等于 Bundle ID、以 `<BundleID>.` 开头，或以 `.<BundleID>` 结尾（Group Containers
/// 的 `<TeamID>.<BundleID>` 形式）时算残留；给了 `app_name` 时，文件名等于它也算。忽略大小写。
fn is_leftover(file_name: &str, bundle_id: &str, app_name: Option<&str>) -> bool {
    let name = file_name.to_lowercase();
    let bid = bundle_id.to_lowercase();
    name == bid
        || name.starts_with(&format!("{bid}."))
        || name.ends_with(&format!(".{bid}"))
        || app_name.is_some_and(|n| name == n.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftover_matching_is_exact() {
        let bid = "com.acme.Foo";
        assert!(is_leftover("com.acme.Foo", bid, None));
        assert!(is_leftover("com.acme.foo.plist", bid, None));
        assert!(is_leftover("com.acme.Foo.savedState", bid, None));
        assert!(is_leftover("ABCDE12345.com.acme.Foo", bid, None));
        assert!(is_leftover("Foo", bid, Some("Foo")));

        assert!(!is_leftover("com.acme.FooBar", bid, None));
        assert!(!is_leftover("com.acme.FooBar.plist", bid, None));
        assert!(!is_leftover("Foo", bid, None));
        assert!(!is_leftover("FooHelper", bid, Some("Foo")));
    }

    #[test]
    fn escapes_regex() {
        assert_eq!(regex_escape("/A (1).app"), r"/A \(1\)\.app");
    }
}
