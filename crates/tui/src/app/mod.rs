//! TUI 应用状态、事件分发与主循环。

//! TUI 应用状态、事件类型与聊天任务。

use std::sync::Arc;

use agent_core::{Message, Usage};
use agent_providers::{OpenAiClient, ProviderConfig};
use crossterm::event::Event as CtEvent;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use crate::review;

use crate::ui;
mod chat;
mod commands;
mod import_flow;
mod keys;
mod outline_flow;
mod review_flow;
mod sessions;

pub use crate::events::{AgentEvent, AppEvent, CourseOpOutcome, Entry};
pub use crate::palette::{CommandPalette, ModelPicker};
pub use crate::wizard::Wizard;

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

pub struct App {
    provider: Arc<OpenAiClient>,
    store: Arc<Store>,
    pub provider_cfg: ProviderConfig,
    /// 全部 provider 配置（/model 切换的候选）
    all_providers: Vec<ProviderConfig>,
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
    /// 当前课程分区（M4 仅展示与检索范围占位，/course 切换属后续里程碑）
    pub course: String,

    pub total_usage: Usage,
    pub total_cost: f64,
    /// 本次会话（自启动或 /new /open 起）的累计花费
    pub session_cost: f64,
    /// 预算熔断阈值（config.toml 的 max_cost，M5 接熔断逻辑）
    pub max_cost: f64,

    inflight: Option<CancellationToken>,
    /// 导入任务取消令牌
    import_cancel: Option<CancellationToken>,
    /// 复习模式状态；Some 时按键路由给复习逻辑
    pub review: Option<review::ReviewState>,
    /// 简答题批改进行中（防并发提交错位）
    review_grading: bool,
    /// 选择模式：临时关闭鼠标捕获，允许终端原生文本选中复制
    pub selection_mode: bool,
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
    /// `/sessions` 显式请求后的刷新回调时要打印列表到聊天区（侧栏静默刷新不打印）
    pending_sessions_print: bool,
    should_quit: bool,

    tx: UnboundedSender<AppEvent>,
    rx: UnboundedReceiver<AppEvent>,
}

impl App {
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
            max_cost,
            courses,
            sidebar_course_stats: std::collections::HashMap::new(),
            history: Vec::new(),
            entries: Vec::new(),
            session_state: SessionState::None,
            sidebar_sessions: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            course: "rust".into(),
            total_usage: Usage::default(),
            total_cost: 0.0,
            session_cost: 0.0,
            inflight: None,
            import_cancel: None,
            review: None,
            review_grading: false,
            selection_mode: false,
            text_selection: None,
            selection_anchor: None,
            chat_lines: Vec::new(),
            chat_rect: ratatui::layout::Rect::default(),
            tick: 0,
            scroll_up: 0,
            model_picker: None,
            palette: None,
            wizard: None,
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

    /// header 状态标签：与按键路由同源的覆盖层状态推导（复习 > 导入 > 请求中 > 选择 > 就绪）。
    /// 让用户随时知道"我现在在哪"，替代隐含状态机。
    pub(crate) fn status_label(&self) -> (String, ratatui::style::Color) {
        use ratatui::style::Color;
        if let Some(rs) = &self.review {
            let grading = if self.review_grading {
                " · 批改中…"
            } else {
                ""
            };
            let cur = (rs.current + 1).min(rs.questions.len());
            return (
                format!("复习 {cur}/{} 题{grading}", rs.questions.len()),
                Color::Magenta,
            );
        }
        if self.import_cancel.is_some() {
            return ("导入中 (Ctrl+C 中断)".into(), Color::Yellow);
        }
        if self.is_inflight() {
            return ("思考中…".into(), Color::Yellow);
        }
        if self.selection_mode {
            return ("选择模式".into(), Color::Cyan);
        }
        ("就绪".into(), Color::Green)
    }

