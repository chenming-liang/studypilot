//! 会话持久化三态机与历史会话管理。

use std::sync::Arc;

use agent_core::Message;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::spawn_blocking;

use super::{App, AppEvent, Entry, SessionState};

/// 从一条 tool 结果消息的 content 提取紧凑摘要（文档四/十六：只存可见元数据，
/// 不存完整 raw result）。search_notes 返回 `{"count":N,"results":[...]}`。
/// 返回 "N 条"；无结构化摘要时返回空串（UI 仅显示 tool 名）。
fn tool_result_summary(content: &str) -> String {
    if content.starts_with("TOOL_ERROR") {
        return "failed".into();
    }
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(v) => {
            if let Some(c) = v.get("count").and_then(|c| c.as_u64()) {
                format!("{c} 条")
            } else if let Some(a) = v.get("results").and_then(|r| r.as_array()) {
                format!("{} 条", a.len())
            } else {
                String::new()
            }
        }
        Err(_) => String::new(),
    }
}

/// 从历史消息重建聊天 entries（文档：Tool Activity 是 Session history 的一部分，
/// 恢复时不重新执行、只显示紧凑记录）。与 live 的 `Entry::Tool` 同构。
/// 返回 (entries, tool_calls 摘要数组：([tool name, summary]) 便于后续 inspector 复用)。
fn history_to_entries(msgs: &[Message]) -> (Vec<Entry>, Vec<(String, String)>) {
    let mut entries = Vec::new();
    let mut tools: Vec<(String, String)> = Vec::new();
    // 记录最近一次 assistant tool_calls 中每个 call_id → tool 名
    let mut call_tool: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for m in msgs {
        match m.role {
            agent_core::Role::System => {
                if let Some(c) = &m.content {
                    entries.push(Entry::Info(c.clone()));
                }
            }
            agent_core::Role::User => {
                if let Some(c) = &m.content {
                    entries.push(Entry::User(c.clone()));
                }
            }
            agent_core::Role::Assistant => {
                if m.tool_calls.is_empty() {
                    entries.push(Entry::Assistant {
                        content: m.content.clone().unwrap_or_default(),
                        reasoning_chars: None,
                    });
                } else {
                    // assistant 携带 tool_calls：登记 name（结果消息只有 tool_call_id）
                    for tc in &m.tool_calls {
                        call_tool.insert(tc.id.clone(), tc.function.name.clone());
                    }
                }
            }
            agent_core::Role::Tool => {
                let name = m
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| call_tool.get(id).cloned())
                    .unwrap_or_else(|| "tool".into());
                let summary = tool_result_summary(m.content.as_deref().unwrap_or(""));
                let ok = !m.content.as_deref().unwrap_or("").starts_with("TOOL_ERROR");
                entries.push(Entry::Tool {
                    text: if summary.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} → {summary}")
                    },
                    ok: Some(ok),
                });
                tools.push((name, summary));
            }
        }
    }
    (entries, tools)
}

impl App {
    /// `/rename <标题>`：重命名当前会话；仅 Ready 状态可改。
    pub(crate) fn rename_session(&mut self, title: &str) {
        if title.is_empty() {
            self.push_entry(Entry::Error("用法: /rename <新标题>".into()));
            return;
        }
        let id = match &self.session_state {
            SessionState::Ready { id, .. } => *id,
            _ => {
                self.push_entry(Entry::Error(
                    "当前会话尚未持久化，发一条消息后再改名".into(),
                ));
                return;
            }
        };
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        let title = title.to_owned();
        tokio::spawn(async move {
            let title2 = title.clone();
            let ok = spawn_blocking(move || store.set_session_title(id, &title2).unwrap_or(false))
                .await
                .unwrap_or(false);
            let _ = tx.send(AppEvent::TitleRenamed(ok, title));
        });
    }

    pub(crate) fn on_title_renamed(&mut self, ok: bool, title: String) {
        if ok {
            self.push_entry(Entry::Info(format!("会话已重命名为: {title}")));
            self.request_sessions_refresh();
        } else {
            self.push_entry(Entry::Error("重命名失败：会话不存在".into()));
        }
    }

