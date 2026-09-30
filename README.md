# clear

macOS 磁盘清理命令行工具，命令名 `clr`。默认只扫描、列出结果，加 `--clean` 才会删除；删除默认移到废纸篓。

## 安装

```sh
cargo install --path .
```

## 用法

```sh
clr junk                          # 缓存、日志、Xcode、iOS 备份、废纸篓、开发工具缓存
clr junk --category dev,xcode     # 只看某些类别
clr large --min 1G --older 90d    # 90 天没访问过的 1G 以上文件
clr large --dirs                  # HOME 下占用最多的目录
clr dupes ~/Downloads             # 重复文件（默认只比较 ≥ 1M 的，跳过 node_modules、Go/Rust/npm 依赖缓存）
clr uninstall Slack               # 应用本体 + Library 下的残留
```

通用参数：

| 参数 | 作用 |
|---|---|
| `--clean` | 扫描后多选删除（large 默认不勾选，其余默认全选） |
| `--permanent` | 永久删除，而不是移到废纸篓 |
| `-y`, `--yes` | 跳过选择，删除全部扫描结果 |
| `--json` | JSON 输出；与 `--clean` 同用时必须加 `--yes` |

退出码：`0` 成功，`1` 有项目删除失败，`2` 参数或其他错误。

## 自定义规则

在 `~/.config/clear/rules.toml` 中追加规则，或用相同 `id` 覆盖内置规则（见 `src/rules.toml`）：

```toml
[[rule]]
id = "my-build"
category = "dev"
paths = ["~/workspace/*/target"]
desc = "Rust 编译产物"
```

## 安全措施

- 删除前检查受保护路径：系统目录、HOME 本身、`~/Documents` 等目录本身不会被删除，父目录是符号链接时按真实路径再检查一次。
- 遍历不跟随符号链接，也不跨文件系统。
- 移到废纸篓用 NSFileManager 而不是 Finder，不会弹窗要密码；没有权限的文件直接报失败。
- 卸载只按 Bundle ID 精确匹配残留，App 正在运行时拒绝卸载。
- 没有权限读取的路径会跳过并在结束时提示。要扫描完整，需要在「系统设置 › 隐私与安全性 › 完全磁盘访问权限」中为终端授权。

## 开发

```sh
cargo test
```

集成测试用 `CLR_HOME` 注入假 HOME，用 `CLR_TRASH_DIR` 把「移到废纸篓」重定向到临时目录。
