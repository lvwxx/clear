//! 应用卸载：定位 .app，读 Bundle ID，精确匹配 Library 下的残留文件。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::finding::Finding;
use crate::walk::{self, Stats};

/// 在这些 `~/Library` 子目录里按 Bundle ID 匹配残留。
const USER_LIBRARY_DIRS: &[&str] = &[
    "Application Support",
    "Application Scripts",
    "Preferences",
    "Preferences/ByHost",
    "Caches",
    "Containers",
    "Group Containers",
    "Saved Application State",
    "LaunchAgents",
    "HTTPStorages",
    "WebKit",
    "Logs",
];

/// 这些目录里还会额外按 App 名称匹配（很多 App 用名称而非 Bundle ID 建目录）。
const NAME_MATCH_DIRS: &[&str] = &["Application Support", "Caches", "Logs"];

const SYSTEM_DIRS: &[&str] = &["/Library/LaunchAgents", "/Library/LaunchDaemons"];

/// 崩溃报告目录，文件名形如 `<进程名>_2026-09-25-231025_<主机>.diag`。
const CRASH_REPORT_DIRS: &[&str] = &[
    "~/Library/Logs/DiagnosticReports",
    "/Library/Logs/DiagnosticReports",
];

/// App 包里可能内嵌辅助程序的位置。Frameworks 不算：那里的 Bundle ID（如 Sparkle）是多个 App 共用的。
const HELPER_DIRS: &[&str] = &[
    "Contents/Library/LoginItems",
    "Contents/XPCServices",
    "Contents/PlugIns",
    "Contents/Helpers",
];

#[derive(Clone, Copy, PartialEq)]
enum Match {
    Id,
    IdOrName,
    CrashReport,
}

/// 读本地化显示名时依次尝试的语言目录，中文优先。
const LPROJ_PREFERENCE: &[&str] = &[
    "zh-Hans", "zh_CN", "zh-Hant", "zh_TW", "en", "English", "Base",
];

pub struct App {
    pub path: PathBuf,
    /// Finder 里显示的名称，优先取本地化名，例如 DrCleanerProPlus.app 显示为「Cleaner One Pro」
    pub display: String,
    pub bundle_id: String,
    /// 内嵌辅助程序（登录项、XPC 服务、扩展）的 Bundle ID，它们的残留也要清
    helper_ids: Vec<String>,
    /// 可能被用来命名残留目录的名称：文件名、CFBundleName、可执行文件名
    names: Vec<String>,
    /// 按名称查找时可匹配的所有写法（小写）：文件名、各语言显示名、Bundle ID
    aliases: Vec<String>,
}

/// 参数是路径（含 `/`，或是存在的 `.app`）时直接使用；否则在应用列表里按名称查找，
/// 文件名、Finder 显示名（含本地化名）、Bundle ID 都能匹配，忽略大小写，可省略 `.app`。
pub fn locate(arg: &str, home: &Path) -> Result<App> {
    if arg.contains('/') || arg.ends_with(".app") && Path::new(arg).exists() {
        return read_app(&std::path::absolute(arg)?);
    }
    let wanted = arg.trim_end_matches(".app").to_lowercase();
    let mut matches: Vec<App> = list(home)
        .into_iter()
        .filter(|a| a.aliases.contains(&wanted))
        .collect();
    match matches.len() {
        0 => bail!(
            "在 /Applications 和 ~/Applications 中找不到应用「{arg}」，运行 clr uninstall 查看所有应用"
        ),
        1 => Ok(matches.remove(0)),
        _ => {
            let paths: Vec<String> = matches
                .iter()
                .map(|a| a.path.display().to_string())
                .collect();
            bail!(
                "「{arg}」匹配到多个应用，请改用路径指定：\n  {}",
                paths.join("\n  ")
            )
        }
    }
}

