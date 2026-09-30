//! 删除前的最后一道拦截：无论规则或参数怎么写，这些路径都不能删。

use std::path::{Component, Path};

/// 这些路径本身不能删（内部的文件不受限制）。
const PROTECTED_EXACT: &[&str] = &[
    "/",
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/etc",
    "/var",
    "/private",
    "/opt",
    "/Library",
    "/Applications",
    "/Users",
    "/Volumes",
];

/// 这些路径及其内部全部不能删。
const PROTECTED_TREES: &[&str] = &["/System", "/usr", "/bin", "/sbin", "/etc", "/private/etc"];

/// HOME 下的这些目录本身不能删。
const PROTECTED_HOME_DIRS: &[&str] = &[
    "Documents",
    "Desktop",
    "Downloads",
    "Pictures",
    "Movies",
    "Music",
    "Library",
    ".ssh",
    ".Trash",
];

pub fn is_protected(path: &Path, home: &Path) -> bool {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return true;
    }
    if path == home || PROTECTED_EXACT.iter().any(|p| path == Path::new(p)) {
        return true;
    }
    if PROTECTED_TREES.iter().any(|p| path.starts_with(p)) {
        return true;
    }
    PROTECTED_HOME_DIRS.iter().any(|d| path == home.join(d))
}

/// 同时检查路径本身和「父目录解析符号链接后」的真实位置，
/// 防止 `~/Library/Caches/x` 里的 Caches 其实是指向 ~/Documents 的链接。
pub fn is_protected_resolved(path: &Path, home: &Path) -> bool {
    if is_protected(path, home) {
        return true;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return true;
    };
    match std::fs::canonicalize(parent) {
        Ok(real_parent) => {
            let real_home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
            is_protected(&real_parent.join(name), &real_home)
        }
        // 父目录不存在，路径本身也就不存在，交给删除逻辑处理
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn protected(p: &str) -> bool {
        is_protected(Path::new(p), Path::new(HOME))
    }

    #[test]
    fn system_paths() {
        for p in [
            "/",
            "/System",
            "/System/Library/x",
            "/usr/bin",
            "/Library",
            "/Applications",
            "/etc/hosts",
        ] {
            assert!(protected(p), "{p} 应受保护");
        }
        assert!(!protected("/Library/LaunchAgents/com.x.plist"));
        assert!(!protected("/Applications/Foo.app"));
    }

    #[test]
    fn home_paths() {
        for p in [
            "/Users/me",
            "/Users/me/",
            "/Users/me/Documents",
            "/Users/me/Library",
            "/Users/me/.Trash",
        ] {
            assert!(protected(p), "{p} 应受保护");
        }
        assert!(!protected("/Users/me/Documents/a.txt"));
        assert!(!protected("/Users/me/Library/Caches/com.x"));
        assert!(!protected("/Users/me/.Trash/old.zip"));
    }

    #[test]
    fn rejects_relative_and_parent_dir() {
        assert!(protected("relative/path"));
        assert!(protected("/Users/me/Library/Caches/../../Documents"));
    }

    #[test]
    fn resolves_symlinked_parent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join("Library")).unwrap();
        std::fs::create_dir_all(home.join("real")).unwrap();
        // ~/cachelink -> ~，于是 ~/cachelink/Library 实际就是 ~/Library
        std::os::unix::fs::symlink(home, home.join("cachelink")).unwrap();
        assert!(!is_protected(&home.join("cachelink/Library"), home));
        assert!(is_protected_resolved(&home.join("cachelink/Library"), home));
        assert!(!is_protected_resolved(&home.join("real/x"), home));
    }
}
