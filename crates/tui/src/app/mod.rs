//! TUI 应用状态、事件分发与主循环。

use std::sync::Arc;

use agent_core::{LoopEvent, Message, Role, Usage};
use agent_providers::{OpenAiClient, ProviderConfig};
use crossterm::event::Event as CtEvent;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use crate::review;

use crate::ui;
mod browser_flow;
pub(crate) mod cards;
mod chat;
mod commands;
mod import_flow;
mod keys;
mod outline_flow;
mod review_flow;
mod sessions;
pub(crate) mod setup;

pub use crate::events::{AgentEvent, AppEvent, CourseOpOutcome, Entry};
pub use crate::palette::{CommandPalette, ListPicker, ModelPicker};
pub use crate::wizard::Wizard;

/// 上次所在课程持久化（仿 data/budget.json 模式，无 schema 变更）。
pub(crate) const LAST_COURSE_PATH: &str = "data/last_course.json";

// 测试可覆盖持久化路径（并行单测隔离 data/last_course.json，防写竞争）。
// 单个共享 thread_local：setter 与 resolver 必须引用同一 static（写在不同函数体
// 里会各生成一份，override 永不生效）。
#[cfg(test)]
thread_local! {
    static LAST_COURSE_OVERRIDE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// 测试可覆盖持久化路径（并行单测隔离 data/last_course.json，防写竞争）。
#[cfg(test)]
pub(crate) fn last_course_path_override(path: Option<String>) {
    LAST_COURSE_OVERRIDE.with(|c| *c.borrow_mut() = path);
}

/// 解析持久化路径：测试覆盖优先，否则默认 data/last_course.json。
pub(crate) fn resolve_last_course_path() -> std::path::PathBuf {
    #[cfg(test)]
    {
        let p = LAST_COURSE_OVERRIDE.with(|c| c.borrow().clone());
        if let Some(p) = p {
            return std::path::PathBuf::from(p);
        }
    }
    std::path::PathBuf::from(LAST_COURSE_PATH)
}

/// 顶层信息架构：Home（Launchpad）→ Course（课程上下文）→ Session（聊天工作区）。
/// 三个 workspace 是 UI state，不写入 session history。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    /// Launchpad：Continue learning / Your courses / + New Course。无聊天、无输入框。
    Home,
    /// 课程上下文：统计 / Continue / Knowledge / Materials / Recent learning。
    Course,
    /// 真正的干活工作区：聊天 / Review / Outline / Import 输出。
    Session,
}

use storage::Store;

/// 会话持久化状态机：首条消息触发建会话，就绪后经单写泵顺序落库。
enum SessionState {
    /// 尚无会话（首条消息时创建）
    None,
    /// 创建中：暂存待落库消息
    Pending { buffered: Vec<Message> },
    /// 就绪：saver 通道把消息喂给单写泵任务
    Ready {
        id: i64,
        saver: UnboundedSender<Message>,
    },
}

/// per-course 分区（文档 §32 Context isolation）：每个课程独立持有聊天瞬态——
/// entries / history / session 状态机 / 会话花费 / 滚动位置。
/// `App` 上的同名字段是"当前活跃分区"；课程切换时经 `swap_course_partition` 换入换出。
struct Partition {
    history: Vec<Message>,
    entries: Vec<Entry>,
    session_state: SessionState,
    session_cost: f64,
    scroll_up: u16,
}

impl Default for Partition {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            entries: Vec::new(),
            session_state: SessionState::None,
            session_cost: 0.0,
            scroll_up: 0,
        }
    }
}

pub struct App {
    provider: Arc<OpenAiClient>,
    store: Arc<Store>,
    pub provider_cfg: ProviderConfig,
    /// 全部 provider 配置（/model 切换的候选）
    pub all_providers: Vec<ProviderConfig>,
    /// 运行时配置文件路径（main 解析；/model 落盘用）
    pub config_file: std::path::PathBuf,
    pub courses: Vec<(i64, String)>,
    /// 侧栏课程统计 {course_id: (笔记数, 概念数)}
    pub sidebar_course_stats: std::collections::HashMap<i64, (usize, usize)>,

    history: Vec<Message>,
    pub entries: Vec<Entry>,

    session_state: SessionState,
    /// 侧栏会话列表（启动与变更后刷新）
    pub sidebar_sessions: Vec<storage::SessionMeta>,