    pub(crate) fn push_entry(&mut self, e: Entry) {
        self.entries.push(e);
        // 新内容到达 → 回到底部跟随
        self.scroll_up = 0;
    }

    /// Ctrl+C / Esc：导入中→中断导入；请求中→中断请求；空闲→退出。
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

    pub(crate) fn quit_now(&mut self) {
        if let Some(token) = &self.import_cancel {
            token.cancel();
        }
        if let Some(token) = &self.inflight {
            token.cancel();
        }
        self.should_quit = true;
    }

    /// 切换选择模式：关闭/恢复鼠标捕获，允许终端原生文本选中复制。
    pub(crate) fn toggle_selection_mode(&mut self) {
        self.selection_mode = !self.selection_mode;
        if self.selection_mode {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
            self.push_entry(Entry::Info(
                "选择模式: 鼠标拖选文本即可复制（v 或 Esc 退出）".into(),
            ));
        } else {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
            self.push_entry(Entry::Info("已退出选择模式，滚轮恢复".into()));
        }
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
        "mynotes-agent 已就绪 │ 课程: {} │ 模型: {} │ Ctrl+K 命令面板 · /help 能力总览",
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
                if app.is_inflight() {
                    app.tick += 1;
                }
            }
            AppEvent::Agent(ae) => app.handle_agent_event(ae),
            AppEvent::CourseManaged(outcome) => app.on_course_managed(outcome),
            AppEvent::SessionReady(id) => app.on_session_ready(id),
            AppEvent::SessionsLoaded(list) => app.on_sessions_loaded(list),
            AppEvent::SessionOpened(opened) => app.on_session_opened(opened),
            AppEvent::TitleRenamed(ok, title) => app.on_title_renamed(ok, title),
            AppEvent::SessionCreateFailed(e) => app.on_session_create_failed(e),
            AppEvent::DatabaseFailed(msg) => {
                app.push_entry(Entry::Error(format!("数据写入失败: {msg}")))
            }
            AppEvent::CostSynced(total) => app.total_cost = app.total_cost.max(total),
            AppEvent::ImportProgress(ev) => app.handle_import_event(ev),
            AppEvent::NotesListed(result) => match result {
                Ok(notes) => {
                    if notes.is_empty() {
                        app.push_entry(Entry::Info("当前分区无笔记".into()));
                    }
                    for n in notes {
                        let course_name: String = app
                            .courses
                            .iter()
                            .find(|(id, _)| Some(*id) == n.course_id)
                            .map(|(_, name)| name.clone())
                            .unwrap_or("all".into());
                        app.push_entry(Entry::Info(format!(
                            "  [{short}] #{id} {title} ({course})",
                            short = n.short_id,
                            id = n.id,
                            title = n.title,
                            course = course_name
                        )));
                    }
                }
                Err(e) => app.push_entry(Entry::Error(format!("列出笔记失败: {e}"))),
            },
            AppEvent::NotesDeleted(result, desc) => match result {
                Ok(true) => app.push_entry(Entry::Info(format!("已删除: {desc}"))),
                Ok(false) => app.push_entry(Entry::Error(format!("{desc} 不存在或已删除"))),
                Err(e) => app.push_entry(Entry::Error(format!("删除失败: {e}"))),
            },
            AppEvent::NotesMoved(result, desc) => match result {
                Ok(true) => app.push_entry(Entry::Info(format!("已移动: {desc}"))),
                Ok(false) => app.push_entry(Entry::Error(format!("{desc} 不存在"))),
                Err(e) => app.push_entry(Entry::Error(format!("移动失败: {e}"))),
            },
            AppEvent::BudgetReset(result) => app.on_budget_reset(result),
            AppEvent::OutlineGenerated(content, course, export, titles) => {
                app.on_outline_generated(content, course, export, titles)
            }
            AppEvent::ReviewReady(result) => app.on_review_ready(result),
            AppEvent::ReviewGraded(idx, result) => app.on_review_graded(idx, result),
        }
    }
    Ok(())
}