    /// `/new`：丢弃当前上下文，开启新会话。
    /// Global 作用域下拒绝（Invariant 4：New Session 默认必须有真实 course_id，
    /// 文档 §四-5 New Conversation 默认 Course 必须是真实 Course）。
    pub(crate) fn start_new_session(&mut self) {
        if self.current_course_id().is_none() {
            self.push_entry(Entry::Error(
                "开新会话需要具体课程：先 /course <课程名> 进入一门课".into(),
            ));
            return;
        }
        self.history.clear();
        self.entries.clear();
        self.session_state = SessionState::None;
        self.session_cost = 0.0;
        self.scroll_up = 0;
        self.push_entry(Entry::Info(
            "已开启新会话（历史仍在侧栏，可用 /open <id> 恢复）".into(),
        ));
        self.request_sessions_refresh();
    }

    /// `/open <id>`：从 DB 加载历史会话恢复上下文（含课程分区，R5）。
    pub(crate) fn open_session(&mut self, id: i64) {
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let loaded = spawn_blocking(
                move || -> storage::Result<(i64, Option<i64>, Vec<Message>)> {
                    let meta: Option<(Option<i64>,)> = store
                        // 通过 list_sessions 找到该会话的 course_id
                        .list_sessions()?
                        .into_iter()
                        .find(|s| s.id == id)
                        .map(|s| (s.course_id,));
                    let course_id = meta.and_then(|(cid,)| cid);
                    let msgs = store.load_session_messages(id)?;
                    Ok((id, course_id, msgs))
                },
            )
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = tx.send(AppEvent::SessionOpened(loaded));
        });
    }

    pub(crate) fn on_session_opened(
        &mut self,
        opened: Result<(i64, Option<i64>, Vec<Message>), String>,
    ) {
        match opened {
            Ok((id, course_id, msgs)) => {
                let count = msgs.len();
                self.history = msgs.clone();
                let (rebuilt, _) = history_to_entries(&msgs);
                self.entries = rebuilt;
                // R5：恢复会话时连课程分区一起还原（文档 §32：per-course 分区）
                if let Some(cid) = course_id {
                    if let Some(name) = self
                        .courses
                        .iter()
                        .find(|(id, _)| *id == cid)
                        .map(|(_, n)| n.clone())
                    {
                        self.swap_course_partition(&name);
                    } else {
                        self.swap_course_partition("all");
                    }
                } else {
                    self.swap_course_partition("all");
                }
                self.enter_ready_session(id, Vec::new());
                self.session_cost = 0.0;
                self.scroll_up = 0;
                self.request_sessions_refresh();
                self.push_entry(Entry::Info(format!(
                    "已恢复会话 #{id}（课程: {}），共 {count} 条消息",
                    self.course
                )));
            }
            Err(e) => self.push_entry(Entry::Error(format!("加载会话失败: {e}"))),
        }
    }

    /// `/export`：当前会话导出为 JSON 文件（含完整 messages，可 /load 还原）。
    pub(crate) fn export_session(&mut self) {
        if self.history.is_empty() {
            self.push_entry(Entry::Error("当前会话为空，无可导出内容".into()));
            return;
        }
        let session_id = match &self.session_state {
            SessionState::Ready { id, .. } => *id,
            _ => 0,
        };
        let payload = serde_json::json!({
            "version": 1,
            "session_id": session_id,
            "course": self.course,
            "provider": self.provider_cfg.name,
            "model": self.provider_cfg.model,
            "messages": self.history,
        });

        let dir = std::path::Path::new("data/exports");
        if let Err(e) = std::fs::create_dir_all(dir) {
            self.push_entry(Entry::Error(format!("创建导出目录失败: {e}")));
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!(
            "session-{}-{ts}.json",
            if session_id == 0 {
                "draft".to_owned()
            } else {
                session_id.to_string()
            }
        ));
        match serde_json::to_string_pretty(&payload)
            .map_err(|e| e.to_string())
            .and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string()))
        {
            Ok(()) => self.push_entry(Entry::Info(format!(
                "已导出 {} 条消息到 {}（/load 该文件可还原）",
                self.history.len(),
                path.display()
            ))),
            Err(e) => self.push_entry(Entry::Error(format!("导出失败: {e}"))),
        }
    }

    /// `/load <文件>`：加载导出的 JSON 为新会话（历史一并持久化入库）。
    pub(crate) fn load_session_file(&mut self, path: &str) {
        let parsed: anyhow::Result<serde_json::Value> = (|| {
            let text = std::fs::read_to_string(path)?;
            serde_json::from_str(&text).map_err(|e| e.into())
        })();
        let value = match parsed {
            Ok(v) => v,
            Err(e) => {
                self.push_entry(Entry::Error(format!("读取 {path} 失败: {e}")));
                return;
            }
        };
        let msgs: Vec<Message> = match serde_json::from_value(value["messages"].clone()) {
            Ok(m) => m,
            Err(e) => {
                self.push_entry(Entry::Error(format!("文件不是合法的会话导出: {e}")));
                return;
            }
        };
        if msgs.is_empty() {
            self.push_entry(Entry::Error("文件中没有消息".into()));
            return;
        }

        let count = msgs.len();
        self.history = msgs.clone();
        let (rebuilt, _) = history_to_entries(&msgs);
        self.entries = rebuilt;

        // 作为新会话整体入库（首条触发建会话，其余缓冲后顺序落库）
        self.session_state = SessionState::None;
        self.scroll_up = 0;
        for m in msgs {
            self.persist(m);
        }
        self.push_entry(Entry::Info(format!(
            "已从 {path} 加载 {count} 条消息为新会话"
        )));
    }

    /// 把消息交给会话状态机落库：无会话则先建，创建中则缓冲，就绪则直发单写泵。
    pub(crate) fn persist(&mut self, msg: Message) {
        match &mut self.session_state {
            SessionState::None => {
                let title: String = msg
                    .content
                    .as_deref()
                    .unwrap_or("(空)")
                    .chars()
                    .take(20)
                    .collect();
                let course_id = self
                    .courses
                    .iter()
                    .find(|(_, n)| *n == self.course)
                    .map(|(id, _)| *id);
                self.session_state = SessionState::Pending {
                    buffered: vec![msg],
                };

                let store = Arc::clone(&self.store);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let opened = spawn_blocking(move || -> storage::Result<i64> {
                        store.create_session(Some(&title), course_id)
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()));
                    match opened {
                        Ok(id) => {
                            let _ = tx.send(AppEvent::SessionReady(id));
                        }
                        Err(e) => {
                            // 不能静默卡死在 Pending：回退状态并告知用户，
                            // 否则后续消息会无限堆积 buffer 且永不落库
                            tracing::error!("创建会话失败: {e}");
                            let _ = tx.send(AppEvent::SessionCreateFailed(e));
                        }
                    }
                });
            }
            SessionState::Pending { buffered } => buffered.push(msg),
            SessionState::Ready { .. } => self.send_to_saver(msg),
        }
    }

    pub(crate) fn send_to_saver(&mut self, msg: Message) {
        if let SessionState::Ready { saver, .. } = &self.session_state {
            let _ = saver.send(msg);
        }
    }

    /// 建会话失败：回退 Pending → None，告知用户哪些消息未持久化。
    /// 不回滚 history（对话体验不受影响），仅放弃落库。
    pub(crate) fn on_session_create_failed(&mut self, err: String) {
        if !matches!(self.session_state, SessionState::Pending { .. }) {
            return;
        }
        let buffered_n = match std::mem::replace(&mut self.session_state, SessionState::None) {
            SessionState::Pending { buffered } => buffered.len(),
            other => {
                self.session_state = other;
                return;
            }
        };
        self.push_entry(Entry::Error(format!(
            "会话创建失败: {err}\n本次的 {buffered_n} 条消息仅保存在内存会话中，\
             不会持久化；可 /new 后重新发送重试"
        )));
    }

    /// 会话就绪：把缓冲消息灌入新建的单写泵。
    pub(crate) fn on_session_ready(&mut self, id: i64) {
        let buffered = match std::mem::replace(&mut self.session_state, SessionState::None) {
            SessionState::Pending { buffered } => buffered,
            other => {
                self.session_state = other;
                return;
            }
        };
        self.enter_ready_session(id, buffered);
        self.request_sessions_refresh();
    }

    /// 单写泵：顺序消费消息逐条 append，保证 seq 有序（D2：DB 走 blocking 池）。
    pub(crate) fn spawn_saver_pump(&mut self, session_id: i64, mut rx: UnboundedReceiver<Message>) {
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let ok = spawn_blocking({
                    let store = Arc::clone(&store);
                    move || store.append_message(session_id, &msg)
                })
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false);
                if !ok {
                    tracing::error!(session_id, "消息落库失败，持久化中止");
                    break;
                }
            }
        });
    }

    pub(crate) fn enter_ready_session(&mut self, id: i64, buffered: Vec<Message>) {
        let (saver_tx, rx) = unbounded_channel::<Message>();
        for m in buffered {
            let _ = saver_tx.send(m);
        }
        self.spawn_saver_pump(id, rx);
        self.session_state = SessionState::Ready {
            id,
            saver: saver_tx,
        };
    }

    /// 侧栏会话列表刷新完成：更新缓存；若是 `/sessions` 显式请求，同时打印到聊天区。
    pub(crate) fn on_sessions_loaded(&mut self, list: Vec<storage::SessionMeta>) {
        self.sidebar_sessions = list;
        if self.pending_sessions_print {
            self.pending_sessions_print = false;
            self.print_session_list();
        }
    }

    /// 会话归属跟随当前分区：切换分区后同步 sessions.course_id 并刷新列表。
    /// Switch（同步分支）与 on_course_managed（异步课程操作）共用。
    pub(crate) fn sync_session_course(&mut self) {
        if let SessionState::Ready { id, .. } = &self.session_state {
            let sid = *id;
            let new_course = self.current_course_id();
            let store = Arc::clone(&self.store);
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let _ = spawn_blocking({
                    let store = Arc::clone(&store);
                    move || store.update_session_course(sid, new_course)
                })
                .await;
                if let Ok(list) =
                    spawn_blocking(move || store.list_sessions().map_err(|e| e.to_string()))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r)
                {
                    let _ = tx.send(AppEvent::SessionsLoaded(list));
                }
            });
        }
    }

    pub(crate) fn print_session_list(&mut self) {
        let lines: Vec<String> = self
            .sidebar_sessions
            .iter()
            .map(|s| {
                let course = self.course_label(s.course_id);
                format!(
                    "  #{} {} · {}",
                    s.id,
                    s.title.as_deref().unwrap_or("(未命名)"),
                    course
                )
            })
            .collect();
        if lines.is_empty() {
            self.push_entry(Entry::Info("尚无历史会话".into()));
            return;
        }
        for l in lines {
            self.push_entry(Entry::Info(l));
        }
    }

    // ---- R6：非 chat 路径的成本对账与 usage 落库 ----

    /// 侧栏会话列表刷新。
    pub(crate) fn request_sessions_refresh(&mut self) {
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let list = spawn_blocking(move || store.list_sessions().map_err(|e| e.to_string()))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            if let Ok(list) = list {
                let _ = tx.send(AppEvent::SessionsLoaded(list));
            }
        });
    }

    // ---- R6：usage 落库 ----
}

