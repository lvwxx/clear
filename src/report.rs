//! 终端表格与 JSON 输出。

use std::path::Path;

use console::{Style, style};
use humansize::{DECIMAL, format_size};
use serde_json::json;

use crate::clean::Outcome;
use crate::finding::Finding;

pub fn human(bytes: u64) -> String {
    format_size(bytes, DECIMAL)
}

/// 路径里的 HOME 显示为 `~`，表格更紧凑。
pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn label(f: &Finding, home: &Path) -> String {
    format!(
        "{:>10}  {:<12}  {}",
        human(f.size),
        f.category,
        display_path(&f.path, home)
    )
}

/// 各类别的颜色，让同类项在长列表里一眼可辨。
fn category_style(category: &str) -> Style {
    let s = Style::new();
    match category {
        "dev" | "large" | "dir" | "file" => s.cyan(),
        "cache" => s.blue(),
        "logs" | "dupe" => s.yellow(),
        "xcode" | "ios-backup" | "app" | "app-leftover" => s.magenta(),
        "trash" => s.red(),
        _ => s.white(),
    }
}

pub fn print_table(findings: &[Finding], home: &Path) {
    if findings.is_empty() {
        println!("{}", style("没有找到可清理的项目。").dim());
        return;
    }
    // 中文字符占两列，宽度相应减半才能与数据列对齐；先补齐宽度再上色，颜色码不占列宽
    println!(
        "{}",
        style(format!("{:>8}  {:<10}  路径", "大小", "类别")).dim()
    );
    for f in findings {
        let size = style(format!("{:>10}", human(f.size))).bold();
        let category = category_style(&f.category).apply_to(format!("{:<12}", f.category));
        let path = display_path(&f.path, home);
        if f.note.is_empty() {
            println!("{size}  {category}  {path}");
        } else {
            println!("{size}  {category}  {path}  {}", style(&f.note).dim());
        }
    }
    let total: u64 = findings.iter().map(|f| f.size).sum();
    println!(
        "\n共 {} 项，可释放 {}",
        findings.len(),
        style(human(total)).green().bold()
    );
}

/// 选择结束后的一行汇总，代替选择控件默认那行逗号拼接的清单。
pub fn print_selected(selected: &[&Finding]) {
    let total: u64 = selected.iter().map(|f| f.size).sum();
    println!(
        "{} 已选择 {} 项，共 {}",
        style("›").cyan(),
        selected.len(),
        style(human(total)).bold()
    );
}

pub fn print_outcome(outcome: &Outcome, home: &Path, to_trash: bool) {
    for f in &outcome.failed {
        eprintln!(
            "{} {}  {}",
            style("✘").red().for_stderr(),
            display_path(&f.path, home),
            style(&f.error).dim().for_stderr()
        );
    }
    if outcome.removed > 0 {
        let verb = if to_trash {
            "移到废纸篓"
        } else {
            "永久删除"
        };
        println!(
            "{} 已{verb} {} 项，释放 {}",
            style("✔").green(),
            outcome.removed,
            style(human(outcome.freed)).green().bold()
        );
        if to_trash {
            println!("  {}", style("清空废纸篓后才会真正释放磁盘空间").dim());
        }
    }
    if !outcome.failed.is_empty() {
        eprintln!(
            "{} {} 项删除失败",
            style("✘").red().for_stderr(),
            outcome.failed.len()
        );
    }
    if outcome.needs_full_disk_access(home) {
        let terminal = std::env::var("TERM_PROGRAM").unwrap_or_else(|_| "终端".into());
        eprintln!(
            "  {}",
            style(format!(
                "~/Library/Containers 下是 macOS 保护的应用数据，sudo 也删不了。\n  \
                 请在 系统设置 › 隐私与安全性 › 完全磁盘访问权限 中打开「{terminal}」，重启它后重新运行。\n  \
                 应用本体已删除的话，可以用 Bundle ID 清理残留：clr uninstall <BundleID> --clean"
            ))
            .yellow()
            .for_stderr()
        );
    }
    if outcome.needs_sudo() {
        eprintln!(
            "  {}",
            style("/Library 下的项目需要管理员权限，请用 sudo 重新运行")
                .yellow()
                .for_stderr()
        );
    }
}

pub fn print_skipped(skipped: usize) {
    if skipped > 0 {
        eprintln!(
            "{} {}",
            style("!").yellow().for_stderr(),
            style(format!(
                "{skipped} 个路径因权限被跳过，可在 系统设置 › 隐私与安全性 › 完全磁盘访问权限 中为终端授权"
            ))
            .dim()
            .for_stderr()
        );
    }
}

pub fn print_json(findings: &[Finding], outcome: Option<&Outcome>, skipped: usize) {
    let total: u64 = findings.iter().map(|f| f.size).sum();
    let mut value = json!({ "findings": findings, "total": total, "skipped": skipped });
    if let Some(outcome) = outcome {
        value["clean"] = json!(outcome);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("序列化不会失败")
    );
}