/// `/Applications` 和 `~/Applications` 下的应用，包括其中子文件夹（如 Utilities）里的一层，
/// 按显示名排序。读不出 Info.plist 的跳过。
pub fn list(home: &Path) -> Vec<App> {
    let mut paths = Vec::new();
    for dir in [PathBuf::from("/Applications"), home.join("Applications")] {
        for path in app_bundles(&dir) {
            paths.push(path);
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_folder = !is_app(&path) && entry.file_type().is_ok_and(|t| t.is_dir());
            if is_folder {
                paths.extend(app_bundles(&path));
            }
        }
    }
    let mut apps: Vec<App> = paths.iter().filter_map(|p| read_app(p).ok()).collect();
    apps.sort_by_key(|a| a.display.to_lowercase());
    apps
}

fn is_app(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "app")
}

fn app_bundles(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_app(p))
        .collect()
}

fn read_app(path: &Path) -> Result<App> {
    let plist_path = path.join("Contents/Info.plist");
    if !plist_path.is_file() {
        bail!(
            "{} 不是有效的应用（缺少 Contents/Info.plist）",
            path.display()
        );
    }
    let value = plist::Value::from_file(&plist_path)
        .with_context(|| format!("解析 {} 失败", plist_path.display()))?;
    let info = value.as_dictionary().context("Info.plist 格式不对")?;
    let get = |key: &str| info.get(key).and_then(|v| v.as_string()).map(str::to_owned);
    let bundle_id = get("CFBundleIdentifier")
        .with_context(|| format!("{} 中没有 CFBundleIdentifier", plist_path.display()))?;
    let stem = path
        .file_stem()
        .context("无效的应用路径")?
        .to_string_lossy()
        .into_owned();

    let localized: Vec<String> = LPROJ_PREFERENCE
        .iter()
        .flat_map(|l| {
            localized_names(&path.join(format!("Contents/Resources/{l}.lproj/InfoPlist.strings")))
        })
        .collect();
    let display = localized
        .first()
        .cloned()
        .or_else(|| get("CFBundleDisplayName"))
        .or_else(|| get("CFBundleName"))
        .unwrap_or_else(|| stem.clone());

    let mut names = vec![stem.clone()];
    names.extend(get("CFBundleName"));
    names.extend(get("CFBundleExecutable"));
    names.sort();
    names.dedup();
    let helper_ids = helper_ids(path, &bundle_id);

    let mut aliases: Vec<String> = [
        Some(stem),
        get("CFBundleName"),
        get("CFBundleDisplayName"),
        Some(bundle_id.clone()),
    ]
    .into_iter()
    .flatten()
    .chain(localized)
    .map(|n| n.to_lowercase())
    .collect();
    aliases.sort();
    aliases.dedup();

    Ok(App {
        path: path.to_path_buf(),
        display,
        bundle_id,
        helper_ids,
        names,
        aliases,
    })
}

/// 内嵌辅助程序的 Bundle ID，只保留与主程序同一厂商前缀（前两段，如 `com.trendmicro`）的。
fn helper_ids(app: &Path, main_id: &str) -> Vec<String> {
    let vendor = |id: &str| {
        id.split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".")
            .to_lowercase()
    };
    let main_vendor = vendor(main_id);
    let mut ids: Vec<String> = HELPER_DIRS
        .iter()
        .flat_map(|d| {
            std::fs::read_dir(app.join(d))
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == "app" || e == "appex" || e == "xpc")
        })
        .filter_map(|p| plist::Value::from_file(p.join("Contents/Info.plist")).ok())
        .filter_map(|v| {
            v.as_dictionary()?
                .get("CFBundleIdentifier")?
                .as_string()
                .map(str::to_owned)
        })
        .filter(|id| id != main_id && vendor(id) == main_vendor)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// 从 InfoPlist.strings 里取 CFBundleDisplayName 和 CFBundleName（按这个顺序）。
