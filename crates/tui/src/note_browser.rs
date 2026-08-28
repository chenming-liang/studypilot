//! 笔记浏览器：Search → Select → Act 统一交互（收口会话⑧）。
//!
//! 一个可搜索的笔记对象集合 + 多选 + 批量动作。设计约束：
//! - 动作触发收敛在浏览器内（m 移动 / d 删除），palette/命令不重复入口
//! - id 不暴露给用户（列表行只显示标题与课程），定位走内部 id
//! - 安全线：确认页始终写明"已选择的 N 篇"而非"搜索结果 N 篇"，
//!   且明确"原始文件不会被删除"（D 红线的用户可见面）
//! - 删除强确认：>10 篇须输入 DELETE

use std::collections::HashSet;

use storage::NoteSummary;

/// 浏览器所处状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMode {
    /// 输入即过滤（共享聊天框缓冲），Enter 进入选择模式
    Search,
    /// 导航与多选：↑↓ 导航、Space 选中、Ctrl+A 全选结果、m/d 触发动作、/ 回搜索
    Select,
    /// 移动的目标课程选择（↑↓ + Enter）
    PickTarget,
    /// 动作确认页（删除 >10 篇时需输入 DELETE）
    Confirm,
}

/// 待执行动作（选择完成后由 m/d 设置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchAction {
    Move,
    Delete,
}

pub struct NoteBrowser {
    pub mode: BrowserMode,
    /// 选择完成后设置的动作（PickTarget/Confirm 依据它执行）
    pub action: Option<BatchAction>,
    /// 当前过滤结果
    pub results: Vec<NoteSummary>,
    /// 已选中的笔记 id（动作只作用于这个集合；空集时光标行即隐式选择）
    pub selected: HashSet<i64>,
    /// 选择模式光标（results 中的行号）
    pub cursor: usize,
    /// 范围：Some(课程 id)=该课程，None=全部。打开时固化，Phase 1 不提供切换
    pub scope_course: Option<i64>,
    /// 范围显示名（header 用）
    pub scope_label: String,
    /// 移动目标课程 id
    pub move_target: Option<i64>,
    /// PickTarget 子状态光标
    pub pick_cursor: usize,
    /// 移动目标显示名（确认页用）
    pub move_target_label: String,
    /// 异步搜索序号：乱序到达的旧结果丢弃
    pub search_seq: u64,
    /// 结果列表滚动起点（Search 预览与 Select 光标共用）
    pub scroll: usize,
    pub loading: bool,
}

/// 强确认阈值：删除篇数超过它需输入 DELETE
pub const STRONG_CONFIRM_THRESHOLD: usize = 10;
/// 弹窗内列表可视行数（height 18 - 边框 2 - 提示余量）
pub const LIST_VISIBLE: usize = 14;

impl NoteBrowser {
    pub fn new(scope_course: Option<i64>, scope_label: String) -> Self {
        Self {
            mode: BrowserMode::Search,
            action: None,
            results: Vec::new(),
            selected: HashSet::new(),
            cursor: 0,
            scope_course,
            scope_label,
            move_target: None,
            pick_cursor: 0,
            move_target_label: String::new(),
            search_seq: 0,
            scroll: 0,
            loading: true,
        }
    }

    /// 发起一次异步搜索的序号（调用方用它丢弃过期响应）。
    pub(crate) fn next_search_seq(&mut self) -> u64 {
        self.search_seq += 1;
        self.search_seq
    }

    /// 供渲染的已选数量。
    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// 动作的目标集合：已选集非空用它；否则光标当前行（单篇快速路径）。
    /// 返回 (ids, 作用对象的显示标题)。
    pub fn action_targets(&self) -> (Vec<i64>, Vec<String>) {
        if !self.selected.is_empty() {
            let ids: Vec<i64> = {
                let mut v: Vec<i64> = self.selected.iter().copied().collect();
                v.sort_unstable();
                v
            };
            let titles = ids
                .iter()
                .filter_map(|id| self.results.iter().find(|n| n.id == *id))
                .map(|n| n.title.clone())
                .collect();
            (ids, titles)
        } else if let Some(n) = self.results.get(self.cursor) {
            (vec![n.id], vec![n.title.clone()])
        } else {
            (Vec::new(), Vec::new())
        }
    }

    /// 滚动起点随光标对齐（经典 listbox 逻辑）：保证 cursor 行落在视口内。
    pub fn ensure_cursor_visible(&mut self, visible: usize) {
        if visible == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + visible {
            self.scroll = self.cursor + 1 - visible;
        }
        let max_scroll = self.results.len().saturating_sub(visible);
        self.scroll = self.scroll.min(max_scroll);
    }