impl App {
    /// 会话浏览器：发起异步搜索（搜索词 = 聊天框共享缓冲）。
    pub(crate) fn session_browser_search(&mut self) {
        let Some(b) = &mut self.session_browser else {
            return;
        };
        let seq = b.next_search_seq();
        b.loading = true;
        let query = self.input.clone();
        let scope_course = b.scope_course;
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = spawn_blocking(move || {
                store
                    .search_sessions(&query, scope_course, 200)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = tx.send(AppEvent::SessionBrowserResults { seq, result });
        });
    }

    pub(crate) fn on_session_browser_results(
        &mut self,
        seq: u64,
        result: Result<Vec<storage::SessionMeta>, String>,
    ) {
        let Some(b) = &mut self.session_browser else {
            return;
        };
        if seq != b.search_seq {
            return;
        }
        match result {
            Ok(rows) => b.results = rows,
            Err(e) => {
                b.loading = false;
                b.clamp_cursor();
                let msg = format!("搜索会话失败: {e}");
                self.push_entry(Entry::Error(msg));
                return;
            }
        }
        b.loading = false;
        b.clamp_cursor();
    }

    /// o/Enter：恢复光标会话（关闭浏览器 → 走原有 open_session 流程）。
    pub(crate) fn session_browser_open(&mut self) {
        let Some(b) = &self.session_browser else {
            return;
        };
        let Some(meta) = b.current() else {
            return;
        };
        let id = meta.id;
        self.session_browser = None;
        self.restore_input_backup();
        self.open_session(id);
    }