/// 文件可能是二进制/XML plist，也可能是 UTF-16 或 UTF-8 的 `"key" = "value";` 文本。
fn localized_names(path: &Path) -> Vec<String> {
    const KEYS: [&str; 2] = ["CFBundleDisplayName", "CFBundleName"];
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    if bytes.starts_with(b"bplist") || bytes.starts_with(b"<?xml") {
        let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(&bytes)) else {
            return Vec::new();
        };
        let Some(dict) = value.as_dictionary() else {
            return Vec::new();
        };
        return KEYS
            .iter()
            .filter_map(|k| dict.get(k).and_then(|v| v.as_string()).map(str::to_owned))
            .collect();
    }
    let text = decode_text(&bytes);
    KEYS.iter()
        .filter_map(|k| strings_value(&text, k))
        .collect()
}

fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |data: &[u8], le: bool| {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        // 无 BOM 的 UTF-16LE：ASCII 字符的高字节是 0
        [_, 0, ..] => utf16(bytes, true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 在 `.strings` 文本里找 `key = "value";`，key 可以带引号也可以不带。
fn strings_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix(&format!("\"{key}\""))
            .or_else(|| line.strip_prefix(key))
        else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let mut value = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => value.extend(chars.next()),
                '"' => return Some(value),
                _ => value.push(c),
            }
        }
    }
    None
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
/// 所有应用及其大小，按大小降序。
pub fn list_with_sizes(home: &Path, stats: &Stats) -> Vec<(App, u64)> {
    stats.stage("统计应用大小");
    let mut apps: Vec<(App, u64)> = list(home)
        .into_iter()
        .map(|app| {
            stats.at(&app.path);
            let size = walk::disk_usage(&app.path, stats);
            (app, size)
        })
        .collect();
    apps.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
    apps
}

pub fn app_finding(app: &App, size: u64) -> Finding {
    Finding {
        path: app.path.clone(),
        size,
        category: "app".into(),
        note: format!("{} · {}", app.display, app.bundle_id),
    }
}

pub fn scan(app: &App, home: &Path, stats: &Stats) -> Vec<Finding> {
    stats.stage("统计应用大小");
    let mut out = vec![app_finding(app, walk::disk_usage(&app.path, stats))];
    let ids: Vec<&str> = std::iter::once(app.bundle_id.as_str())
        .chain(app.helper_ids.iter().map(String::as_str))
        .collect();
    out.extend(leftovers(&ids, &app.names, home, stats));
    out
}

/// 应用本体已经删掉时，只按 Bundle ID 找残留。
pub fn scan_orphan(bundle_id: &str, home: &Path, stats: &Stats) -> Vec<Finding> {
    leftovers(&[bundle_id], &[], home, stats)
}

/// 参数看起来像 Bundle ID（`com.vendor.App` 这种，无空格和斜杠）。
pub fn looks_like_bundle_id(arg: &str) -> bool {
    let parts: Vec<&str> = arg.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| !p.is_empty())
        && !arg.contains(|c: char| c.is_whitespace() || c == '/')
}

