# clear — macOS 磁盘清理 CLI 设计

- 日期：2026-09-29
- 状态：已确认，待出实现计划

## 1. 目标与范围

一个类似 Cleaner One Pro 核心功能的 macOS 命令行清理工具，用 Rust 实现。

- 项目 / 仓库名：`clear`；可执行文件名：`clr`（避免与系统 `/usr/bin/clear` 冲突）
- 位置：`~/workspace/github/clear`
- 第一版包含四个功能：垃圾清理、大文件分析、重复文件查找、应用卸载残留
- 不在范围内：GUI / TUI、内存优化、启动项管理、相似照片、Windows / Linux 支持

## 2. 架构

单个 Cargo crate（二进制 + 内部模块）。规则库 `rules.toml` 通过 `include_str!` 编译进二进制；若存在 `~/.config/clear/rules.toml`，则与内置规则合并（同 `id` 覆盖，新 `id` 追加）。

模块划分，每个模块只做一件事：

| 模块 | 职责 |
|---|---|
| `cli` | clap 定义子命令与参数，调度各命令 |
| `rules` | 解析、合并规则；`~` 与 glob 展开 |
| `walk` | 并行遍历（`jwalk`），不跟随符号链接，按 `(dev, inode)` 去重计算大小，记录权限跳过数 |
| `junk` / `large` / `dupes` / `uninstall` | 四个扫描器，各自输出统一的 `Finding` 列表 |
| `safety` | 受保护路径判断，删除前硬性拦截 |
| `remove` | `Remover` trait；`TrashRemover`（`trash` crate）、`PermanentRemover`；测试用假实现 |
| `report` | 表格输出（人类可读大小）与 `--json` 输出、最终汇总 |
| `size` | 解析 `500M` / `2G` 与 `90d` / `12w` 等参数 |

统一数据结构：

```rust
struct Finding {
    path: PathBuf,
    size: u64,        // 字节，目录为递归大小
    category: String, // 规则类别 / "large" / "dupe" / "app-leftover"
    note: String,     // 规则描述、重复组号等
}
```

HOME 目录取自环境变量 `CLR_HOME`，未设置时用 `dirs::home_dir()`，以便集成测试注入假 HOME。

## 3. 命令与交互

```
clr junk      [--category <a,b,...>]
clr large     [PATH] [--min 500M] [--older 90d] [--dirs] [--top N] [--include-library]
clr dupes     [PATH] [--min 1M] [--include-library]
clr uninstall <App名称 | .app路径>
```

通用参数：`--clean`、`--permanent`、`--yes`、`--json`。

- 默认只扫描、以表格列出（路径、大小、类别、说明），**不删除任何东西**。
- `--clean`：用 `dialoguer` 多选确认后删除；配合 `--yes` 跳过确认，全部删除。
- 删除默认移到废纸篓（Finder 接口，支持「放回原处」）；`--permanent` 永久删除。
- `--json` 输出结构化结果，不做交互（与 `--clean` 同用时必须同时给 `--yes`，否则报参数错误）。

## 4. 扫描器细节

### 4.1 junk

规则格式：

```toml
[[rule]]
id = "xcode-derived"
category = "xcode"
paths = ["~/Library/Developer/Xcode/DerivedData/*"]
desc = "Xcode 编译中间产物，会自动重建"
```

内置类别：

- `cache`：`~/Library/Caches/*`
- `logs`：`~/Library/Logs/*`
- `xcode`：DerivedData、Archives、`~/Library/Developer/CoreSimulator/Caches`
- `ios-backup`：`~/Library/Application Support/MobileSync/Backup/*`
- `trash`：`~/.Trash/*`
- `dev`：`~/.npm/_cacache`、`~/Library/Caches/Yarn`、`~/Library/pnpm/store`、`~/.cargo/registry/cache`、`~/Library/Caches/go-build`、`~/Library/Caches/Homebrew`、`~/Library/Caches/pip`

每个 glob 匹配项作为一个 `Finding`，大小由 `walk` 统计。不存在的路径静默忽略。`--category` 过滤类别。

### 4.2 large

- 并行遍历 PATH（默认 HOME），取大小 ≥ `--min`（默认 500M）的文件；`--older` 按最后访问时间（atime）过滤；按大小降序。
- 默认跳过 `~/Library` 与任意 `.git` 目录；`--include-library` 取消对 `~/Library` 的跳过。
- `--dirs`：改为输出递归占用最大的前 `--top`（默认 20）个直接子目录。

### 4.3 dupes

三级筛选：

1. 按大小分组（仅 ≥ `--min`，默认 1M），丢弃单成员组；
2. 头部 4KB 的 BLAKE3 哈希再分组；
3. 全量 BLAKE3 哈希确认。