    /// r：进入重命名子状态（搜索词暂存，输入缓冲腾给新标题）。
    pub(crate) fn session_browser_begin_rename(&mut self) {
        let (id, old_title) = {
            let Some(b) = &self.session_browser else {
                return;
            };
            match b.current() {
                Some(meta) => (meta.id, meta.title.clone().unwrap_or_default()),
                None => return,
            }
        };
        let Some(b) = &mut self.session_browser else {
            return;
        };
        b.rename_id = Some(id);
        b.rename_old = old_title;
        b.mode = crate::session_browser::SessionBrowserMode::Rename;
        self.input = b.rename_old.clone();
        self.cursor_pos = self.input.chars().count();
    }

    /// 重命名确认（Enter）：写库 → 刷新列表 → 返回 Select。
    pub(crate) fn session_browser_confirm_rename(&mut self) {
        let (id, _old_title) = {
            let Some(b) = &self.session_browser else {
                return;
            };
            match b.rename_id {
                Some(id) => (id, b.rename_old.clone()),
                None => return,
            }
        };
        let new_title = self.input.trim().to_owned();
        if new_title.is_empty() {
            return;
        }
        // 乐观更新：立即回 Select 并刷新本地列表（写库失败再报错，下次搜索自愈）
        if let Some(b) = &mut self.session_browser {
            for r in b.results.iter_mut() {
                if r.id == id {
                    r.title = Some(new_title.clone());
                }
            }
            b.mode = crate::session_browser::SessionBrowserMode::Select;
        }
        self.input.clear();
        self.cursor_pos = 0;
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = spawn_blocking({
                let store = Arc::clone(&store);
                move || {
                    store
                        .set_session_title(id, &new_title)
                        .map(|ok| ok && !new_title.is_empty())
                        .map_err(|e| e.to_string())
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            match result {
                Ok(true) => {
                    let _ = tx.send(AppEvent::BrowserActionDone {
                        scope_label: "会话".into(),
                        result: Ok(format!("会话 #{id} 已重命名")),
                    });
                    if let Ok(list) =
                        spawn_blocking(move || store.list_sessions().map_err(|e| e.to_string()))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r)
                    {
                        let _ = tx.send(AppEvent::SessionsLoaded(list));
                    }
                }
                _ => {
                    let _ = tx.send(AppEvent::BrowserActionDone {
                        scope_label: "会话".into(),
                        result: Err(format!(
                            "重命名失败：会话 #{id} 可能不存在，重新搜索可恢复显示"
                        )),
                    });
                }
            }
        });
    }

