//! 终端表格与 JSON 输出。

use std::path::Path;

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

pub fn print_table(findings: &[Finding], home: &Path) {
    if findings.is_empty() {
        println!("没有找到可清理的项目。");
        return;
    }
    // 中文字符占两列，宽度相应减半才能与数据列对齐
    println!("{:>8}  {:<10}  路径", "大小", "类别");
    for f in findings {
        if f.note.is_empty() {
            println!("{}", label(f, home));
        } else {
            println!("{}  ({})", label(f, home), f.note);
        }
    }
    let total: u64 = findings.iter().map(|f| f.size).sum();
    println!("\n共 {} 项，{}", findings.len(), human(total));
}

pub fn print_outcome(outcome: &Outcome, home: &Path, to_trash: bool) {
    for f in &outcome.failed {
        eprintln!("失败: {}: {}", display_path(&f.path, home), f.error);
    }
    let verb = if to_trash {
        "移到废纸篓"
    } else {
        "永久删除"
    };
    println!(
        "已{verb} {} 项，释放 {}；失败 {} 项。",
        outcome.removed,
        human(outcome.freed),
        outcome.failed.len()
    );
    if to_trash && outcome.removed > 0 {
        println!("清空废纸篓后才会真正释放空间。");
    }
    if outcome.needs_sudo() {
        eprintln!("提示: /Library 下的项目需要管理员权限，请用 sudo 重新运行。");
    }
}

pub fn print_skipped(skipped: usize) {
    if skipped > 0 {
        eprintln!(
            "有 {skipped} 个路径因权限等原因被跳过。可在 系统设置 › 隐私与安全性 › 完全磁盘访问权限 中为终端授权后重试。"
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