    pub input: String,
    /// 输入框光标位置（字符索引，非字节索引）
    pub cursor_pos: usize,
    /// 覆盖层（面板/向导/选择器）打开前的聊天框输入备份；关闭时恢复
    input_backup: Option<String>,
    /// 当前课程分区（M4 仅展示与检索范围占位，/course 切换属后续里程碑）
    pub course: String,
    /// 当前顶层 workspace（Home/Course/Session）
    pub workspace: Workspace,
    /// Home 视图光标（课程列表项索引）
    pub home_cursor: usize,
    /// Course 视图光标（动作项索引）
    pub course_cursor: usize,
    /// Course 视图需巩固概念数缓存 {course_id: weak}（进入 Course 时异步刷新）
    pub weak_stats: std::collections::HashMap<i64, usize>,
    /// Home 发起创建课程：向导完成且创建成功 → 直达新课程 Course workspace。
    /// （Session 内 /course -new 不置位，避免把聊天中的用户拽走）
    pub pending_course_enter: bool,
    /// Home `d` 键删除课程的确认臂：true = 已提示，再按 `d` 真正删除。
    pub home_delete_armed: bool,
    /// Home `d` 键确认臂对应的目标课程名（防止光标移动后误删他课）。
    pub home_delete_target: Option<String>,
    /// per-course 分区：非活跃课程的聊天瞬态缓存（活跃的在 App 顶层字段上）
    partitions: std::collections::HashMap<String, Partition>,

    pub total_usage: Usage,
    pub total_cost: f64,
    /// 本次会话（自启动或 /new /open 起）的累计花费
    pub session_cost: f64,
    /// 预算熔断阈值（config.toml 的 max_cost，M5 接熔断逻辑）
    pub max_cost: f64,

    inflight: Option<CancellationToken>,
    /// 导入任务取消令牌
    import_cancel: Option<CancellationToken>,
    /// 复习逐题生成的取消令牌（出题中等待态 Esc 退出时取消）
    review_gen: Option<CancellationToken>,
    /// 复习模式状态；Some 时按键路由给复习逻辑
    pub review: Option<review::ReviewState>,
    /// 最近一次生成的复习地图（选择器重开数据源）
    pub review_map: Option<crate::outline_render::ReviewMap>,
    /// 简答题批改进行中（防并发提交错位）
    review_grading: bool,
    /// 复习追问回答生成中（防并发；Esc 可中断，留在反馈停留态）
    pub followup_pending: Option<CancellationToken>,
    /// 鼠标拖选状态：(起始行, 结束行) 在渲染行列表中的索引
    pub text_selection: Option<(usize, usize)>,
    /// 拖选锚点（左键按下时的行；Drag 期间不随方向漂移）
    selection_anchor: Option<usize>,
    /// 每帧渲染的聊天文本行（用于选中复制提取原文）
    pub chat_lines: Vec<String>,
    /// 聊天区屏幕坐标（draw 时更新，handle_mouse 时读取）
    pub chat_rect: ratatui::layout::Rect,
    /// spinner 动画帧计数器（inflight 时每 100ms +1）
    pub tick: usize,
    /// 距底部的滚动偏移（0 = 跟随最新）
    pub scroll_up: u16,
    /// /model 弹窗状态；Some 时按键路由给弹窗
    pub model_picker: Option<ModelPicker>,
    /// 命令面板（Ctrl+K）；Some 时按键路由给面板
    pub palette: Option<CommandPalette>,
    /// 参数向导；Some 时按键路由给向导
    pub wizard: Option<Wizard>,
    /// 列表选择器；Some 时按键路由给选择器
    pub list_picker: Option<ListPicker>,
    /// 复习地图（Learning Map）树形选择器；Some 时按键路由给它
    pub review_map_picker: Option<crate::palette::ReviewMapPicker>,
    /// Flashcard Warm-up（正式 Review 前置）；Some 时按键路由给暖场
    pub warmup: Option<review::WarmupState>,
    /// First-run AI Setup Wizard；Some 时进入 Setup 覆盖层
    pub setup: Option<crate::app::setup::SetupState>,
    /// 笔记浏览器（Search→Select→Act）；Some 时按键路由给浏览器
    pub note_browser: Option<crate::note_browser::NoteBrowser>,
    /// 会话浏览器；Some 时按键路由给会话浏览器
    pub session_browser: Option<crate::session_browser::SessionBrowser>,
    /// 右上角临时通知（复制反馈等），到时自动消失
    pub toast: Option<Toast>,
    /// `/sessions` 显式请求后的刷新回调时要打印列表到聊天区（侧栏静默刷新不打印）
    pending_sessions_print: bool,
    should_quit: bool,

    tx: UnboundedSender<AppEvent>,
    rx: UnboundedReceiver<AppEvent>,
}

