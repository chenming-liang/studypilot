//! 笔记浏览器的搜索与批量动作执行（Search→Select→Act，D6：直连内部函数）。

use std::sync::Arc;

use tokio::task::spawn_blocking;

use super::{App, Entry};
use crate::note_browser::{BatchAction, BrowserMode};

impl App {
    pub(crate) fn browser_search(&mut self) {
        let Some(b) = &mut self.note_browser else {
            return;
        };
        let seq = b.next_search_seq();
        b.loading = true;
        let query = self.input.clone();
        let scope = b.scope_course;
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = spawn_blocking(move || {
                store
                    .search_note_titles(&query, scope, 200)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = tx.send(crate::app::AppEvent::BrowserResults { seq, result });
        });
    }

    pub(crate) fn on_browser_results(
        &mut self,
        seq: u64,
        result: Result<Vec<storage::NoteSummary>, String>,
    ) {
        let Some(b) = &mut self.note_browser else {
            return;
        };
        if seq != b.search_seq {
            return; // 过期响应（用户已继续输入）
        }
        match result {
            Ok(rows) => b.results = rows,
            Err(e) => {
                b.loading = false;
                b.clamp_cursor();
                let msg = format!("搜索笔记失败: {e}");
                self.push_entry(Entry::Error(msg));
                return;
            }
        }
        b.loading = false;
        b.clamp_cursor();
    }

    /// m：进入移动流程——先选目标课程（内存列表 + all）。
    pub(crate) fn browser_begin_move(&mut self) {
        let Some(b) = &mut self.note_browser else {
            return;
        };
        let (ids, _) = b.action_targets();
        if ids.is_empty() {
            return;
        }
        b.action = Some(BatchAction::Move);
        b.mode = BrowserMode::PickTarget;
        b.move_target = None;
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// d：进入删除流程——确认页（>10 篇强确认输入 DELETE）。
    pub(crate) fn browser_begin_delete(&mut self) {
        let Some(b) = &mut self.note_browser else {
            return;
        };
        let (ids, _) = b.action_targets();
        if ids.is_empty() {
            return;
        }
        b.action = Some(BatchAction::Delete);
        b.mode = BrowserMode::Confirm;
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// PickTarget：记录目标课程（None=all），进入移动确认页。
    pub(crate) fn browser_set_move_target(&mut self, id: Option<i64>, label: String) {
        let Some(b) = &mut self.note_browser else {
            return;
        };
        b.move_target = id;
        b.move_target_label = label;
        b.mode = BrowserMode::Confirm;
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// 确认页 Enter：执行动作并关闭浏览器。
    pub(crate) fn browser_confirm(&mut self) {
        let Some(b) = &self.note_browser else {
            return;
        };
        let Some(action) = b.action else {
            return;
        };
        let (ids, _titles) = b.action_targets();
        let count = ids.len();
        // 强确认（删除 >10 篇）：输入必须等于 DELETE
        if action == BatchAction::Delete
            && count > crate::note_browser::STRONG_CONFIRM_THRESHOLD
            && self.input.trim() != "DELETE"
        {
            self.push_entry(Entry::Info("请输入 DELETE 以确认批量删除".into()));
            return;
        }
        let scope_label = b.scope_label.clone();
        let target_label = b.move_target_label.clone();
        let course_id = b.move_target;
        self.note_browser = None;
        self.drop_input_backup();
        self.input.clear();
        self.cursor_pos = 0;

        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = spawn_blocking(move || -> Result<String, String> {
                match action {
                    BatchAction::Move => {
                        // course_id=None = all 区，同样是合法目标
                        let mut ok = 0usize;
                        for id in &ids {
                            if store.move_note(*id, course_id).unwrap_or(false) {
                                ok += 1;
                            }
                        }
                        Ok(format!("已移动 {ok}/{count} 篇笔记 → {target_label}"))
                    }
                    BatchAction::Delete => {
                        // 只删知识库数据，磁盘原文件不动（安全红线）
                        let deleted = store.delete_notes_by_ids(&ids).map_err(|e| e.to_string())?;
                        Ok(format!("已删除 {deleted} 篇笔记（原始文件未受影响）"))
                    }
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = tx.send(crate::app::AppEvent::BrowserActionDone {
                scope_label,
                result,
            });
        });
    }

    /// 动作完成回填（关闭浏览器后仍需报告 + 刷新课程统计）。
    pub(crate) fn on_browser_action_done(
        &mut self,
        scope_label: String,
        result: Result<String, String>,
    ) {
        match result {
            Ok(msg) => {
                self.push_entry(Entry::Info(format!("✓ {msg}（范围：{scope_label}）")));
            }
            Err(e) => self.push_entry(Entry::Error(format!("批量操作失败: {e}"))),
        }
        self.request_courses_refresh();
    }
}
