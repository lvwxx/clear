//! 端到端测试：在临时目录里搭假 HOME，通过 `CLR_HOME` 注入，运行 `clr` 二进制。
//! 删除到废纸篓的路径用 `CLR_TRASH_DIR` 重定向，不碰真实废纸篓。

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

struct Env {
    _dir: TempDir,
    home: PathBuf,
    trash: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let trash = dir.path().join("trash");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&trash).unwrap();
        Env {
            _dir: dir,
            home,
            trash,
        }
    }

    fn write(&self, rel: &str, bytes: usize) -> PathBuf {
        self.write_bytes(rel, &vec![b'x'; bytes])
    }

    fn write_bytes(&self, rel: &str, content: &[u8]) -> PathBuf {
        let path = self.home.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn clr(&self) -> Command {
        let mut cmd = Command::cargo_bin("clr").unwrap();
        cmd.env("CLR_HOME", &self.home)
            .env("CLR_TRASH_DIR", &self.trash);
        cmd
    }

    /// 运行并解析 JSON 输出，返回 (退出码, JSON)。
    fn json(&self, args: &[&str]) -> (i32, Value) {
        let out = self.clr().args(args).arg("--json").output().unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        let value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
            panic!(
                "非 JSON 输出 ({e}): {stdout}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), value)
    }

    fn rel(&self, path: &str) -> String {
        Path::new(path)
            .strip_prefix(&self.home)
            .unwrap()
            .display()
            .to_string()
    }

    fn paths(&self, v: &Value) -> Vec<String> {
        let mut out: Vec<String> = v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| self.rel(f["path"].as_str().unwrap()))
            .collect();
        out.sort();
        out
    }
}

#[test]
fn junk_scans_by_rules_and_category() {
    let env = Env::new();
    env.write("Library/Caches/com.foo/data", 50_000);
    env.write("Library/Caches/Homebrew/pkg.tar.gz", 80_000);
    env.write("Library/Logs/foo.log", 10_000);
    env.write("Library/Developer/Xcode/DerivedData/App-abc/build", 30_000);
    env.write(".Trash/old.zip", 20_000);
    env.write("Documents/keep.txt", 10_000);

    let (code, v) = env.json(&["junk"]);
    assert_eq!(code, 0);
    assert_eq!(
        env.paths(&v),
        [
            ".Trash/old.zip",
            "Library/Caches/Homebrew",
            "Library/Caches/com.foo",
            "Library/Developer/Xcode/DerivedData/App-abc",
            "Library/Logs/foo.log",
        ]
    );
    let homebrew = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with("Homebrew"))
        .unwrap();
    assert_eq!(homebrew["category"], "dev");
    assert!(homebrew["size"].as_u64().unwrap() >= 80_000);

    let (_, v) = env.json(&["junk", "--category", "xcode,trash"]);
    assert_eq!(
        env.paths(&v),
        [
            ".Trash/old.zip",
            "Library/Developer/Xcode/DerivedData/App-abc"
        ]
    );
}

#[test]
fn junk_user_rules_extend_builtin() {
    let env = Env::new();
    env.write("scratch/tmp1", 5_000);
    env.write_bytes(
        ".config/clear/rules.toml",
        br#"
[[rule]]
id = "scratch"
category = "custom"
paths = ["~/scratch/*"]
"#,
    );
    let (_, v) = env.json(&["junk", "--category", "custom"]);
    assert_eq!(env.paths(&v), ["scratch/tmp1"]);
}

#[test]
fn junk_clean_moves_to_trash_and_permanent_deletes() {
    let env = Env::new();
    let cache = env.write("Library/Caches/com.foo/data", 50_000);
    let log = env.write("Library/Logs/foo.log", 10_000);

    let (code, v) = env.json(&["junk", "--category", "cache", "--clean", "--yes"]);
    assert_eq!(code, 0);
    assert_eq!(v["clean"]["removed"], 1);
    assert!(!cache.exists());
    assert!(env.trash.join("com.foo/data").exists());
    assert!(log.exists());

    let (code, v) = env.json(&[
        "junk",
        "--category",
        "logs",
        "--clean",
        "--yes",
        "--permanent",
    ]);
    assert_eq!(code, 0);
    assert_eq!(v["clean"]["removed"], 1);
    assert!(!log.exists());
    assert!(!env.trash.join("foo.log").exists());
}