/// 从文本提取引用编号：`[数字]`（去重升序；[0] 不算——资料编号从 1 起）。
pub(crate) fn extract_ref_numbers(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1
                && j < bytes.len()
                && bytes[j] == b']'
                && let Ok(n) = text[i + 1..j].parse::<usize>()
                && n > 0
            {
                out.push(n);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// 从 agent loop 的 tool 消息中提取 search_notes 结果的 (编号 → 来源) 映射，
/// 返回回答中**实际被引用**的编号及来源（"标题 · 小节"）。
/// 多次搜索时同编号以最后出现为准（模型按就近的 tool 结果理解 [n]）。
pub(crate) fn extract_citations(new_messages: &[Message], answer: &str) -> Vec<(usize, String)> {
    let mut map: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for m in new_messages {
        if m.role != Role::Tool {
            continue;
        }
        let Some(c) = &m.content else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(c) else {
            continue;
        };
        let Some(results) = v.get("results").and_then(|r| r.as_array()) else {
            continue;
        };
        for r in results {
            let id = r.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            if id == 0 {
                continue;
            }
            let title = r.get("title").and_then(|x| x.as_str()).unwrap_or("");
            let section = r.get("section").and_then(|x| x.as_str()).unwrap_or("");
            map.insert(id, format!("{title} · {section}"));
        }
    }
    let mut cited = extract_ref_numbers(answer);
    cited.retain(|n| map.contains_key(n));
    cited.into_iter().map(|n| (n, map[&n].clone())).collect()
}

/// 命令参数规范化：剥掉照抄文档产生的 `<...>` 占位符 token（约定：占位符
/// 在任何命令参数里都无合法语义）。仅作用于斜杠命令参数，普通聊天文本不经过此路径。
pub(crate) fn strip_placeholder_tokens(arg: &str) -> String {
    arg.split_whitespace()
        .filter(|t| !(t.starts_with('<') && t.ends_with('>')))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 右上角临时通知。
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    /// 错误类通知用红色并停留更久
    pub error: bool,
    pub expires_at: std::time::Instant,
}

impl Toast {
    const DURATION: std::time::Duration = std::time::Duration::from_millis(2500);
    const ERROR_DURATION: std::time::Duration = std::time::Duration::from_millis(4000);

    fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            error: false,
            expires_at: std::time::Instant::now() + Self::DURATION,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            error: true,
            expires_at: std::time::Instant::now() + Self::ERROR_DURATION,
        }
    }

    fn expired(&self) -> bool {
        std::time::Instant::now() >= self.expires_at
    }
}