    /// 重命名任意会话（浏览器 r 动作与向导直连共用）。
    pub(crate) fn rename_session_id(&mut self, id: i64, title: &str) {
        if title.is_empty() {
            return;
        }
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        let title = title.to_owned();
        tokio::spawn(async move {
            let result = spawn_blocking({
                let store = Arc::clone(&store);
                move || {
                    store
                        .set_session_title(id, &title)
                        .map_err(|e| e.to_string())
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            match result {
                Ok(true) => {
                    let _ = tx.send(AppEvent::BrowserActionDone {
                        scope_label: "会话".into(),
                        result: Ok(format!("会话 #{id} 已重命名")),
                    });
                    if let Ok(list) =
                        spawn_blocking(move || store.list_sessions().map_err(|e| e.to_string()))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r)
                    {
                        let _ = tx.send(AppEvent::SessionsLoaded(list));
                    }
                }
                _ => {
                    let _ = tx.send(AppEvent::BrowserActionDone {
                        scope_label: "会话".into(),
                        result: Err("重命名失败：会话不存在".into()),
                    });
                }
            }
        });
    }

    /// d：进入删除确认（当前打开的会话禁止删除）。
    pub(crate) fn session_browser_begin_delete(&mut self) {
        if self.current_session_id()
            == self
                .session_browser
                .as_ref()
                .and_then(|b| b.current().map(|m| m.id))
        {
            self.push_entry(Entry::Error(
                "这是当前打开的会话，请先切换到其他会话再删除".into(),
            ));
            return;
        }
        if let Some(b) = &mut self.session_browser
            && b.current().is_some()
        {
            b.mode = crate::session_browser::SessionBrowserMode::ConfirmDelete;
            self.input.clear();
            self.cursor_pos = 0;
        }
    }

    /// 删除确认（Enter）：级联删聊天记录 → 刷新列表 → 返回 Select。
    pub(crate) fn session_browser_confirm_delete(&mut self) {
        let Some(b) = &self.session_browser else {
            return;
        };
        let Some(meta) = b.current() else {
            return;
        };
        let id = meta.id;
        let title = meta.title.clone().unwrap_or_else(|| "(未命名)".into());
        self.session_browser = None;
        self.restore_input_backup();
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = spawn_blocking({
                let store = Arc::clone(&store);
                move || store.delete_session(id).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            match result {
                Ok(true) => {
                    let _ = tx.send(AppEvent::BrowserActionDone {
                        scope_label: "会话".into(),
                        result: Ok(format!("已删除会话 #{id} {title}（聊天记录一并清除）")),
                    });
                    if let Ok(list) =
                        spawn_blocking(move || store.list_sessions().map_err(|e| e.to_string()))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r)
                    {
                        let _ = tx.send(AppEvent::SessionsLoaded(list));
                    }
                }
                other => {
                    let msg = match other {
                        Ok(false) => format!("会话 #{id} 不存在"),
                        Err(e) => format!("删除失败: {e}"),
                        Ok(true) => unreachable!(),
                    };
                    let _ = tx.send(AppEvent::BrowserActionDone {
                        scope_label: "会话".into(),
                        result: Err(msg),
                    });
                }
            }
        });
    }
}

impl App {
    /// 向导完成：按 kind 分发到结构化内部调用（不再合成命令字符串走文本解析）。
    pub(crate) fn finish_wizard(&mut self) {
        use crate::wizard::WizardKind;
        let Some(w) = &self.wizard else { return };
        let values = w.values();
        let kind = w.kind().clone();
        let course_name = w.course_name().to_owned();
        // 数据克隆完毕后才清向导（调用方不得提前置 None）
        self.wizard = None;
        self.drop_input_backup();
        self.input.clear();
        self.cursor_pos = 0;

        match kind {
            WizardKind::Review { course_id, .. } => {
                let concept = values
                    .first()
                    .map(|v| v.trim().to_owned())
                    .unwrap_or_default();
                let n = values
                    .get(1)
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .filter(|n| *n > 0)
                    .unwrap_or(5);
                let scope = if concept.is_empty() {
                    None
                } else {
                    Some(concept.trim().to_owned())
                };
                // all（course_id=None）= 全部笔记出题，同样合法
                self.run_review(course_id, course_name, scope, n);
            }
            WizardKind::ReviewCount {
                course_id,
                course_name,
                scope,
            } => {
                // 正式复习题数 = 用户指定（默认 5）；warm-up 卡数由 LLM 自主 3~8
                let n = values
                    .first()
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .filter(|n| *n > 0)
                    .unwrap_or(5);
                self.start_warmup(scope, course_id, course_name, n);
            }
            WizardKind::Import => {
                let dir = values
                    .first()
                    .map(|s| s.trim().trim_matches('"').to_owned());
                let course = values
                    .get(1)
                    .map(|s| s.trim().to_owned())
                    .filter(|c| !c.is_empty() && c != "all");
                match dir {
                    Some(d) if !d.is_empty() => {
                        // 统一路径解析（绝对/相对任意位置），失败给用户可读错误
                        match crate::import_path::resolve_import_path(&d) {
                            Ok(target) => self.run_import(target, course),
                            Err(e) => self.push_entry(Entry::Error(e.to_string())),
                        }
                    }
                    _ => self.push_entry(Entry::Error("用法: /import <路径>".into())),
                }
            }
            WizardKind::RenameSession => {
                let Some(id) = self.current_session_id() else {
                    self.push_entry(Entry::Error("当前没有已持久化的聊天会话".into()));
                    return;
                };
                let title = values
                    .first()
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_default();
                if !title.is_empty() {
                    self.rename_session_id(id, &title);
                }
            }
            WizardKind::RenameCourse => {
                let name = values
                    .first()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_default();
                if !name.is_empty() {
                    self.rename_course(&name);
                }
            }
            WizardKind::LoadFile => {
                if let Some(path) = values
                    .first()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                {
                    self.load_session_file(&path);
                }
            }
            WizardKind::CreateCourse => {
                if let Some(name) = values
                    .first()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                {
                    self.create_course_flow(name);
                } else {
                    // 空名不发起流程：清掉 Home 创建标志，避免残留
                    self.pending_course_enter = false;
                }
            }
            WizardKind::BudgetLimit => {
                if let Some(v) = values
                    .first()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .filter(|v| *v > 0.0)
                {
                    self.max_cost = v;
                    self.push_entry(Entry::Info(format!("预算上限已设为 ${v:.2}")));
                } else {
                    self.push_entry(Entry::Error("用法: /budget <金额>（金额须为正数）".into()));
                }
            }
        }
    }
}

#[cfg(test)]
mod finish_wizard_tests {
    use super::super::commands::course_delete_tests::test_app;
    use super::*;

