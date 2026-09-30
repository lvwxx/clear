mod clean;
mod dupes;
mod finding;
mod junk;
mod large;
mod remove;
mod report;
mod rules;
mod safety;
mod size;
mod uninstall;
mod walk;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use dialoguer::MultiSelect;
use dialoguer::theme::ColorfulTheme;

use finding::Finding;
use remove::{PermanentRemover, Remover, TrashRemover};
use walk::{Skip, Stats};

#[derive(Parser)]
#[command(
    name = "clr",
    version,
    about = "macOS 磁盘清理工具：默认只扫描，加 --clean 才删除"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,

    /// 扫描后选择并删除
    #[arg(long, global = true)]
    clean: bool,

    /// 永久删除，而不是移到废纸篓
    #[arg(long, global = true)]
    permanent: bool,

    /// 跳过确认，删除全部扫描结果
    #[arg(long, short, global = true)]
    yes: bool,

    /// 以 JSON 输出
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// 按规则库扫描缓存、日志、Xcode、开发工具缓存等垃圾文件
    Junk {
        /// 只扫描这些类别，逗号分隔（cache,logs,xcode,ios-backup,trash,dev）
        #[arg(long, value_delimiter = ',')]
        category: Vec<String>,
    },
    /// 查找大文件，或用 --dirs 查看目录占用
    Large {
        /// 扫描的目录，默认 HOME
        path: Option<PathBuf>,
        /// 最小占用，如 500M、2G
        #[arg(long, default_value = "500M", value_parser = size::parse_size)]
        min: u64,
        /// 只列出这么久没访问过的文件，如 90d、4w
        #[arg(long, value_parser = size::parse_duration)]
        older: Option<Duration>,
        /// 改为列出占用最多的直接子目录
        #[arg(long)]
        dirs: bool,
        /// --dirs 模式下列出的数量
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// 不跳过 ~/Library
        #[arg(long)]
        include_library: bool,
    },
    /// 查找内容完全相同的重复文件
    Dupes {
        /// 扫描的目录，默认 HOME
        path: Option<PathBuf>,
        /// 只比较不小于这个大小的文件
        #[arg(long, default_value = "1M", value_parser = size::parse_size)]
        min: u64,
        /// 不跳过 ~/Library
        #[arg(long)]
        include_library: bool,
    },
    /// 卸载应用并清理它在 Library 下的残留文件
    Uninstall {
        /// 应用名称（如 Slack）或 .app 路径
        app: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("错误: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    if cli.json && cli.clean && !cli.yes {
        bail!("--json 与 --clean 同时使用时必须加 --yes");
    }
    let home = home_dir()?;
    let stats = Stats::default();

    // large 结果是用户自己的文件，默认不勾选；其余默认全选
    let (findings, preselect) = match &cli.cmd {
        Cmd::Junk { category } => {
            let rules = rules::load(&home)?;
            (junk::scan(&rules, category, &home, &stats), true)
        }
        Cmd::Large {
            path,
            min,
            older,
            dirs,
            top,
            include_library,
        } => {
            let root = resolve_root(path.as_deref(), &home)?;
            let skip = default_skip(&home, *include_library);
            let found = if *dirs {
                large::dirs(&root, *top, &skip, &stats)
            } else {
                large::files(&root, *min, *older, &skip, &stats)
            };
            (found, false)
        }
        Cmd::Dupes {
            path,
            min,
            include_library,
        } => {
            let root = resolve_root(path.as_deref(), &home)?;
            let skip = default_skip(&home, *include_library);
            (dupes::scan(&root, *min, &skip, &stats), true)
        }
        Cmd::Uninstall { app } => {
            let app = uninstall::locate(app, &home)?;
            if cli.clean && uninstall::is_running(&app) {
                bail!("{} 正在运行，请先退出再卸载", app.name);
            }
            (uninstall::scan(&app, &home, &stats), true)
        }
    };

    if !cli.clean {
        if cli.json {
            report::print_json(&findings, None, stats.skipped());
        } else {
            report::print_table(&findings, &home);
            report::print_skipped(stats.skipped());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let selected = if cli.yes {
        findings.iter().collect()
    } else {
        if !cli.json {
            report::print_skipped(stats.skipped());
        }
        select(&findings, &home, preselect)?
    };
    if selected.is_empty() {
        if cli.json {
            report::print_json(&findings, None, stats.skipped());
        } else {
            println!("没有选中任何项目。");
        }
        return Ok(ExitCode::SUCCESS);
    }

    let remover: &dyn Remover = if cli.permanent {
        &PermanentRemover
    } else {
        &TrashRemover
    };
    let outcome = clean::execute(&selected, remover, &home);
    if cli.json {
        report::print_json(&findings, Some(&outcome), stats.skipped());
    } else {
        if cli.yes {
            report::print_skipped(stats.skipped());
        }
        report::print_outcome(&outcome, &home, !cli.permanent);
    }
    Ok(if outcome.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn select<'a>(findings: &'a [Finding], home: &Path, preselect: bool) -> Result<Vec<&'a Finding>> {
    if findings.is_empty() {
        println!("没有找到可清理的项目。");
        return Ok(Vec::new());
    }
    if !std::io::stdin().is_terminal() {
        bail!("非交互终端下删除需要加 --yes");
    }
    let labels: Vec<String> = findings.iter().map(|f| report::label(f, home)).collect();
    // ColorfulTheme 用 ✔ 标记已勾选项，默认主题是 [x]/[ ]，不够直观
    let chosen = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("空格勾选/取消，a 全选，回车确认，Esc 放弃")
        .items(&labels)
        .defaults(&vec![preselect; findings.len()])
        .max_length(20)
        .interact_opt()?
        .unwrap_or_default();
    Ok(chosen.into_iter().map(|i| &findings[i]).collect())
}

/// HOME 目录。环境变量 `CLR_HOME` 优先，供测试注入假 HOME。
fn home_dir() -> Result<PathBuf> {
    if let Some(h) = std::env::var_os("CLR_HOME") {
        return Ok(PathBuf::from(h));
    }
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法确定 HOME 目录"))
}

fn resolve_root(path: Option<&Path>, home: &Path) -> Result<PathBuf> {
    let root = match path {
        Some(p) => std::path::absolute(p)?,
        None => home.to_path_buf(),
    };
    if !root.is_dir() {
        bail!("{} 不是目录", root.display());
    }
    Ok(root)
}

fn default_skip(home: &Path, include_library: bool) -> Skip {
    Skip {
        dirs: if include_library {
            vec![]
        } else {
            vec![home.join("Library")]
        },
        git: true,
    }
}
