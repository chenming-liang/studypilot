//! 按键/鼠标路由：主输入、复习、面板、向导、弹窗、拖选复制。

use std::sync::Arc;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::{App, Entry, Wizard};
use crate::clipboard::copy_to_clipboard;
use crate::input_edit::{delete_at, delete_before, insert_char};
use crate::palette::CommandPalette;
use crate::review;

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // 键盘操作清除文本选区（防止残留高亮）
        self.text_selection = None;
        self.selection_anchor = None;
        // 复习模式优先处理
        if self.review.is_some() {
            self.handle_review_key(key);
            return;
        }
        // 参数向导（palette Enter 触发）：导航键拦截，编辑键落入普通路径
        // （向导/面板与聊天框共享同一输入缓冲，fzf 风格）
        if self.wizard.is_some() && self.handle_wizard_key(key) {
            return;
        }
        // 列表选择器（面板 Pick 动作唤起）
        if self.list_picker.is_some() {
            self.handle_list_picker_key(key);
            return;
        }
        // 笔记浏览器：Search 模式编辑键落入普通路径（共享输入缓冲）
        if self.note_browser.is_some() && self.handle_browser_key(key) {
            return;
        }
        // 会话浏览器：Search/Rename 编辑键落入普通路径
        if self.session_browser.is_some() && self.handle_session_browser_key(key) {
            return;
        }
        // 命令面板（Ctrl+K 唤起）：导航键拦截，编辑键落入普通路径
        if self.palette.is_some() && self.handle_palette_key(key) {
            return;
        }
        // 弹窗打开时按键优先由弹窗处理
        if self.model_picker.is_some() {
            self.handle_picker_key(key);
            return;
        }
        // Ctrl+K 打开命令面板（接管聊天框输入作过滤缓冲）
        if let KeyCode::Char('k') = key.code
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.take_input_for_overlay();
            self.palette = Some(CommandPalette::new());
            return;
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.interrupt_or_quit();
            }
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit_now();
            }
            KeyCode::Esc => self.interrupt_or_quit(),
            KeyCode::Enter => self.submit(),
            KeyCode::Left => {
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right => {
                let len = self.input.chars().count();
                self.cursor_pos = (self.cursor_pos + 1).min(len);
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.input.chars().count(),
            KeyCode::Backspace => {
                self.cursor_pos = delete_before(&mut self.input, self.cursor_pos);
                self.sync_palette_filter();
                self.sync_browser_search();
                self.sync_session_browser_search();
            }
            KeyCode::Delete => {
                delete_at(&mut self.input, self.cursor_pos);
                self.sync_palette_filter();
                self.sync_browser_search();
                self.sync_session_browser_search();
            }
            KeyCode::PageUp => self.scroll_up = self.scroll_up.saturating_add(10),
            KeyCode::PageDown => self.scroll_up = self.scroll_up.saturating_sub(10),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor_pos = insert_char(&mut self.input, self.cursor_pos, c);
                self.sync_palette_filter();
                self.sync_browser_search();
                self.sync_session_browser_search();
            }
            _ => {}
        }
    }

    /// 浏览器搜索模式下，聊天框缓冲即搜索词——编辑后重新发起异步搜索
    fn sync_browser_search(&mut self) {
        let in_search = self
            .note_browser
            .as_ref()
            .map(|b| b.mode == crate::note_browser::BrowserMode::Search)
            .unwrap_or(false);
        if in_search {
            self.browser_search();
        }
    }

    /// 会话浏览器搜索模式下同步过滤
    fn sync_session_browser_search(&mut self) {
        let in_search = self
            .session_browser
            .as_ref()
            .map(|b| b.mode == crate::session_browser::SessionBrowserMode::Search)
            .unwrap_or(false);
        if in_search {
            self.session_browser_search();
        }
    }

    /// 面板打开期间，聊天框缓冲即过滤串——编辑后同步过滤结果。
    fn sync_palette_filter(&mut self) {
        if let Some(p) = &mut self.palette {
            let input = self.input.clone();
            p.refilter(&input);
        }
    }

    /// 鼠标：滚轮滚动 + 左键拖选复制聊天内容。
    pub(crate) async fn handle_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_up = self.scroll_up.saturating_sub(3);
            }
            MouseEventKind::ScrollUp => {
                self.scroll_up = self.scroll_up.saturating_add(3);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.update_text_selection(event.row, event.column, true);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.update_text_selection(event.row, event.column, false);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // 同步等待复制完成（fd 1/2 重定向窗口内主线程不得 draw，
                // 否则该帧输出进 /dev/null → 残留高亮/画面污染）
                self.copy_text_selection().await;
            }
            _ => {}
        }
    }

    /// 根据鼠标位置更新文本选区（仅限聊天区内）+ 边缘自动翻页。
    pub(crate) fn update_text_selection(&mut self, row: u16, col: u16, is_down: bool) {
        // 聊天区无边框：chat_rect 即内容区（渲染从 area.y 画第一行），
        // 坐标映射必须与渲染端完全同系——此前误用 inner(Margin 1,1)，
        // 导致选中恒偏上一行。
        let inner = self.chat_rect;
        if !inner.contains(ratatui::layout::Position { x: col, y: row }) {
            return;
        }

        // 自动翻页：贴近视口上下边缘时滚动（拖选超出可视范围时跟随）
        let max_offset = self.chat_lines.len().saturating_sub(inner.height as usize);
        let near_top = row <= inner.y;
        let near_bottom = row >= inner.bottom().saturating_sub(1);
        if near_top && max_offset > 0 {
            self.scroll_up = (self.scroll_up + 3).min(max_offset as u16);
        } else if near_bottom {
            self.scroll_up = self.scroll_up.saturating_sub(3);
        }

        // 屏幕 row → 渲染行索引（用更新后的 scroll_top，保持与 draw 一致）
        let scroll_top = max_offset.saturating_sub(self.scroll_up as usize);
        let visible_row = (row - inner.y) as usize;
        let line_idx = (scroll_top + visible_row).min(self.chat_lines.len().saturating_sub(1));
        tracing::debug!(row, col, line_idx, "拖选更新");

        if is_down {
            // 左键按下：定锚，锚点在 Drag 期间不漂移
            self.selection_anchor = Some(line_idx);
            self.text_selection = Some((line_idx, line_idx));
            return;
        }
        // Drag：以固定锚点为基准双向扩展（上/下方向对称）
        match self.selection_anchor {
            Some(anchor) => {
                let lo = anchor.min(line_idx);
                let hi = anchor.max(line_idx);
                self.text_selection = Some((lo, hi));
            }
            None => {
                // 极端情况：Drag 先于 Down 到达，当作起点
                self.selection_anchor = Some(line_idx);
                self.text_selection = Some((line_idx, line_idx));
            }
        }
    }

    /// 释放鼠标：提取选中文本，复制到系统剪贴板。
    /// 释放鼠标：提取选中文本，复制到系统剪贴板。
    /// 同步执行；任何内部 panic 被捕获转为 Err，绝不杀死 TUI。
    pub(crate) async fn copy_text_selection(&mut self) {
        self.selection_anchor = None;
        let Some((start, end)) = self.text_selection.take() else {
            return;
        };
        // 双端钳制，防越界切片
        let last = self.chat_lines.len().saturating_sub(1);
        let start = start.min(last);
        let end = end.min(last);
        let text = self.chat_lines[start..=end].join("\n");
        if text.trim().is_empty() {
            return;
        }
        let count = end - start + 1;

        // 同步执行：等 fd 1/2 恢复后再返回，下一帧 draw 才不会丢输出
        let result = tokio::task::spawn_blocking(move || copy_to_clipboard(&text))
            .await
            .unwrap_or_else(|e| Err(format!("任务错误: {e}")));

        // 复制反馈走右上角 toast（不插入聊天流）
        match result {
            Ok(()) => self.set_toast(format!("已复制 {count} 行到剪贴板"), false),
            Err(e) => self.set_toast(format!("复制失败: {e}"), true),
        }
    }

    /// 复习模式按键处理：选择题 1-4 / 简答题 Enter 提交 / Esc 退出。
    pub(crate) fn handle_review_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // 批改进行中：禁止再次提交或切题（并发批改会错位污染 attempts/mastery）
        if self.review_grading {
            if key.code == KeyCode::Esc {
                self.exit_review("已退出复习模式（批改结果将不作记录）");
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.exit_review("已退出复习模式");
            }
            KeyCode::Enter => {
                // 简答题提交：先提取数据，避免 borrow 冲突
                let (idx, q_clone) = match &self.review {
                    Some(rs) => match rs.questions.get(rs.current) {
                        Some(q) if q.q_type == review::QType::ShortAnswer => {
                            (rs.current, q.clone())
                        }
                        _ => return,
                    },
                    None => return,
                };
                let answer = self.input.trim().to_owned();
                self.input.clear();
                self.cursor_pos = 0;
                if answer.is_empty() {
                    return;
                }
                // 防止并发批改：同题多任务会错位污染 attempts/mastery
                self.review_grading = true;
                self.push_entry(Entry::User(format!("答: {answer}")));
                self.push_entry(Entry::Info("批改中…".into()));
                let provider = self.provider.clone();
                let provider_cfg = self.provider_cfg.clone();
                let store = Arc::clone(&self.store);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    review::grade_short_answer(
                        provider,
                        provider_cfg,
                        store,
                        &q_clone,
                        &answer,
                        tx,
                        idx,
                    )
                    .await;
                });
            }
            KeyCode::Char(c @ '1'..='9') => {
                // 选择题作答：先提取数据
                let (q_clone, _current) = match &self.review {
                    Some(rs) => match rs.questions.get(rs.current) {
                        Some(q) if q.q_type == review::QType::Choice => (q.clone(), rs.current),
                        _ => return,
                    },
                    None => return,
                };
                let choice = (c as u8 - b'1') as usize;
                if choice >= q_clone.options.len() {
                    return;
                }
                let user_letter = (b'A' + choice as u8) as char;

                // answer 缺失或越界（LLM 输出不可控）：不计分，提示后跳过
                let Some(correct_idx) = q_clone.answer else {
                    self.push_entry(Entry::User(format!("选 {user_letter}")));
                    self.push_entry(Entry::Error(
                        "该题缺少标准答案（LLM 未生成），无法判分，跳过此题".into(),
                    ));
                    self.finish_review_question(false, None, "答案缺失跳过", &[]);
                    return;
                };
                if correct_idx < 0 || correct_idx as usize >= q_clone.options.len() {
                    self.push_entry(Entry::User(format!("选 {user_letter}")));
                    self.push_entry(Entry::Error(format!(
                        "该题答案下标非法 ({correct_idx})，无法判分，跳过此题"
                    )));
                    self.finish_review_question(false, None, "答案非法跳过", &[]);
                    return;
                }

                let is_correct = choice as i64 == correct_idx;
                let correct_letter = (b'A' + correct_idx as u8) as char;
                let feedback = if is_correct {
                    format!("✓ 正确（选 {user_letter}）")
                } else {
                    format!("✗ 错误（选 {user_letter}，正确答案: {correct_letter}）")
                };
                self.push_entry(Entry::User(format!("选 {user_letter}")));
                self.push_entry(Entry::Info(feedback.clone()));
                if let Some(exp) = &q_clone.explanation {
                    self.push_entry(Entry::Info(format!("解析: {exp}")));
                }
                self.finish_review_question(is_correct, None, &feedback, &[]);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let is_short = self
                    .review
                    .as_ref()
                    .and_then(|rs| rs.questions.get(rs.current))
                    .map(|q| q.q_type == review::QType::ShortAnswer)
                    .unwrap_or(false);
                if is_short {
                    self.cursor_pos = insert_char(&mut self.input, self.cursor_pos, c);
                }
            }
            KeyCode::Backspace => {
                let is_short = self
                    .review
                    .as_ref()
                    .and_then(|rs| rs.questions.get(rs.current))
                    .map(|q| q.q_type == review::QType::ShortAnswer)
                    .unwrap_or(false);
                if is_short {
                    self.cursor_pos = delete_before(&mut self.input, self.cursor_pos);
                }
            }
            KeyCode::Left => {
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right => {
                self.cursor_pos = (self.cursor_pos + 1).min(self.input.chars().count());
            }
            _ => {}
        }
    }

    /// 浏览器按键：返回 true 表示已消费（导航/动作键）；
    /// Search 模式的编辑键返回 false 落入普通聊天框编辑路径。
    pub(crate) fn handle_browser_key(&mut self, key: KeyEvent) -> bool {
        use crate::note_browser::BrowserMode;
        if key.kind != KeyEventKind::Press {
            return false;
        }
        let Some(b) = &self.note_browser else {
            return false;
        };
        match b.mode {
            BrowserMode::Search => match key.code {
                KeyCode::Up => {
                    if let Some(b) = &mut self.note_browser {
                        b.scroll_preview(-3, crate::note_browser::LIST_VISIBLE.saturating_sub(3));
                    }
                    true
                }
                KeyCode::Down => {
                    if let Some(b) = &mut self.note_browser {
                        b.scroll_preview(3, crate::note_browser::LIST_VISIBLE.saturating_sub(3));
                    }
                    true
                }
                KeyCode::Esc => {
                    self.note_browser = None;
                    self.restore_input_backup();
                    true
                }
                KeyCode::Enter => {
                    if let Some(b) = &mut self.note_browser {
                        b.mode = BrowserMode::Select;
                        b.clamp_cursor();
                    }
                    true
                }
                _ => false, // 编辑键 → 普通路径（sync_browser_search 已挂）
            },
            BrowserMode::Select => match key.code {
                KeyCode::Esc => {
                    if let Some(b) = &mut self.note_browser {
                        b.mode = BrowserMode::Search;
                        b.selected.clear();
                        self.input.clear();
                        self.cursor_pos = 0;
                        self.browser_search();
                    }
                    true
                }
                KeyCode::Up => {
                    if let Some(b) = &mut self.note_browser {
                        b.cursor = b.cursor.saturating_sub(1);
                    }
                    true
                }
                KeyCode::Down => {
                    if let Some(b) = &mut self.note_browser
                        && !b.results.is_empty()
                    {
                        b.cursor = (b.cursor + 1).min(b.results.len() - 1);
                    }
                    true
                }
                KeyCode::Char(' ') => {
                    if let Some(b) = &mut self.note_browser {
                        b.toggle_current();
                    }
                    true
                }
                KeyCode::Char('a') | KeyCode::Char('A')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if let Some(b) = &mut self.note_browser {
                        b.select_all_results();
                    }
                    true
                }
                KeyCode::Char('/') | KeyCode::Enter => {
                    if let Some(b) = &mut self.note_browser {
                        b.mode = BrowserMode::Search;
                    }
                    self.input.clear();
                    self.cursor_pos = 0;
                    self.browser_search(); // 与结果集对齐
                    true
                }
                KeyCode::Char('m') => {
                    self.browser_begin_move();
                    true
                }
                KeyCode::Char('d') => {
                    self.browser_begin_delete();
                    true
                }
                _ => true, // Select 模式其余键不落到聊天框
            },
            BrowserMode::PickTarget => match key.code {
                KeyCode::Esc => {
                    if let Some(b) = &mut self.note_browser {
                        b.mode = BrowserMode::Select;
                        b.action = None;
                    }
                    self.input.clear();
                    true
                }
                KeyCode::Up | KeyCode::Down => {
                    // 光标由渲染层维护 pick_cursor
                    let len = self.pick_items().len();
                    if let (Some(b), true) = (&mut self.note_browser, len > 0) {
                        b.pick_cursor = match key.code {
                            KeyCode::Up => b.pick_cursor.saturating_sub(1),
                            _ => (b.pick_cursor + 1).min(len - 1),
                        };
                    }
                    true
                }
                KeyCode::Enter => {
                    let pick_cursor = self
                        .note_browser
                        .as_ref()
                        .map(|b| b.pick_cursor)
                        .unwrap_or(0);
                    let picked = self.pick_items().into_iter().nth(pick_cursor);
                    let (id, label) = match picked {
                        Some((i, n)) => (i, n),
                        None => return true,
                    };
                    // all 哨兵 id=-1 → None（course_id IS NULL 即 all 区）
                    let target = if id < 0 { None } else { Some(id) };
                    self.browser_set_move_target(target, label);
                    true
                }
                _ => true,
            },
            BrowserMode::Confirm => match key.code {
                KeyCode::Esc => {
                    if let Some(b) = &mut self.note_browser {
                        b.mode = BrowserMode::Select;
                        b.action = None;
                    }
                    self.input.clear();
                    true
                }
                KeyCode::Enter => {
                    self.browser_confirm();
                    true
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // 强确认需要输入 DELETE
                    self.cursor_pos = insert_char(&mut self.input, self.cursor_pos, c);
                    true
                }
                KeyCode::Backspace => {
                    self.cursor_pos = delete_before(&mut self.input, self.cursor_pos);
                    true
                }
                _ => true,
            },
        }
    }

    /// 打开复习参数向导（手输无参 /review 与面板共用；课程自动取当前分区）。
    pub(crate) fn open_review_wizard(&mut self) {
        if self.review.is_some() {
            self.set_toast("复习进行中，请先完成或 Esc 退出", true);
            return;
        }
        let course_id = self.current_course_id();
        let course_name = self.course.clone();
        self.take_input_for_overlay();
        self.wizard = Some(Wizard::new_review(course_id, course_name));
        self.enter_wizard_step();
    }

    /// 会话浏览器按键：返回 true 表示已消费。
    pub(crate) fn handle_session_browser_key(&mut self, key: KeyEvent) -> bool {
        use crate::session_browser::{SESSION_LIST_VISIBLE, SessionBrowserMode};
        if key.kind != KeyEventKind::Press {
            return false;
        }
        let Some(b) = &self.session_browser else {
            return false;
        };
        match b.mode {
            SessionBrowserMode::Search => match key.code {
                KeyCode::Esc => {
                    self.session_browser = None;
                    self.restore_input_backup();
                    true
                }
                KeyCode::Enter => {
                    if let Some(b) = &mut self.session_browser {
                        b.mode = SessionBrowserMode::Select;
                        b.clamp_cursor();
                    }
                    true
                }
                KeyCode::Up | KeyCode::Down => {
                    if let Some(b) = &mut self.session_browser {
                        b.scroll_preview(
                            if key.code == KeyCode::Up { -3 } else { 3 },
                            SESSION_LIST_VISIBLE,
                        );
                    }
                    true
                }
                _ => false,
            },
            SessionBrowserMode::Select => match key.code {
                KeyCode::Esc => {
                    if let Some(b) = &mut self.session_browser {
                        b.mode = SessionBrowserMode::Search;
                        self.input.clear();
                        self.cursor_pos = 0;
                        self.session_browser_search();
                    }
                    true
                }
                KeyCode::Up => {
                    if let Some(b) = &mut self.session_browser {
                        b.cursor = b.cursor.saturating_sub(1);
                        b.ensure_cursor_visible(SESSION_LIST_VISIBLE);
                    }
                    true
                }
                KeyCode::Down => {
                    if let Some(b) = &mut self.session_browser
                        && !b.results.is_empty()
                    {
                        b.cursor = (b.cursor + 1).min(b.results.len() - 1);
                        b.ensure_cursor_visible(SESSION_LIST_VISIBLE);
                    }
                    true
                }
                KeyCode::Enter | KeyCode::Char('o') => {
                    self.session_browser_open();
                    true
                }
                KeyCode::Char('r') => {
                    self.session_browser_begin_rename();
                    true
                }
                KeyCode::Char('d') => {
                    self.session_browser_begin_delete();
                    true
                }
                KeyCode::Char('/') => {
                    if let Some(b) = &mut self.session_browser {
                        b.mode = SessionBrowserMode::Search;
                    }
                    self.input.clear();
                    self.cursor_pos = 0;
                    self.session_browser_search(); // 与结果集对齐
                    true
                }
                _ => true,
            },
            SessionBrowserMode::Rename => match key.code {
                KeyCode::Esc => {
                    // 取消：恢复搜索词
                    if let Some(b) = &mut self.session_browser {
                        b.mode = SessionBrowserMode::Select;
                        self.input = b.rename_old.clone();
                        self.cursor_pos = self.input.chars().count();
                    }
                    true
                }
                KeyCode::Enter => {
                    self.session_browser_confirm_rename();
                    true
                }
                KeyCode::Backspace => {
                    self.cursor_pos = delete_before(&mut self.input, self.cursor_pos);
                    true
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.cursor_pos = insert_char(&mut self.input, self.cursor_pos, c);
                    true
                }
                _ => true,
            },
            SessionBrowserMode::ConfirmDelete => match key.code {
                KeyCode::Esc => {
                    if let Some(b) = &mut self.session_browser {
                        b.mode = SessionBrowserMode::Select;
                    }
                    self.input.clear();
                    true
                }
                KeyCode::Enter => {
                    self.session_browser_confirm_delete();
                    true
                }
                _ => true,
            },
        }
    }

    /// PickTarget 的候选列表（all + 全部课程），渲染与按键共用。
    fn pick_items(&self) -> Vec<(i64, String)> {
        let mut items = vec![(-1i64, "all（全部）".to_owned())];
        items.extend(self.courses.iter().cloned());
        items
    }

    /// 面板导航键（编辑键由普通聊天框路径处理——共享同一输入缓冲）。
    /// 返回 true 表示该键已被面板消费。
    pub(crate) fn handle_palette_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                self.palette = None;
                self.restore_input_backup();
                true
            }
            KeyCode::Up => {
                if let Some(p) = &mut self.palette {
                    p.selected = p.selected.saturating_sub(1);
                }
                true
            }
            KeyCode::Down => {
                if let Some(p) = &mut self.palette
                    && !p.filtered.is_empty()
                {
                    p.selected = (p.selected + 1).min(p.filtered.len() - 1);
                }
                true
            }
            KeyCode::Enter | KeyCode::Tab => {
                let fill_only = key.code == KeyCode::Tab;
                self.palette_execute(fill_only);
                true
            }
            _ => false,
        }
    }

    /// 执行面板选中项：行为由条目的 PaletteAction 决定（向导/选择器/单步输入/
    /// 直执行）；Tab 一律填入文本模式（power-user 兜底）。
    pub(crate) fn palette_execute(&mut self, fill_only: bool) {
        use crate::palette::PaletteAction as A;
        let Some(p) = &self.palette else {
            return;
        };
        let Some(&idx) = p.filtered.get(p.selected) else {
            return;
        };
        let item = &p.items[idx];
        let cmd = item.command;
        let action = item.action.clone();
        self.palette = None;
        // 输入缓冲将被命令文本/向导接管：备份使命结束
        if !fill_only {
            self.drop_input_backup();
        }

        if !fill_only {
            match action {
                A::Run => {
                    self.input = cmd.to_owned();
                    self.cursor_pos = self.input.chars().count();
                    self.submit();
                    return;
                }
                A::WizardReview => {
                    self.open_review_wizard();
                    return;
                }
                A::WizardImport => {
                    self.wizard = Some(Wizard::new_import(self.course.clone()));
                    self.enter_wizard_step();
                    return;
                }
                A::Prompt {
                    title,
                    prompt,
                    kind,
                } => {
                    if kind == crate::wizard::WizardKind::RenameSession
                        && self.current_session_id().is_none()
                    {
                        self.push_entry(Entry::Error(
                            "当前没有已持久化的聊天会话——先发一条消息创建会话".into(),
                        ));
                        return;
                    }
                    self.wizard = Some(Wizard::new_for(kind, title, prompt));
                    self.enter_wizard_step();
                    return;
                }
                A::Pick(kind) => {
                    self.open_list_picker(kind);
                    return;
                }
            }
        }
        // Tab 或 Fill：填入输入框（power-user 文本模式）
        self.restore_input_backup();
        self.input = cmd.to_owned();
        self.cursor_pos = self.input.chars().count();
    }

    /// 打开列表选择器：数据源全部来自内存缓存（courses / sidebar_sessions）。
    pub(crate) fn open_list_picker(&mut self, kind: crate::palette::PickKind) {
        use crate::palette::{ListChoice, ListPicker, PickKind as K};
        let (title, items) = match kind {
            K::CourseSwitch => {
                let mut items = vec![ListChoice {
                    label: "all（全部）".into(),
                    command: "/course all".into(),
                }];
                items.extend(self.courses.iter().map(|(id, name)| ListChoice {
                    label: name.clone(),
                    // id 定位：课程名含任何字符（空格/尖括号）都不影响
                    command: format!("/course --id {id}"),
                }));
                ("切换课程分区".to_owned(), items)
            }
            K::CourseDelete => (
                "删除课程（其笔记回落 all 区）".to_owned(),
                self.courses
                    .iter()
                    .map(|(id, name)| ListChoice {
                        label: name.clone(),
                        command: format!("/course -delete --id {id}"),
                    })
                    .collect(),
            ),
        };
        if items.is_empty() {
            self.push_entry(Entry::Info("暂无可选项".into()));
            return;
        }
        self.list_picker = Some(ListPicker {
            title,
            items,
            selected: 0,
        });
    }

    /// 列表选择器按键：↑↓ 选择、Enter 提交绑定命令、Esc 关闭。
    pub(crate) fn handle_list_picker_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.list_picker = None;
                self.restore_input_backup();
            }
            KeyCode::Up => {
                if let Some(lp) = &mut self.list_picker {
                    lp.selected = lp.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(lp) = &mut self.list_picker
                    && !lp.items.is_empty()
                {
                    lp.selected = (lp.selected + 1).min(lp.items.len() - 1);
                }
            }
            KeyCode::Enter => {
                let Some(lp) = &self.list_picker else {
                    return;
                };
                let Some(choice) = lp.items.get(lp.selected) else {
                    return;
                };
                let cmd = choice.command.clone();
                self.list_picker = None;
                self.drop_input_backup();
                self.input = cmd;
                self.cursor_pos = self.input.chars().count();
                self.submit();
            }
            _ => {}
        }
    }

    /// 向导导航键（编辑键由普通聊天框路径处理——共享输入缓冲）。
    /// 返回 true 表示该键已被向导消费。
    pub(crate) fn handle_wizard_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                match self.wizard.as_mut().unwrap().back() {
                    // 回退上一步：聊天框恢复该步默认/已填值
                    Some(restored) => {
                        self.input = restored;
                        self.cursor_pos = self.input.chars().count();
                    }
                    None => {
                        self.wizard = None;
                        self.restore_input_backup();
                    }
                }
                true
            }
            KeyCode::Enter => {
                let value = std::mem::take(&mut self.input);
                self.cursor_pos = 0;
                let done = self.wizard.as_mut().unwrap().confirm(value);
                if done {
                    // 数据在 finish_wizard 内读取后才清理
                    self.finish_wizard();
                } else {
                    self.enter_wizard_step();
                }
                true
            }
            _ => false,
        }
    }

    /// 推进到向导下一步：把该步默认值放进聊天框缓冲。
    pub(crate) fn enter_wizard_step(&mut self) {
        if let Some(w) = &self.wizard {
            let default = w.steps[w.current].default.clone();
            self.input = default;
            self.cursor_pos = self.input.chars().count();
        }
    }

    /// 弹窗按键：↑↓/j/k 移动、数字直选、Enter 确认、Esc 关闭。
    pub(crate) fn handle_picker_key(&mut self, key: KeyEvent) {
        let picker = self.model_picker.as_mut().unwrap();
        let len = picker.options.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                picker.selected = picker.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 {
                    picker.selected = (picker.selected + 1).min(len - 1);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.model_picker = None;
            }
            KeyCode::Enter => {
                let name = picker.options.get(picker.selected).map(|p| p.name.clone());
                self.model_picker = None;
                if let Some(name) = name {
                    self.apply_provider_switch(&name);
                }
            }
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as u8 - b'1') as usize;
                if idx < len {
                    let name = picker.options[idx].name.clone();
                    self.model_picker = None;
                    self.apply_provider_switch(&name);
                }
            }
            _ => {}
        }
    }
}