    /// 回归：finish_wizard 必须在向导数据读取之后才清理——
    /// 此前调用方先置 None 导致所有向导动作静默失效。
    #[tokio::test]
    async fn finish_dispatches_review_with_data() {
        let mut app = test_app();
        app.course = "rust".into();
        app.wizard = Some(crate::wizard::Wizard::new_review(Some(1), "rust".into()));
        // 模拟两步确认
        let w = app.wizard.as_mut().unwrap();
        assert!(!w.confirm("ownership".into()));
        assert!(w.confirm("3".into()));
        // 调用方不清 wizard，直接 finish（修复后的顺序）
        app.finish_wizard();

        assert!(app.wizard.is_none(), "向导应被清理");
        assert!(
            app.entries.iter().any(
                |e| matches!(e, Entry::Info(t) if t.contains("开始出题") && t.contains("ownership"))
            ),
            "应执行出题: {:?}",
            app.entries
        );
    }

    #[test]
    fn finish_without_wizard_is_noop() {
        let mut app = test_app();
        app.finish_wizard();
        assert!(app.wizard.is_none());
    }
}

/// 文档（Agent项目改进建议.md）：Tool Activity 是 Session history 的一部分，
/// 恢复时重建紧凑记录，绝不重新执行。
#[cfg(test)]
mod tool_activity_tests {
    use super::*;
    use agent_core::ToolCall;