fn leftovers(ids: &[&str], names: &[String], home: &Path, stats: &Stats) -> Vec<Finding> {
    stats.stage("查找残留文件");
    let mut out = Vec::new();
    let library = home.join("Library");
    let dirs = USER_LIBRARY_DIRS
        .iter()
        .map(|d| {
            let mode = if NAME_MATCH_DIRS.contains(d) {
                Match::IdOrName
            } else {
                Match::Id
            };
            (library.join(d), mode)
        })
        .chain(SYSTEM_DIRS.iter().map(|d| (PathBuf::from(d), Match::Id)))
        .chain(CRASH_REPORT_DIRS.iter().map(|d| {
            let dir = match d.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(d),
            };
            (dir, Match::CrashReport)
        }));
    for (dir, mode) in dirs {
        stats.at(&dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut matched: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                match mode {
                    Match::Id => is_leftover(&name, ids, &[]),
                    Match::IdOrName => is_leftover(&name, ids, names),
                    Match::CrashReport => is_crash_report(&name, names),
                }
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
/// 的 `<TeamID>.<BundleID>` 形式）时算残留；文件名等于 `app_names` 之一也算。忽略大小写。
fn is_leftover(file_name: &str, bundle_ids: &[&str], app_names: &[String]) -> bool {
    let name = file_name.to_lowercase();
    bundle_ids.iter().any(|bid| {
        let bid = bid.to_lowercase();
        name == bid || name.starts_with(&format!("{bid}.")) || name.ends_with(&format!(".{bid}"))
    }) || app_names.iter().any(|n| name == n.to_lowercase())
}

/// 崩溃报告的文件名是「进程名」加 `_` 或 `-` 再接日期，要求分隔符后是数字，
/// 以免 `Foo` 匹配到 `Foo-Helper_2026...`。
fn is_crash_report(file_name: &str, app_names: &[String]) -> bool {
    let name = file_name.to_lowercase();
    app_names.iter().any(|n| {
        name.strip_prefix(&n.to_lowercase())
            .and_then(|rest| rest.strip_prefix(['_', '-']))
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftover_matching_is_exact() {
        let bid = &["com.acme.Foo"];
        assert!(is_leftover("com.acme.Foo", bid, &[]));
        assert!(is_leftover("com.acme.foo.plist", bid, &[]));
        assert!(is_leftover("com.acme.Foo.savedState", bid, &[]));
        assert!(is_leftover("ABCDE12345.com.acme.Foo", bid, &[]));
        assert!(is_leftover("Foo", bid, &["Foo".to_string()]));

        assert!(!is_leftover("com.acme.FooBar", bid, &[]));
        assert!(!is_leftover("com.acme.FooBar.plist", bid, &[]));
        assert!(!is_leftover("Foo", bid, &[]));
        assert!(!is_leftover("FooHelper", bid, &["Foo".to_string()]));
        // 内嵌辅助程序的 ID 也参与匹配
        assert!(is_leftover(
            "com.acme.FooLogin",
            &["com.acme.Foo", "com.acme.FooLogin"],
            &[]
        ));
    }

    #[test]
    fn crash_reports() {
        let names = ["DrFoo".to_string()];
        assert!(is_crash_report("DrFoo_2026-09-25-231025_host.diag", &names));
        assert!(is_crash_report("drfoo-2026-09-25-231025.ips", &names));
        assert!(!is_crash_report("DrFoo-Helper_2026-09-25.ips", &names));
        assert!(!is_crash_report("DrFooBar_2026-09-25.ips", &names));
    }

    #[test]
    fn parses_strings_files() {
        let text = "/* comment */\nCFBundleName = \"Cleaner One\";\n\"CFBundleDisplayName\" = \"A \\\"B\\\"\";\n";
        assert_eq!(
            strings_value(text, "CFBundleName").as_deref(),
            Some("Cleaner One")
        );
        assert_eq!(
            strings_value(text, "CFBundleDisplayName").as_deref(),
            Some("A \"B\"")
        );
        assert_eq!(strings_value(text, "Missing"), None);
        // CFBundleNameX 不能被当成 CFBundleName
        assert_eq!(
            strings_value("CFBundleNameX = \"no\";", "CFBundleName"),
            None
        );
    }

    #[test]
    fn decodes_utf16_with_and_without_bom() {
        let utf16le: Vec<u8> = "名=\"x\""
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let mut with_bom = vec![0xFF, 0xFE];
        with_bom.extend(&utf16le);
        assert_eq!(decode_text(&with_bom), "名=\"x\"");
        let ascii_le: Vec<u8> = "ab".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_text(&ascii_le), "ab");
        assert_eq!(decode_text("中文".as_bytes()), "中文");
    }

    #[test]
    fn bundle_id_shape() {
        assert!(looks_like_bundle_id("com.trendmicro.DrCleanerProPlus"));
        assert!(looks_like_bundle_id("cn.com.10jqka.macstockPro"));
        assert!(!looks_like_bundle_id("Cleaner One Pro"));
        assert!(!looks_like_bundle_id("Foo"));
        assert!(!looks_like_bundle_id("Foo.app/x"));
        assert!(!looks_like_bundle_id("com..x"));
    }

    #[test]
    fn escapes_regex() {
        assert_eq!(regex_escape("/A (1).app"), r"/A \(1\)\.app");
    }
}