#[test]
fn large_threshold_skips_library_and_git() {
    let env = Env::new();
    env.write("Movies/big.mov", 5 << 20);
    env.write("small.txt", 1 << 10);
    env.write("Library/huge.cache", 3 << 20);
    env.write("proj/.git/objects/pack", 3 << 20);

    let (_, v) = env.json(&["large", "--min", "1M"]);
    assert_eq!(env.paths(&v), ["Movies/big.mov"]);

    let (_, v) = env.json(&["large", "--min", "1M", "--include-library"]);
    assert_eq!(env.paths(&v), ["Library/huge.cache", "Movies/big.mov"]);

    // 刚写入的文件不满足「90 天没访问」
    let (_, v) = env.json(&["large", "--min", "1M", "--older", "90d"]);
    assert_eq!(env.paths(&v), Vec::<String>::new());

    let (_, v) = env.json(&["large", "--dirs", "--top", "1"]);
    assert_eq!(env.paths(&v), ["Movies"]);
}

#[test]
fn dupes_groups_identical_content_only() {
    let env = Env::new();
    let content = vec![3u8; 2 << 20];
    let mut near = content.clone();
    *near.last_mut().unwrap() = 4;
    env.write_bytes("a.bin", &content);
    env.write_bytes("backup/old/a.bin", &content);
    env.write_bytes("near.bin", &near);
    fs::hard_link(env.home.join("a.bin"), env.home.join("hard.bin")).unwrap();
    std::os::unix::fs::symlink(env.home.join("a.bin"), env.home.join("link.bin")).unwrap();

    let (_, v) = env.json(&["dupes"]);
    assert_eq!(env.paths(&v), ["backup/old/a.bin"]);

    let (code, v) = env.json(&["dupes", "--clean", "--yes", "--permanent"]);
    assert_eq!(code, 0);
    assert_eq!(v["clean"]["removed"], 1);
    assert!(env.home.join("a.bin").exists());
    assert!(!env.home.join("backup/old/a.bin").exists());
}

fn fake_app(env: &Env, rel: &str, bundle_id: &str) -> PathBuf {
    let app = env.home.join(rel);
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{bundle_id}</string>
</dict></plist>"#
    );
    env.write_bytes(&format!("{rel}/Contents/Info.plist"), plist.as_bytes());
    env.write(&format!("{rel}/Contents/MacOS/Foo"), 4096);
    app
}

#[test]
fn uninstall_matches_leftovers_exactly() {
    let env = Env::new();
    fake_app(&env, "Applications/Foo.app", "com.acme.Foo");
    env.write("Library/Application Support/com.acme.Foo/db", 1000);
    env.write("Library/Application Support/Foo/state", 1000);
    env.write("Library/Preferences/com.acme.Foo.plist", 100);
    env.write("Library/Caches/com.acme.Foo/c", 1000);
    env.write("Library/Containers/com.acme.Foo/Data/x", 1000);
    env.write("Library/Group Containers/TEAM123.com.acme.Foo/y", 1000);
    env.write(
        "Library/Saved Application State/com.acme.Foo.savedState/w",
        100,
    );
    // 这些名字相近但不属于 Foo
    env.write("Library/Application Support/com.acme.FooBar/db", 1000);
    env.write("Library/Preferences/com.acme.FooBar.plist", 100);
    env.write("Library/Preferences/Foo.plist", 100);

    // 按名称查找（忽略大小写）
    let (code, v) = env.json(&["uninstall", "foo"]);
    assert_eq!(code, 0);
    assert_eq!(
        env.paths(&v),
        [
            "Applications/Foo.app",
            "Library/Application Support/Foo",
            "Library/Application Support/com.acme.Foo",
            "Library/Caches/com.acme.Foo",
            "Library/Containers/com.acme.Foo",
            "Library/Group Containers/TEAM123.com.acme.Foo",
            "Library/Preferences/com.acme.Foo.plist",
            "Library/Saved Application State/com.acme.Foo.savedState",
        ]
    );

    let app_path = env.home.join("Applications/Foo.app");
    let (code, v) = env.json(&["uninstall", app_path.to_str().unwrap(), "--clean", "--yes"]);
    assert_eq!(code, 0);
    assert_eq!(v["clean"]["removed"], 8);
    assert!(!app_path.exists());
    assert!(
        env.home
            .join("Library/Application Support/com.acme.FooBar/db")
            .exists()
    );
    assert!(env.home.join("Library/Preferences/Foo.plist").exists());
}