    /// 构造一轮带工具调用的历史：user → assistant(tool_calls) → tool result → assistant 回答。
    fn turn_with_tool() -> Vec<Message> {
        vec![
            Message::user("为什么 Iterator 是惰性的？"),
            Message::assistant_tool_calls(vec![
                ToolCall::function("call_1", "search_notes", r#"{"query":"iterator"}"#),
                ToolCall::function("call_2", "get_document_text", r#"{"note_id":1}"#),
            ]),
            Message::tool_result("call_1", r#"{"results":[1,2,3],"count":3}"#),
            Message::tool_result("call_2", "page 4"),
            Message::assistant("Iterator 是惰性的，因为……"),
        ]
    }

    fn tool_entries(entries: &[Entry]) -> Vec<&Entry> {
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Tool { .. }))
            .collect()
    }

    #[test]
    fn tool_activity_persists_across_session_reload() {
        let msgs = turn_with_tool();
        let (entries, tools) = history_to_entries(&msgs);
        let tools2 = tool_entries(&entries);
        assert_eq!(tools.len(), 2, "恢复后应保留 tool activities");
        assert_eq!(tools2.len(), 2);
        // 不是 "[工具结果]" 占位符
        assert!(
            !entries
                .iter()
                .any(|e| matches!(e, Entry::Info(t) if t.contains("[工具结果]"))),
            "不得再渲染 [工具结果] 占位符"
        );
    }

    #[test]
    fn tool_name_visible_after_restore() {
        let (entries, _) = history_to_entries(&turn_with_tool());
        let tools = tool_entries(&entries);
        let Entry::Tool { text, ok } = tools[0] else {
            unreachable!()
        };
        assert!(text.contains("search_notes"), "tool 名必须可见: {text}");
        assert_eq!(*ok, Some(true), "成功状态持久化");
        let Entry::Tool { text: t2, .. } = tools[1] else {
            unreachable!()
        };
        assert!(t2.contains("get_document_text"), "第二个 tool 名可见: {t2}");
    }

    #[test]
    fn tool_summary_persisted() {
        let (entries, _) = history_to_entries(&turn_with_tool());
        let tools = tool_entries(&entries);
        let Entry::Tool { text, .. } = tools[0] else {
            unreachable!()
        };
        assert!(
            text.contains("→ 3 条"),
            "search_notes 应重建命中数摘要: {text}"
        );
    }

    #[test]
    fn tool_failure_persisted() {
        let msgs = vec![
            Message::assistant_tool_calls(vec![ToolCall::function("c1", "search_notes", "{}")]),
            Message::tool_result("c1", "TOOL_ERROR [api]: 网络错误"),
        ];
        let (entries, _) = history_to_entries(&msgs);
        let Entry::Tool { text, ok } = tool_entries(&entries)[0] else {
            unreachable!()
        };
        assert!(text.contains("search_notes"), "失败也保留 tool 名");
        assert_eq!(*ok, Some(false), "失败状态持久化");
    }

    #[test]
    fn tool_activity_does_not_reexecute_on_restore() {
        // 重建是纯数据变换：不触发任何工具执行（无 provider/无 store 依赖即可断言）
        let (entries, _) = history_to_entries(&turn_with_tool());
        assert_eq!(tool_entries(&entries).len(), 2);
    }

    #[test]
    fn tool_activity_is_not_cot() {
        // 只重建真实 tool call，不渲染 hidden reasoning / CoT
        let (entries, _) = history_to_entries(&turn_with_tool());
        assert!(
            !entries
                .iter()
                .any(|e| matches!(e, Entry::Assistant { content, .. } if content.contains("推理"))),
            "不得把 CoT 当历史"
        );
    }

    #[test]
    fn export_load_roundtrip_preserves_tool_activity() {
        // /export 序列化 history（含 tool 消息），/load 后 history_to_entries 重建 tool activity
        let msgs = turn_with_tool();
        let json = serde_json::to_string(&msgs).unwrap();
        let restored: Vec<Message> = serde_json::from_str(&json).unwrap();
        let (entries, _) = history_to_entries(&restored);
        assert_eq!(
            tool_entries(&entries).len(),
            2,
            "导出/导入往返后 tool activity 不丢"
        );
    }
}
