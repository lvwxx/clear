//! 扫描时在 stderr 显示的进度行：阶段、已扫描文件数、当前位置。

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::report::display_path;

#[derive(Clone)]
pub struct Progress {
    bar: ProgressBar,
    home: Arc<PathBuf>,
}

impl Progress {
    pub fn new(home: &Path) -> Self {
        let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
        bar.set_style(
            ProgressStyle::with_template(
                "{spinner:.cyan} {prefix:.bold} · {human_pos} 个文件 · {wide_msg:.dim}",
            )
            .expect("模板是固定的")
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
        );
        bar.set_prefix("准备扫描");
        bar.enable_steady_tick(Duration::from_millis(100));
        Progress {
            bar,
            home: Arc::new(home.to_path_buf()),
        }
    }

    /// 切换阶段时清掉上一阶段的路径，避免显示过时的位置。
    pub fn stage(&self, stage: impl Into<Cow<'static, str>>) {
        self.bar.set_prefix(stage);
        self.bar.set_message("");
    }

    /// 新阶段从 0 开始计数，例如重复文件从「遍历」进入「比较内容」时。
    pub fn restart(&self, stage: impl Into<Cow<'static, str>>) {
        self.bar.set_position(0);
        self.stage(stage);
    }

    pub fn at(&self, path: &Path) {
        self.bar.set_message(display_path(path, &self.home));
    }

    pub fn file(&self) {
        self.bar.inc(1);
    }

    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }
}
