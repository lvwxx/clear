use std::path::PathBuf;

use serde::Serialize;

/// 扫描器的统一输出：一个可清理的路径。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    pub path: PathBuf,
    /// 实际占用磁盘空间（字节），目录为递归大小
    pub size: u64,
    /// 规则类别，或 "large" / "dupe" / "app" / "app-leftover"
    pub category: String,
    /// 规则描述、重复组信息等
    pub note: String,
}