impl App {
    /// 课程列表+统计刷新（D2）：导入流水线可能在 DB 里新建课程，
    /// 内存列表与侧栏统计需要与库对齐。
    pub(crate) fn request_courses_refresh(&self) {
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = spawn_blocking(
                move || -> Result<Vec<(i64, String, usize, usize)>, String> {
                    let courses = store.list_courses().map_err(|e| e.to_string())?;
                    courses
                        .into_iter()
                        .map(|(id, name)| {
                            let (n, c) = store.course_stats(id).unwrap_or((0, 0));
                            Ok((id, name, n, c))
                        })
                        .collect()
                },
            )
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            if let Ok(rows) = result {
                let list = rows.iter().map(|(id, n, _, _)| (*id, n.clone())).collect();
                let stats = rows.into_iter().map(|(id, _, a, b)| (id, (a, b))).collect();
                let _ = tx.send(AppEvent::CoursesRefreshed(list, stats));
            }
        });
    }

    /// agent 过程活动行：◌ 开始 / ✓ 完成原地替换 / ✗ 失败。
    pub(crate) fn on_agent_activity(&mut self, ev: LoopEvent) {
        match ev {
            LoopEvent::RoundStart { round } => {
                if round > 1 {
                    self.push_entry(Entry::Info(format!("◌ 第 {round} 轮思考…")));
                }
            }
            LoopEvent::ToolStart { tool, detail, .. } => {
                self.entries.push(Entry::Tool {
                    text: format!("{tool} {detail}"),
                    ok: None,
                });
                self.scroll_up = 0;
            }
            LoopEvent::ToolDone {
                tool, ok, detail, ..
            } => {
                // 原地替换最后的 Running 行（工具串行执行，最后一行必是其 Start 行）
                if let Some(Entry::Tool { text, ok: slot }) = self.entries.last_mut()
                    && slot.is_none()
                    && text.starts_with(&tool)
                {
                    *text = format!("{tool}{detail}");
                    *slot = Some(ok);
                    return;
                }
                self.entries.push(Entry::Tool {
                    text: format!("{tool}{detail}"),
                    ok: Some(ok),
                });
                self.scroll_up = 0;
            }
        }
    }

    /// 弹出右上角临时通知（Tick 时自动过期清除）。
    pub(crate) fn set_toast(&mut self, message: impl Into<String>, error: bool) {
        self.toast = Some(if error {
            Toast::error(message)
        } else {
            Toast::info(message)
        });
    }

    /// 清除已过期的 toast（主循环 Tick 调用）。
    pub(crate) fn expire_toast(&mut self) {
        if self.toast.as_ref().is_some_and(Toast::expired) {
            self.toast = None;
        }
    }

    /// 覆盖层接管聊天框输入：备份原内容并清空（fzf 风格——覆盖层与聊天框共享同一缓冲）。
    pub(crate) fn take_input_for_overlay(&mut self) {
        // 嵌套守卫：备份仅首次建立。浏览器的 Select→Search 二次进入
        // 不得覆盖外层备份，否则关闭时恢复的是搜索词、原聊天内容丢失。
        if self.input_backup.is_none() {
            self.input_backup = Some(std::mem::take(&mut self.input));
        }
        self.cursor_pos = 0;
    }

    /// 覆盖层取消：恢复备份的聊天框内容。无备份时不动。
    pub(crate) fn restore_input_backup(&mut self) {
        if let Some(prev) = self.input_backup.take() {
            self.input = prev;
            self.cursor_pos = self.input.chars().count();
        }
    }

    /// 覆盖层完成（提交执行）：丢弃备份。
    pub(crate) fn drop_input_backup(&mut self) {
        self.input_backup = None;
    }

    pub(crate) fn new(
        provider: Arc<OpenAiClient>,
        store: Arc<Store>,
        provider_cfg: ProviderConfig,
        all_providers: Vec<ProviderConfig>,
        max_cost: f64,
        courses: Vec<(i64, String)>,
    ) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            provider,
            store,
            provider_cfg,
            all_providers,
            config_file: std::path::PathBuf::from("config.toml"),
            max_cost,
            courses,
            sidebar_course_stats: std::collections::HashMap::new(),
            history: Vec::new(),
            entries: Vec::new(),
            session_state: SessionState::None,
            sidebar_sessions: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            input_backup: None,
            course: "rust".into(),
            workspace: Workspace::Home,
            home_cursor: 0,
            course_cursor: 0,
            weak_stats: std::collections::HashMap::new(),
            pending_course_enter: false,
            home_delete_armed: false,
            home_delete_target: None,
            partitions: std::collections::HashMap::new(),
            total_usage: Usage::default(),
            total_cost: 0.0,
            session_cost: 0.0,
            inflight: None,
            review_gen: None,
            review_map: None,
            import_cancel: None,
            review: None,
            review_grading: false,
            followup_pending: None,
            text_selection: None,
            selection_anchor: None,
            chat_lines: Vec::new(),
            chat_rect: ratatui::layout::Rect::default(),
            tick: 0,
            scroll_up: 0,
            model_picker: None,
            palette: None,
            wizard: None,
            list_picker: None,
            review_map_picker: None,
            warmup: None,
            setup: None,
            note_browser: None,
            session_browser: None,
            toast: None,
            pending_sessions_print: false,
            should_quit: false,
            tx,
            rx,
        }
    }

    pub(crate) fn tx(&self) -> UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub(crate) fn is_inflight(&self) -> bool {
        self.inflight.is_some()
    }

    /// 简答题批改进行中（供 UI 渲染"批改中…"提示）。
    pub(crate) fn is_review_grading(&self) -> bool {
        self.review_grading
    }

    /// 是否需要 AI 配置（首次运行 / 配置缺失时 Home 显示引导，文档 §九）。
    /// 判定：当前 provider 无有效 api_key（明文或 env 均无）即视为未配置。
    pub(crate) fn ai_needs_setup(&self) -> bool {
        if self
            .provider_cfg
            .api_key
            .as_deref()
            .map(|k| !k.is_empty())
            .unwrap_or(false)
        {
            return false;
        }
        !self
            .provider_cfg
            .api_key_env
            .as_deref()
            .is_some_and(|env| std::env::var(env).ok().filter(|v| !v.is_empty()).is_some())
    }

    /// header 状态标签：与按键路由同源的覆盖层状态推导（复习 > 导入 > 请求中 > 选择 > 就绪）。
    /// 让用户随时知道"我现在在哪"，替代隐含状态机。
    pub(crate) fn status_label(&self) -> (String, ratatui::style::Color) {
        use crate::theme::{StatusKind, status_color};
        if let Some(rs) = &self.review {
            let grading = if self.review_grading {
                " · Grading…"
            } else {
                ""
            };
            let generating = if !self.review_grading && rs.questions.get(rs.current).is_none() {
                " · Generating…"
            } else {
                ""
            };
            let cur = (rs.current + 1).min(rs.planned);
            return (
                format!("◌ Review {cur}/{}{grading}{generating}", rs.planned),
                status_color(StatusKind::Processing),
            );
        }
        if self.import_cancel.is_some() {
            // 顶部状态栏只留简洁语义色（进度条已移到对话栏，问题 1 对齐）
            return ("◌ 导入中…".into(), status_color(StatusKind::Processing));
        }
        if self.is_inflight() {
            return ("◌ Thinking…".into(), status_color(StatusKind::Processing));
        }
        ("● Ready".into(), status_color(StatusKind::Success))
    }

    pub(crate) fn push_entry(&mut self, e: Entry) {
        self.entries.push(e);
        // 新内容到达 → 回到底部跟随
        self.scroll_up = 0;
    }

    /// 启动时的上下文恢复：若当前课程不在地图（默认 "rust" 失效）落到第一门课。
    /// Home workspace 本身按 course 数渲染 Welcome / Continue / 课程列表——
    /// 首屏不再往聊天流 push 卡片（导航状态不伪装成聊天消息）。
    pub(crate) fn push_startup_cards(&mut self) {
        if self.courses.is_empty() {
            return;
        }
        if (self.course == "all" || !self.courses.iter().any(|(_, n)| *n == self.course))
            && let Some((_, first)) = self.courses.first()
        {
            self.course = first.clone();
        }
        // 启动恢复后 Home 光标要指向当前课程（否则首帧 Enter 进旧课/错课）
        self.sync_home_cursor_to_course();
    }

    /// 启动时恢复上次所在课程（data/last_course.json，仿 budget.json）。
    /// 课程已不存在则忽略，保留默认/后续兜底。
    pub(crate) fn restore_last_course(&mut self) {
        let Ok(raw) = std::fs::read_to_string(resolve_last_course_path()) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return;
        };
        let Some(name) = v.get("course").and_then(serde_json::Value::as_str) else {
            return;
        };
        if self.courses.iter().any(|(_, n)| n == name) {
            self.course = name.to_owned();
        }
    }

    /// Ctrl+C：导入中→中断导入；请求中→中断请求；空闲→退出（保持既有语义）。
    pub(crate) fn interrupt_or_quit(&mut self) {
        if let Some(token) = &self.import_cancel {
            token.cancel();
            self.push_entry(Entry::Info("中断导入…".into()));
        } else if let Some(token) = &self.inflight {
            token.cancel();
            self.push_entry(Entry::Info("中断请求…".into()));
        } else {
            self.should_quit = true;
        }
    }

    /// Esc：导入中/请求中→中断；否则按 workspace 回退（Session→Course→Home，Home 才退出）。
    pub(crate) fn esc_or_back(&mut self) {
        if let Some(token) = &self.import_cancel {
            token.cancel();
            self.push_entry(Entry::Info("中断导入…".into()));
        } else if let Some(token) = &self.inflight {
            token.cancel();
            self.push_entry(Entry::Info("中断请求…".into()));
        } else if self.workspace == Workspace::Session || self.workspace == Workspace::Course {
            self.go_back();
        } else {
            self.should_quit = true;
        }
    }

    pub(crate) fn quit_now(&mut self) {
        if let Some(token) = &self.import_cancel {
            token.cancel();
        }
        if let Some(token) = &self.inflight {
            token.cancel();
        }
        self.should_quit = true;
    }

    // ── Home / Course / Session 顶层导航 ──

    /// Home 视图可选条数：AI 接入入口（恒占位 0）+ Continue（有可继续 session 时）+ 课程数 + [+ New Course]。
    pub(crate) fn home_cursor_count(&self) -> usize {
        let setup = 1; // AI 入口恒在 Home 首项（未配置=Set up AI / 已配置=AI Models）
        if self.courses.is_empty() {
            return setup + 1; // [+ New Course]（Welcome 态）
        }
        let courses = self.courses.len();
        let items = if self.continue_session_id().is_some() {
            courses + 1
        } else {
            courses
        };
        setup + items + 1 // [+ New Course] 恒在末尾
    }

    /// Course 视图可选动作条数：Continue（本课程有 session 时）+ New conversation/Review Map/
    /// Outline/Import + 最近学习 session 数。
    pub(crate) fn course_cursor_count(&self) -> usize {
        let course_id = self.current_course_id();
        let has_continue = self
            .sidebar_sessions
            .iter()
            .any(|s| s.course_id == course_id);
        let base = if has_continue { 5 } else { 4 };
        let recent = self
            .sidebar_sessions
            .iter()
            .filter(|s| s.course_id == course_id)
            .take(3)
            .count();
        base + recent
    }

    /// 可继续的 session：优先最近的真实课程 session（文档 §五：Global session 不覆盖
    /// 真实课程 Continue 目标）；仅当没有真实课程 session 时才回退到最近的 Global session。
    /// 返回 (session_id, course_name, title)。
    pub(crate) fn continue_session(&self) -> Option<(i64, String, String)> {
        let s = self
            .sidebar_sessions
            .iter()
            .find(|s| s.course_id.is_some())
            .or_else(|| self.sidebar_sessions.first())?;
        let course = self.course_label(s.course_id).to_string();
        let title = s.title.as_deref().unwrap_or("(未命名)").to_string();
        Some((s.id, course, title))
    }

    /// Continue 目标 session_id（Home/Course 是否有 Continue 行的依据）。
    pub(crate) fn continue_session_id(&self) -> Option<i64> {
        self.continue_session().map(|(id, _, _)| id)
    }

    /// per-course 分区切换（文档 §32 Context isolation）：把当前活跃分区的瞬态状态
    /// 存回旧课程的槽位，再从新课程槽位载入（无槽位 = 全新空分区）。
    /// 由课程上下文变更点调用（switch_course / enter_course_workspace / on_course_managed /
    /// on_session_opened）。活跃分区字段保留在 App 顶层，其余代码零改动。
    pub(crate) fn swap_course_partition(&mut self, new_course: &str) {
        let old = std::mem::take(&mut self.course);
        let saved = Partition {
            history: std::mem::take(&mut self.history),
            entries: std::mem::take(&mut self.entries),
            session_state: std::mem::replace(&mut self.session_state, SessionState::None),
            session_cost: std::mem::take(&mut self.session_cost),
            scroll_up: std::mem::take(&mut self.scroll_up),
        };
        self.partitions.insert(old, saved);
        let p = self.partitions.remove(new_course).unwrap_or_default();
        self.history = p.history;
        self.entries = p.entries;
        self.session_state = p.session_state;
        self.session_cost = p.session_cost;
        self.scroll_up = p.scroll_up;
        self.course = new_course.to_owned();
    }

    /// 进入 Course workspace：设置课程上下文 + 进入 Course 视图 + 异步刷新 weak 数。
    pub(crate) fn enter_course_workspace(&mut self, course: &str) {
        if course != self.course {
            self.swap_course_partition(course);
        }
        self.workspace = Workspace::Course;
        self.course_cursor = 0;
        self.spawn_weak_stats(self.current_course_id());
        self.persist_last_course_inner();
    }

    /// 进入 Session workspace：恢复聊天工作区（内容保持不变）。
    pub(crate) fn enter_session_workspace(&mut self) {
        self.workspace = Workspace::Session;
    }

    /// 让 Home 光标跟随权威的当前课程（单一事实源：`app.course` → `home_cursor` 派生）。
    /// Home 列表布局 = [AI 入口] + [Continue?] + courses + [+ New Course]；课程从索引 1 起
    /// （Continue 存在时从 2 起）。
    /// 课程创建/切换/删除/回退到 Home 后都必须调用，否则 Enter 会进旧课程（state 脱节）。
    pub(crate) fn sync_home_cursor_to_course(&mut self) {
        if self.courses.is_empty() {
            self.home_cursor = 0;
            return;
        }
        let idx = self.courses.iter().position(|(_, n)| *n == self.course);
        let offset = 1 + if self.continue_session_id().is_some() {
            1
        } else {
            0
        };
        self.home_cursor = match idx {
            Some(i) => offset + i,
            None => 0,
        };
    }

    /// 返回上一级：Session→Course→Home。
    pub(crate) fn go_back(&mut self) {
        match self.workspace {
            Workspace::Session => self.enter_course_workspace(&self.course.clone()),
            Workspace::Course => {
                self.workspace = Workspace::Home;
                // 回 Home 后光标要指向当前课程，而不是回到 0（否则 Enter 又进旧课）
                self.sync_home_cursor_to_course();
            }
            Workspace::Home => {}
        }
    }

    /// 异步刷新 Course 视图的需巩固概念数（D2）。
    pub(crate) fn spawn_weak_stats(&mut self, course_id: Option<i64>) {
        let Some(course_id) = course_id else {
            return;
        };
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let weak = spawn_blocking(move || -> Result<usize, String> {
                let mastery = store
                    .list_concepts_with_mastery(Some(course_id))
                    .map_err(|e| e.to_string())?;
                Ok(mastery
                    .iter()
                    .filter(|c| c.attempts > 0 && c.correct * 100 < c.attempts * 70)
                    .count())
            })
            .await;
            if let Ok(Ok(weak)) = weak {
                let _ = tx.send(AppEvent::WeakStats(course_id, weak));
            }
        });
    }

    /// 持久化当前课程（切换/创建/进入 Course 时调用；供 commands.rs 复用）。
    pub(crate) fn persist_last_course_inner(&mut self) {
        if self.course != "all" {
            crate::app::commands::persist_last_course(&self.course);
        }
    }

    /// Home 视图 Enter：光标项动作。
    /// 光标布局 = [AI 入口] + [Continue?] + courses... + [+ New Course]；无 Continue 时首课程紧跟 AI。
    pub(crate) fn home_activate(&mut self) {
        let last = self.home_cursor_count().saturating_sub(1);
        if self.home_cursor == last {
            // [+ New Course]：打开创建向导
            self.open_course_creation_wizard();
            return;
        }
        // Home 首项恒为 AI 接入入口（未配置=Set up AI 引导；已配置=继续接入/管理模型）
        let setup_offset = 1;
        if self.home_cursor == 0 {
            self.start_setup();
            return;
        }
        let continue_course = self.continue_session();
        if self.home_cursor == setup_offset
            && let Some((id, course, _)) = continue_course
        {
            // 换到该会话所属课程的分区（文档 §32），再开会话
            self.swap_course_partition(&course);
            self.enter_session_workspace();
            self.open_session(id);
            return;
        }
        let offset = setup_offset + if continue_course.is_some() { 1 } else { 0 };
        let i = self.home_cursor.saturating_sub(offset);
        let name = self.courses.get(i).map(|(_, n)| n.clone());
        if let Some(name) = name {
            self.enter_course_workspace(&name);
        }
    }

    /// Home 光标当前指向的课程名（None = 指向 AI 入口/Continue/+ New Course）。
    fn home_cursor_course(&self) -> Option<String> {
        let setup = 1; // AI 入口恒占位 0
        let offset = setup
            + if self.continue_session_id().is_some() {
                1
            } else {
                0
            };
        if self.home_cursor < offset {
            return None; // AI / Continue 行
        }
        let i = self.home_cursor - offset;
        self.courses.get(i).map(|(_, n)| n.clone())
    }

    /// Home `d` 键删除课程：首次提示确认，再按一次真正删除（文档 §33）。
    pub(crate) fn home_delete_course(&mut self) {
        let Some(course) = self.home_cursor_course() else {
            return; // 光标不在课程行
        };
        if self.home_delete_armed && self.home_delete_target.as_deref() == Some(course.as_str()) {
            // 第二次：真正删除
            self.home_delete_armed = false;
            self.home_delete_target = None;
            self.delete_course(&course);
        } else {
            self.home_delete_armed = true;
            self.home_delete_target = Some(course.clone());
            self.set_toast(
                format!("按 d 确认删除课程「{course}」（笔记回落 all 区）"),
                true,
            );
        }
    }

    /// 打开 `/course -new` 创建向导（Home「+ New Course」与命令面板共用入口）。
    /// 对话框是模态覆盖层，直接叠在当前 workspace 上弹出——不再先切到 Session。
    /// 若从 Home 发起（workspace==Home），记 pending_course_enter：创建成功直达新课程 Course 页。
    pub(crate) fn open_course_creation_wizard(&mut self) {
        self.pending_course_enter = self.workspace == Workspace::Home;
        self.wizard = Some(crate::wizard::Wizard::new_for(
            crate::wizard::WizardKind::CreateCourse,
            "新建课程",
            "课程名",
        ));
        self.enter_wizard_step();
    }

    /// Course 视图 Enter：动作项。
    /// 光标布局 = [Continue?] New conversation ReviewMap Outline Import + recent sessions。
    pub(crate) fn course_activate(&mut self) {
        let course_id = self.current_course_id();
        let has_continue = self
            .sidebar_sessions
            .iter()
            .any(|s| s.course_id == course_id);
        let mut i = self.course_cursor;
        if has_continue {
            if i == 0 {
                if let Some(id) = self
                    .sidebar_sessions
                    .iter()
                    .find(|s| s.course_id == course_id)
                    .map(|s| s.id)
                {
                    self.enter_session_workspace();
                    self.open_session(id);
                }
                return;
            }
            i -= 1;
        }
        match i {
            0 => self.start_new_session_and_enter(),
            1 => {
                self.enter_session_workspace();
                self.handle_review_map_command(""); // /review-map：打开知识点选择器
            }
            2 => self.handle_outline_command(""),
            3 => self.open_import_wizard(),
            _ => {
                // recent sessions（has_continue 时 offset 已扣）
                let idx = i - 4;
                let sessions: Vec<i64> = self
                    .sidebar_sessions
                    .iter()
                    .filter(|s| s.course_id == course_id)
                    .take(3)
                    .map(|s| s.id)
                    .collect();
                if let Some(&id) = sessions.get(idx) {
                    self.enter_session_workspace();
                    self.open_session(id);
                }
            }
        }
    }

    /// 新会话：清上下文 + 进入 Session workspace（首条消息自动建会话）。
    pub(crate) fn start_new_session_and_enter(&mut self) {
        self.start_new_session();
        self.enter_session_workspace();
    }
}

