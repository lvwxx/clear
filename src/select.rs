//! 终端多选列表。dialoguer 的 MultiSelect 到底会循环回第一项且无法关闭，所以自己实现：
//! 光标在首尾停住，底部实时显示已选数量和大小。

use std::io;

use console::{Key, Term, style};

use crate::report::human;

/// 列表状态与按键处理，和终端绘制分开，便于测试。
struct State {
    checked: Vec<bool>,
    cursor: usize,
    /// 可视区域第一行对应的项
    offset: usize,
    height: usize,
}

enum Action {
    Continue,
    Confirm,
    Cancel,
}

impl State {
    fn new(len: usize, preselect: bool, height: usize) -> Self {
        State {
            checked: vec![preselect; len],
            cursor: 0,
            offset: 0,
            height: height.max(1),
        }
    }

    fn handle(&mut self, key: Key) -> Action {
        let last = self.checked.len().saturating_sub(1);
        match key {
            Key::ArrowDown | Key::Char('j') => self.cursor = (self.cursor + 1).min(last),
            Key::ArrowUp | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::PageDown => self.cursor = (self.cursor + self.height).min(last),
            Key::PageUp => self.cursor = self.cursor.saturating_sub(self.height),
            Key::Home | Key::Char('g') => self.cursor = 0,
            Key::End | Key::Char('G') => self.cursor = last,
            Key::Char(' ') => {
                if let Some(c) = self.checked.get_mut(self.cursor) {
                    *c = !*c;
                }
            }
            Key::Char('a') => {
                let all = self.checked.iter().all(|&c| c);
                self.checked.iter_mut().for_each(|c| *c = !all);
            }
            Key::Enter => return Action::Confirm,
            Key::Escape | Key::Char('q') => return Action::Cancel,
            _ => {}
        }
        // 让光标始终在可视区域内
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if self.cursor >= self.offset + self.height {
            self.offset = self.cursor + 1 - self.height;
        }
        Action::Continue
    }

    fn chosen(&self) -> Vec<usize> {
        (0..self.checked.len())
            .filter(|&i| self.checked[i])
            .collect()
    }
}

/// 返回选中项的下标；按 Esc 放弃时返回 None。
pub fn multi_select(
    labels: &[String],
    sizes: &[u64],
    preselect: bool,
) -> io::Result<Option<Vec<usize>>> {
    let term = Term::stderr();
    // 留出提示行、底部汇总行和一行余量
    let rows = term.size().0 as usize;
    let height = labels.len().min(20).min(rows.saturating_sub(3).max(1));
    let mut state = State::new(labels.len(), preselect, height);

    term.hide_cursor()?;
    let result = run(&term, &mut state, labels, sizes);
    // 无论成功与否都要恢复光标、清掉列表
    let _ = term.clear_last_lines(height + 2);
    let _ = term.show_cursor();
    result
}

fn run(
    term: &Term,
    state: &mut State,
    labels: &[String],
    sizes: &[u64],
) -> io::Result<Option<Vec<usize>>> {
    let mut drawn = false;
    loop {
        if drawn {
            term.clear_last_lines(state.height + 2)?;
        }
        draw(term, state, labels, sizes)?;
        drawn = true;
        match state.handle(term.read_key()?) {
            Action::Continue => {}
            Action::Confirm => return Ok(Some(state.chosen())),
            Action::Cancel => return Ok(None),
        }
    }
}

fn draw(term: &Term, state: &State, labels: &[String], sizes: &[u64]) -> io::Result<()> {
    term.write_line(&format!(
        "{} {}",
        style("?").yellow(),
        style("↑↓ 移动  空格 勾选  a 全选  回车 确认  Esc 放弃").dim()
    ))?;
    let width = term.size().1 as usize;
    let visible = labels
        .iter()
        .enumerate()
        .skip(state.offset)
        .take(state.height);
    for (i, text) in visible {
        let pointer = if i == state.cursor {
            style("❯").cyan().to_string()
        } else {
            " ".into()
        };
        let mark = if state.checked[i] {
            style("✔").green().to_string()
        } else {
            style("○").dim().to_string()
        };
        let label = console::truncate_str(text, width.saturating_sub(5), "…");
        let label = if i == state.cursor {
            style(label).bold().to_string()
        } else {
            label.into_owned()
        };
        term.write_line(&format!("{pointer} {mark} {label}"))?;
    }
    let chosen: Vec<usize> = state.chosen();
    let total: u64 = chosen.iter().map(|&i| sizes[i]).sum();
    term.write_line(&format!(
        "  {}",
        style(format!(
            "已选 {} 项 · {} · 第 {}/{} 项",
            chosen.len(),
            human(total),
            state.cursor + 1,
            labels.len()
        ))
        .dim()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_stops_at_both_ends() {
        let mut s = State::new(3, false, 10);
        s.handle(Key::ArrowUp);
        assert_eq!(s.cursor, 0);
        for _ in 0..5 {
            s.handle(Key::ArrowDown);
        }
        assert_eq!(s.cursor, 2);
        s.handle(Key::PageDown);
        assert_eq!(s.cursor, 2);
        s.handle(Key::Home);
        assert_eq!(s.cursor, 0);
        s.handle(Key::End);
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn viewport_follows_cursor() {
        let mut s = State::new(10, false, 3);
        for _ in 0..4 {
            s.handle(Key::ArrowDown);
        }
        assert_eq!((s.cursor, s.offset), (4, 2));
        s.handle(Key::PageUp);
        assert_eq!((s.cursor, s.offset), (1, 1));
        s.handle(Key::End);
        assert_eq!((s.cursor, s.offset), (9, 7));
    }

    #[test]
    fn toggle_and_select_all() {
        let mut s = State::new(3, true, 10);
        s.handle(Key::ArrowDown);
        s.handle(Key::Char(' '));
        assert_eq!(s.chosen(), [0, 2]);
        s.handle(Key::Char('a'));
        assert_eq!(s.chosen(), [0, 1, 2]);
        s.handle(Key::Char('a'));
        assert!(s.chosen().is_empty());
    }

    #[test]
    fn confirm_and_cancel() {
        let mut s = State::new(2, false, 10);
        assert!(matches!(s.handle(Key::Enter), Action::Confirm));
        assert!(matches!(s.handle(Key::Escape), Action::Cancel));
    }
}