#[test]
fn uninstall_unknown_app_fails_with_code_2() {
    let env = Env::new();
    env.clr()
        .args(["uninstall", "NoSuchApp"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("找不到应用"));
}

#[test]
fn protected_paths_are_never_deleted() {
    let env = Env::new();
    env.write("Documents/important.txt", 100);
    // 恶意用户规则：试图删除 ~/Documents 本身
    env.write_bytes(
        ".config/clear/rules.toml",
        br#"
[[rule]]
id = "evil"
category = "evil"
paths = ["~/Documents"]
"#,
    );
    let (code, v) = env.json(&[
        "junk",
        "--category",
        "evil",
        "--clean",
        "--yes",
        "--permanent",
    ]);
    assert_eq!(code, 1);
    assert_eq!(v["clean"]["removed"], 0);
    assert_eq!(v["clean"]["failed"].as_array().unwrap().len(), 1);
    assert!(env.home.join("Documents/important.txt").exists());
}

#[test]
fn symlinks_are_not_followed_when_deleting() {
    let env = Env::new();
    let precious = env.write("Documents/precious.txt", 100);
    fs::create_dir_all(env.home.join("Library/Caches/com.foo")).unwrap();
    env.write("Library/Caches/com.foo/data", 1000);
    std::os::unix::fs::symlink(
        env.home.join("Documents"),
        env.home.join("Library/Caches/com.foo/docs"),
    )
    .unwrap();

    let (code, _) = env.json(&[
        "junk",
        "--category",
        "cache",
        "--clean",
        "--yes",
        "--permanent",
    ]);
    assert_eq!(code, 0);
    assert!(!env.home.join("Library/Caches/com.foo").exists());
    assert!(precious.exists());
}

#[test]
fn json_clean_requires_yes() {
    let env = Env::new();
    env.clr()
        .args(["junk", "--json", "--clean"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--yes"));
}

#[test]
fn clean_without_tty_requires_yes() {
    let env = Env::new();
    env.write("Library/Logs/a.log", 1000);
    env.clr()
        .args(["junk", "--clean"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--yes"));
    assert!(env.home.join("Library/Logs/a.log").exists());
}

#[test]
fn bad_size_argument_is_usage_error() {
    let env = Env::new();
    env.clr().args(["large", "--min", "5X"]).assert().code(2);
}

#[test]
fn dupes_skips_dependency_dirs_by_default() {
    let env = Env::new();
    let content = vec![5u8; 2 << 20];
    env.write_bytes("proj-a/node_modules/lib/big.js", &content);
    env.write_bytes("proj-b/node_modules/lib/big.js", &content);
    env.write_bytes("go/pkg/mod/x@v1/big.bin", &content);
    env.write_bytes("go/pkg/mod/y@v1/big.bin", &content);

    let (_, v) = env.json(&["dupes"]);
    assert_eq!(env.paths(&v), Vec::<String>::new());

    let (_, v) = env.json(&["dupes", "--include-deps"]);
    assert_eq!(v["findings"].as_array().unwrap().len(), 3);
}