/// 主循环：绘制 → 等事件 → 处理。
pub async fn run(mut terminal: DefaultTerminal, mut app: App) -> anyhow::Result<()> {
    // 键盘事件转发进统一通道
    let forward_tx = app.tx();
    tokio::spawn(async move {
        let mut reader = crossterm::event::EventStream::new();
        use futures_util::StreamExt;
        while let Some(Ok(ev)) = reader.next().await {
            if forward_tx.send(AppEvent::Input(ev)).is_err() {
                break;
            }
        }
    });

    // 100ms tick：inflight 时驱动 spinner 动画
    let tick_tx = app.tx();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    app.push_entry(Entry::Info(format!(
        "StudyPilot 已就绪 │ 课程: {} │ 模型: {} │ Ctrl+K 命令面板 · /help 能力总览",
        app.course, app.provider_cfg.model
    )));
    app.request_sessions_refresh();

    while !app.should_quit() {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        let Some(ev) = app.rx.recv().await else {
            break;
        };
        match ev {
            AppEvent::Input(CtEvent::Key(k)) => app.handle_key(k),
            AppEvent::Input(CtEvent::Mouse(m)) => app.handle_mouse(m).await,
            AppEvent::Input(_) => {}
            AppEvent::Tick => {
                // inflight（请求/出题）或追问生成中：驱动 spinner 动画
                if app.is_inflight() || app.followup_pending.is_some() {
                    app.tick += 1;
                }
                app.expire_toast();
            }
            AppEvent::Agent(ae) => app.handle_agent_event(ae),
            AppEvent::AgentActivity(ev) => app.on_agent_activity(ev),
            AppEvent::CourseManaged(outcome) => app.on_course_managed(outcome),
            AppEvent::SessionReady(id) => app.on_session_ready(id),
            AppEvent::SessionsLoaded(list) => app.on_sessions_loaded(list),
            AppEvent::SessionOpened(opened) => app.on_session_opened(opened),
            AppEvent::TitleRenamed(ok, title) => app.on_title_renamed(ok, title),
            AppEvent::CourseRenamed(ok, id, name) => app.on_course_renamed(ok, id, name),
            AppEvent::SessionCreateFailed(e) => app.on_session_create_failed(e),
            AppEvent::DatabaseFailed(msg) => {
                app.push_entry(Entry::Error(format!("数据写入失败: {msg}")))
            }
            AppEvent::CostSynced(total) => app.total_cost = app.total_cost.max(total),
            AppEvent::CoursesRefreshed(list, stats) => {
                app.courses = list;
                app.sidebar_course_stats = stats;
                // 导入可能新建课程/删除后列表变化：Home 光标跟随当前课程
                app.sync_home_cursor_to_course();
            }
            AppEvent::BrowserActionDone {
                scope_label,
                result,
            } => app.on_browser_action_done(scope_label, result),
            AppEvent::ImportProgress(ev) => app.handle_import_event(ev),
            AppEvent::BrowserResults { seq, result } => app.on_browser_results(seq, result),
            AppEvent::SessionBrowserResults { seq, result } => {
                app.on_session_browser_results(seq, result)
            }
            AppEvent::TestConnection(text) => {
                app.inflight = None;
                if text.starts_with('✓') {
                    app.push_entry(Entry::Info(text));
                } else {
                    app.push_entry(Entry::Error(text));
                }
            }
            AppEvent::SetupTestDone(text) => {
                app.inflight = None;
                app.on_setup_test_done(text);
            }
            AppEvent::NotesDeleted(result, desc) => match result {
                Ok(true) => {
                    app.push_entry(Entry::Info(format!("已删除: {desc}")));
                    app.request_courses_refresh();
                }
                Ok(false) => app.push_entry(Entry::Error(format!("{desc} 不存在或已删除"))),
                Err(e) => app.push_entry(Entry::Error(format!("删除失败: {e}"))),
            },
            AppEvent::NotesMoved(result, desc) => match result {
                Ok(true) => {
                    app.push_entry(Entry::Info(format!("已移动: {desc}")));
                    app.request_courses_refresh();
                }
                Ok(false) => app.push_entry(Entry::Error(format!("{desc} 不存在"))),
                Err(e) => app.push_entry(Entry::Error(format!("移动失败: {e}"))),
            },
            AppEvent::BudgetReset(result) => app.on_budget_reset(result),
            AppEvent::OutlineReady(result) => app.on_outline_ready(result),
            AppEvent::ReviewReady(result) => app.on_review_ready(result),
            AppEvent::ReviewQuestionReady(result) => app.on_review_question_ready(result),
            AppEvent::ReviewMapReady(result) => {
                app.inflight = None;
                match result {
                    Ok(map) => {
                        app.review_map = Some(map.clone());
                        // 只弹选择器（问题9：地图已由 /outline 渲染，这里不再打印一遍）
                        app.open_review_map_picker();
                    }
                    Err(e) => app.push_entry(Entry::Error(format!("复习地图加载失败: {e}"))),
                }
            }
            AppEvent::WarmupReady(result) => app.on_warmup_ready(result),
            AppEvent::ConceptsRefreshed(result) => {
                app.inflight = None;
                app.request_cost_sync();
                match result {
                    Ok(msg) => app.push_entry(Entry::Info(msg)),
                    Err(e) => app.push_entry(Entry::Error(format!("概念刷新失败: {e}"))),
                }
            }
            AppEvent::ReviewGraded(idx, result) => app.on_review_graded(idx, result),
            AppEvent::ReviewAdvice(text) => app.on_review_advice(text),
            AppEvent::ReviewFollowup(idx, question, result, citations) => {
                app.on_review_followup(idx, question, result, citations)
            }
            AppEvent::CourseSummary {
                course_name,
                notes,
                concepts,
                weak,
                last_session,
            } => app.on_course_summary(course_name, notes, concepts, weak, last_session),
            AppEvent::WeakStats(course_id, weak) => {
                app.weak_stats.insert(course_id, weak);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod strip_tokens_tests {
    use super::strip_placeholder_tokens;

    #[test]
    fn strips_placeholder_tokens_only() {
        assert_eq!(strip_placeholder_tokens("<标题> my rust"), "my rust");
        assert_eq!(strip_placeholder_tokens("rust"), "rust");
        assert_eq!(
            strip_placeholder_tokens("-delete <名> rust"),
            "-delete rust"
        );
        // 全是占位符 → 空（上层按"参数缺失"报用法错误）
        assert_eq!(strip_placeholder_tokens("<标题>"), "");
        // 纯文本聊天不经此路径，但函数本身不破坏非占位符内容
        assert_eq!(strip_placeholder_tokens("a <b c"), "a <b c");
    }
}