    /// Search 模式的结果预览滚动（无光标，按页 ±3）。
    pub fn scroll_preview(&mut self, delta: isize, visible: usize) {
        let total = self.results.len();
        let max_scroll = total.saturating_sub(visible);
        let cur = self.scroll as isize + delta;
        self.scroll = cur.clamp(0, max_scroll as isize) as usize;
    }

    /// 光标随结果集收缩钳制。
    pub(crate) fn clamp_cursor(&mut self) {
        if self.results.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(self.results.len() - 1);
        }
    }

    pub fn toggle_current(&mut self) {
        if let Some(n) = self.results.get(self.cursor) {
            let id = n.id;
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
        }
    }

    /// 全选当前过滤结果。
    pub fn select_all_results(&mut self) {
        for n in &self.results {
            self.selected.insert(n.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn summary(id: i64, title: &str) -> NoteSummary {
        NoteSummary {
            id,
            course_id: Some(1),
            title: title.to_owned(),
            short_id: "abcd".into(),
        }
    }

    fn browser_with_results() -> NoteBrowser {
        let mut b = NoteBrowser::new(Some(1), "rust".into());
        b.mode = BrowserMode::Select;
        b.results = vec![
            summary(1, "所有权"),
            summary(2, "借用"),
            summary(3, "生命周期"),
        ];
        b
    }

    #[test]
    fn empty_selection_uses_cursor_row() {
        let mut b = browser_with_results();
        b.cursor = 1;
        let (ids, titles) = b.action_targets();
        assert_eq!(ids, vec![2]);
        assert_eq!(titles, vec!["借用"]);
    }

    #[test]
    fn selection_wins_over_cursor() {
        let mut b = browser_with_results();
        b.cursor = 1;
        b.selected.insert(3);
        let (ids, _) = b.action_targets();
        assert_eq!(ids, vec![3]);
    }

    #[test]
    fn toggle_and_select_all() {
        let mut b = browser_with_results();
        b.cursor = 0;
        b.toggle_current();
        assert_eq!(b.selected_count(), 1);
        b.toggle_current();
        assert_eq!(b.selected_count(), 0);
        b.select_all_results();
        assert_eq!(b.selected_count(), 3);
    }

    #[test]
    fn cursor_clamps_when_results_shrink() {
        let mut b = browser_with_results();
        b.cursor = 2;
        b.results = vec![summary(1, "所有权")];
        b.clamp_cursor();
        assert_eq!(b.cursor, 0);
    }

    #[test]
    fn empty_results_no_targets() {
        let mut b = NoteBrowser::new(Some(1), "rust".into());
        b.mode = BrowserMode::Select;
        assert!(b.action_targets().0.is_empty());
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::super::tests::summary;
    use super::*;

    fn b_with(n: usize) -> NoteBrowser {
        let mut b = NoteBrowser::new(Some(1), "rust".into());
        b.results = (0..n)
            .map(|i| summary(i as i64, &format!("t{i}")))
            .collect();
        b
    }

    #[test]
    fn cursor_below_window_pulls_scroll_up() {
        let mut b = b_with(30);
        b.scroll = 10;
        b.cursor = 2;
        b.ensure_cursor_visible(crate::note_browser::LIST_VISIBLE);
        assert_eq!(b.scroll, 2);
    }

    #[test]
    fn cursor_beyond_window_pushes_scroll_down() {
        let mut b = b_with(30);
        b.cursor = 20;
        b.ensure_cursor_visible(crate::note_browser::LIST_VISIBLE);
        assert_eq!(b.scroll, 20 + 1 - crate::note_browser::LIST_VISIBLE);
    }

    #[test]
    fn scroll_clamps_to_total() {
        let mut b = b_with(5);
        b.scroll_preview(100, crate::note_browser::LIST_VISIBLE);
        assert_eq!(b.scroll, 0, "总数小于视口时滚动封底");
        let mut b = b_with(30);
        b.scroll_preview(100, 5);
        assert_eq!(b.scroll, 25);
        b.scroll_preview(-100, 5);
        assert_eq!(b.scroll, 0);
    }

    #[test]
    fn small_result_no_scroll() {
        let mut b = b_with(3);
        b.cursor = 2;
        b.ensure_cursor_visible(crate::note_browser::LIST_VISIBLE);
        assert_eq!(b.scroll, 0);
    }
}
