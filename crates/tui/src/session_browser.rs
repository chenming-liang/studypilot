//! 会话浏览器：Search → Select → Act（收口会话⑨，范式与笔记浏览器一致）。
//!
//! 动作：Enter/o 恢复会话 · r 重命名（任意历史会话）· d 删除（级联删聊天记录）。
//! 安全线：禁止删除当前打开的会话（正被 session_state 引用写消息）；
//! 删除确认页明示"聊天记录将被删除且无法恢复"（会话没有外部副本，语气比笔记重）。

use storage::SessionMeta;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBrowserMode {
    /// 输入即过滤（共享聊天框缓冲）
    Search,
    /// 导航与动作
    Select,
    /// 重命名输入（r 触发；App.input 作编辑缓冲）
    Rename,
    /// 删除确认
    ConfirmDelete,
}

pub struct SessionBrowser {
    pub mode: SessionBrowserMode,
    pub results: Vec<SessionMeta>,
    pub cursor: usize,
    pub scroll: usize,
    /// 重命名目标（进入 Rename 时固化）
    pub rename_id: Option<i64>,
    pub rename_old: String,
    /// 异步搜索序号
    pub search_seq: u64,
    pub loading: bool,
}

/// 弹窗内列表可视行数（height 20 - 边框 2 - 余量）
pub const SESSION_LIST_VISIBLE: usize = 15;

impl SessionBrowser {
    pub fn new() -> Self {
        Self {
            mode: SessionBrowserMode::Search,
            results: Vec::new(),
            cursor: 0,
            scroll: 0,
            rename_id: None,
            rename_old: String::new(),
            search_seq: 0,
            loading: true,
        }
    }

    pub(crate) fn next_search_seq(&mut self) -> u64 {
        self.search_seq += 1;
        self.search_seq
    }

    /// 光标可见性对齐（Select 模式）。
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

    /// 光标随结果集收缩钳制。
    pub fn clamp_cursor(&mut self) {
        if self.results.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(self.results.len() - 1);
        }
    }

    /// Search 模式结果预览滚动（无光标，按页 ±3）。
    pub fn scroll_preview(&mut self, delta: isize, visible: usize) {
        let total = self.results.len();
        let max_scroll = total.saturating_sub(visible);
        let cur = self.scroll as isize + delta;
        self.scroll = cur.clamp(0, max_scroll as isize) as usize;
    }

    /// 当前光标会话。
    pub fn current(&self) -> Option<&SessionMeta> {
        self.results.get(self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: i64, title: &str) -> SessionMeta {
        SessionMeta {
            id,
            title: Some(title.to_owned()),
            course_id: Some(1),
            created_at: "2026-09-02 10:00:00".into(),
        }
    }

    #[test]
    fn cursor_visibility_alignment() {
        let mut b = SessionBrowser::new();
        b.results = (0..30).map(|i| meta(i, "s")).collect();
        b.cursor = 25;
        b.ensure_cursor_visible(SESSION_LIST_VISIBLE);
        assert_eq!(b.scroll, 25 + 1 - SESSION_LIST_VISIBLE);
        b.cursor = 2;
        b.ensure_cursor_visible(SESSION_LIST_VISIBLE);
        assert_eq!(b.scroll, 2);
    }
}