- 哈希用 `rayon` 并行。
- 同一 `(dev, inode)` 视为同一个文件，不算重复。
- 每组保留一份：路径最短者，长度相同时取修改时间最早者；其余作为可删除的 `Finding`，`note` 标注组号和保留的路径。
- 结果按每组可释放空间降序。

### 4.4 uninstall

1. 定位 App：参数是 `.app` 路径时直接使用，否则在 `/Applications`、`~/Applications` 中按名称（忽略大小写，可省略 `.app`）查找。
2. 用 `plist` crate 读取 `Contents/Info.plist` 的 `CFBundleIdentifier`。
3. 在以下目录中匹配文件名**等于 Bundle ID、以 `<BundleID>.` 开头，或以 `.<BundleID>` 结尾**（Group Containers 的 `<TeamID>.<BundleID>`）的项（忽略大小写），以及文件名等于 App 名称的项（仅限 Application Support、Caches）：
   - `~/Library/`：`Application Support`、`Preferences`、`Preferences/ByHost`、`Caches`、`Containers`、`Group Containers`、`Saved Application State`、`LaunchAgents`、`HTTPStorages`、`WebKit`
   - `/Library/LaunchAgents`、`/Library/LaunchDaemons`
4. 不做模糊匹配。
5. App 正在运行（`pgrep -f <app路径>/Contents/MacOS/`）时拒绝卸载并报错。
6. `.app` 本体本身也作为一个 `Finding` 列出。

## 5. 安全

`safety::is_protected(path)` 在每次删除前调用，命中即拒绝该项并计为失败。以下路径受保护：

- 路径本身是 `/`、HOME、`/System`、`/usr`、`/bin`、`/sbin`、`/etc`、`/var`、`/private`、`/opt`、`/Library`、`/Applications`、`/Users`、`/Volumes`，或以 `/System/`、`/usr/`、`/bin/`、`/sbin/`、`/etc/`、`/private/etc/` 开头；
- HOME 下的这些目录本身：`Documents`、`Desktop`、`Downloads`、`Pictures`、`Movies`、`Music`、`Library`、`.ssh`、`.Trash`（其内部文件不受保护）；
- 带 `..` 的路径不做规范化就直接拒绝。

- 所有遍历都不跟随符号链接，也不跨文件系统（类似 `du -x`，避免把 ~/OrbStack 这类挂载点算进来）；删除符号链接时只删链接本身。
- 删除前还会解析父目录的符号链接，用真实路径再检查一次受保护路径。
- `/Library` 下的文件删除失败（EACCES / EPERM）时，提示「使用 sudo 重新运行」，不静默失败。

## 6. 错误处理

- 顶层用 `anyhow`，模块内部错误用 `thiserror`。
- 扫描时无权限的路径跳过并计数，结束时汇总一行提示，引导用户到「系统设置 › 隐私与安全性 › 完全磁盘访问权限」为终端授权。
- 删除逐项执行，单项失败不影响其余项；结束时报告成功 / 失败项数与释放空间。
- 退出码：`0` 成功；`1` 部分删除失败；`2` 参数或规则错误。

## 7. 测试

- **单元测试**：规则解析与合并、`~` / glob 展开、`safety::is_protected`、大小与时长参数解析、dupes 保留项的选择规则。
- **集成测试**（`tests/`，用 `tempfile` 搭假 HOME，通过 `CLR_HOME` 注入，用 `assert_cmd` 运行二进制）：
  - junk：伪造的缓存和日志能被识别，大小正确，`--category` 过滤有效；
  - large：阈值、`--older`、`--dirs`，默认跳过 Library 和 `.git`；
  - dupes：内容相同的文件被分组；大小相同但内容不同的不算重复；硬链接不算重复；
  - uninstall：带 `Info.plist` 的假 `.app` 加残留文件能被全部匹配，不相关的相似名称文件不被匹配；
  - 符号链接不被跟随；指向受保护路径的删除被拒绝；
  - `--clean --yes --permanent` 确实删除临时文件，退出码正确。
- 废纸篓路径通过 `Remover` trait 注入假实现测试；集成测试设置环境变量 `CLR_TRASH_DIR`，让「移到废纸篓」改为移动到临时目录，不触碰真实废纸篓。

## 8. 依赖

`clap`（derive）、`anyhow`、`thiserror`、`serde` + `toml`、`serde_json`、`glob`、`jwalk`、`rayon`、`blake3`、`plist`、`trash`、`dialoguer`、`dirs`、`humansize`；开发依赖：`tempfile`、`assert_cmd`、`predicates`。
