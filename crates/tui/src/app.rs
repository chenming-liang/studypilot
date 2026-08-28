//! TUI 应用状态、事件类型与聊天任务。

use std::sync::Arc;

use agent_core::{Agent, Message, Provider, ToolBox, Usage};
use agent_providers::{OpenAiClient, ProviderConfig, estimate_cost};
use crossterm::event::{
    Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;
use tools::{ListCoursesTool, SearchNotesTool};

use crate::review;

use crate::course_cmd::{CourseAction, parse_course_action};
use crate::ui;

// ---- 输入框 UTF-8 编辑辅助（纯函数，单测覆盖）----

/// 字符索引 → 字节偏移；越界返回串长。
fn byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or(s.len(), |(byte, _)| byte)
}

/// 在 pos 处插入字符，返回新光标位置（pos+1）。
fn insert_char(input: &mut String, pos: usize, ch: char) -> usize {
    let pos = pos.min(input.chars().count());
    let byte = byte_offset(input, pos);
    input.insert(byte, ch);
    pos + 1
}

/// 删除 pos 前一个字符，返回新光标位置（pos-1；已在行首则不动）。
fn delete_before(input: &mut String, pos: usize) -> usize {
    if pos == 0 || pos > input.chars().count() {
        return pos.min(input.chars().count());
    }
    let start = byte_offset(input, pos - 1);
    let end = byte_offset(input, pos);
    input.replace_range(start..end, "");
    pos - 1
}

/// 删除 pos 处的字符（光标不动）；pos 越界则不动。
fn delete_at(input: &mut String, pos: usize) {
    if pos >= input.chars().count() {
        return;
    }
    let start = byte_offset(input, pos);
    let end = byte_offset(input, pos + 1);
    input.replace_range(start..end, "");
}
use storage::Store;

/// 后台 LLM 请求 → TUI 的事件。
#[derive(Debug)]
pub enum AgentEvent {
    /// 请求已发出（显示"思考中"）
    Thinking,
    Done {
        content: String,
        reasoning_chars: Option<usize>,
        usage: Usage,
        new_messages: Vec<Message>,
        /// 工具调用轨迹
        tool_trace: Vec<agent_core::ToolTraceEntry>,
    },
    Failed(String),
    Interrupted,
}

/// 课程管理操作的结果：(结果消息, 刷新后的完整列表, 需切换到的分区)
pub type CourseOpOutcome = Result<(String, Vec<(i64, String)>, Option<String>), String>;

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

/// 统一事件源：键盘/鼠标/tick + agent 事件共用一条 mpsc（R4 骨架，M6 导入进度复用）。
#[derive(Debug)]
pub enum AppEvent {
    Input(CtEvent),
    Tick,
    Agent(AgentEvent),
    /// /course 管理操作（新建/删除）完成
    CourseManaged(CourseOpOutcome),
    /// 新会话已创建（含首条待落库消息的会话）
    SessionReady(i64),
    /// 侧栏会话列表刷新完成
    SessionsLoaded(Vec<storage::SessionMeta>),
    /// /open 加载历史会话完成：(会话 id, 课程分区, 消息序列)
    SessionOpened(Result<(i64, Option<i64>, Vec<Message>), String>),
    /// /rename 完成：(是否成功, 新标题)
    TitleRenamed(bool, String),
    /// 导入进度事件
    ImportProgress(importer::ImportEvent),
    /// /notes 列表查询完成
    NotesListed(Result<Vec<storage::NoteSummary>, String>),
    /// /delete 完成：(是否成功, 描述)
    NotesDeleted(Result<bool, String>, String),
    /// /move 完成：(是否成功, 描述)
    NotesMoved(Result<bool, String>, String),
    /// /budget reset 完成
    BudgetReset(Result<(), String>),
    /// /outline 完成：(LLM 返回的 JSON 文本, 课程名, 是否导出, 笔记标题列表)
    OutlineGenerated(Option<String>, String, bool, Vec<(i64, String)>),
    /// /review 出题完成
    ReviewReady(Result<review::ReviewState, String>),
    /// 简答题批改完成：(题目索引, (score, missing, comment))
    ReviewGraded(usize, Result<(i64, Vec<String>, Option<String>), String>),
    /// 会话创建失败：Pending 态无法继续落库，状态已回退，需用户重新发消息重试
    SessionCreateFailed(String),
    /// 后台 DB 写入失败（attempts / 掌握度等闭环数据）
    DatabaseFailed(String),
    /// usage_log 对账后的累计成本（R6：review/outline/import 的花费也纳入熔断口径）
    CostSynced(f64),
}

/// 聊天流里的一条内容。
#[derive(Debug, Clone)]
pub enum Entry {
    /// 灰色系统信息（帮助/中断提示等）
    Info(String),
    /// 用户消息
    User(String),
    /// 助手回复；reasoning_chars 为思考长度（D7：思考文本不计入正文）
    Assistant {
        content: String,
        reasoning_chars: Option<usize>,
    },
    Error(String),
}

/// 模型选择弹窗状态。
pub struct ModelPicker {
    pub options: Vec<ProviderConfig>,
    pub selected: usize,
}

/// 命令面板条目：`needs_arg` 决定 Enter 行为——无参命令直接执行，
/// 带参命令填入输入框等待用户补全。
pub struct PaletteItem {
    pub command: &'static str,
    pub desc: &'static str,
    pub needs_arg: bool,
}

/// 命令面板（Ctrl+K）：可发现性入口——把"项目有什么能力"直接摆在眼前，
/// 交互与 /model 弹窗同范式（覆盖层 + 按键路由优先级）。
pub struct CommandPalette {
    pub items: Vec<PaletteItem>,
    /// 过滤后命中的 items 下标
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub filter: String,
}

impl CommandPalette {
    /// 全量命令表（与 /help 能力文案一一对应）。
    fn all_items() -> Vec<PaletteItem> {
        use PaletteItem as P;
        vec![
            P {
                command: "/help",
                desc: "我能做什么（能力总览）",
                needs_arg: false,
            },
            P {
                command: "/outline",
                desc: "生成当前课程知识大纲",
                needs_arg: false,
            },
            P {
                command: "/outline <课程> --export",
                desc: "生成大纲并导出 Markdown",
                needs_arg: true,
            },
            P {
                command: "/review <课程> [概念] [--n 数量]",
                desc: "出题复习（掌握度低优先）",
                needs_arg: true,
            },
            P {
                command: "/import <目录> [--course 名]",
                desc: "批量导入 md/pdf/pptx",
                needs_arg: true,
            },
            P {
                command: "/notes",
                desc: "列出当前分区的笔记",
                needs_arg: false,
            },
            P {
                command: "/course",
                desc: "查看课程分区",
                needs_arg: false,
            },
            P {
                command: "/model",
                desc: "切换模型（弹窗）",
                needs_arg: false,
            },
            P {
                command: "/budget",
                desc: "查看预算花费",
                needs_arg: false,
            },
            P {
                command: "/sessions",
                desc: "历史会话列表",
                needs_arg: false,
            },
            P {
                command: "/new",
                desc: "开启新会话",
                needs_arg: false,
            },
            P {
                command: "/export",
                desc: "导出当前会话为 JSON",
                needs_arg: false,
            },
            P {
                command: "/load <文件>",
                desc: "加载导出的会话 JSON",
                needs_arg: true,
            },
            P {
                command: "/open <id>",
                desc: "恢复指定历史会话",
                needs_arg: true,
            },
            P {
                command: "/rename <标题>",
                desc: "重命名当前会话",
                needs_arg: true,
            },
            P {
                command: "/delete <id>",
                desc: "删除笔记（不碰磁盘原文件）",
                needs_arg: true,
            },
            P {
                command: "/move <id> <课程>",
                desc: "迁移笔记到另一课程",
                needs_arg: true,
            },
            P {
                command: "/course -new <名>",
                desc: "新建课程分区",
                needs_arg: true,
            },
            P {
                command: "/course -delete <名>",
                desc: "删除课程（笔记回落 all 区）",
                needs_arg: true,
            },
        ]
    }

    pub fn new() -> Self {
        let items = Self::all_items();
        let filtered = (0..items.len()).collect();
        Self {
            items,
            filtered,
            selected: 0,
            filter: String::new(),
        }
    }

    fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                f.is_empty()
                    || i.command.to_lowercase().contains(&f)
                    || i.desc.to_lowercase().contains(&f)
            })
            .map(|(idx, _)| idx)
            .collect();
        self.selected = 0;
    }
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
    /// `/sessions` 显式请求后的刷新回调时要打印列表到聊天区（侧栏静默刷新不打印）
    pending_sessions_print: bool,
    should_quit: bool,

    tx: UnboundedSender<AppEvent>,
    rx: UnboundedReceiver<AppEvent>,
}

impl App {
    pub fn new(
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
            pending_sessions_print: false,
            should_quit: false,
            tx,
            rx,
        }
    }

    pub fn tx(&self) -> UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn is_inflight(&self) -> bool {
        self.inflight.is_some()
    }

    /// header 状态标签：与按键路由同源的覆盖层状态推导（复习 > 导入 > 请求中 > 选择 > 就绪）。
    /// 让用户随时知道"我现在在哪"，替代隐含状态机。
    pub fn status_label(&self) -> (String, ratatui::style::Color) {
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

    pub fn push_entry(&mut self, e: Entry) {
        self.entries.push(e);
        // 新内容到达 → 回到底部跟随
        self.scroll_up = 0;
    }

    /// Ctrl+C / Esc：导入中→中断导入；请求中→中断请求；空闲→退出。
    pub fn interrupt_or_quit(&mut self) {
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

    pub fn quit_now(&mut self) {
        if let Some(token) = &self.import_cancel {
            token.cancel();
        }
        if let Some(token) = &self.inflight {
            token.cancel();
        }
        self.should_quit = true;
    }

    /// 切换选择模式：关闭/恢复鼠标捕获，允许终端原生文本选中复制。
    fn toggle_selection_mode(&mut self) {
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

    pub fn handle_key(&mut self, key: KeyEvent) {
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
        // 命令面板（Ctrl+K 唤起）覆盖普通输入
        if self.palette.is_some() {
            self.handle_palette_key(key);
            return;
        }
        // 弹窗打开时按键优先由弹窗处理
        if self.model_picker.is_some() {
            self.handle_picker_key(key);
            return;
        }
        // Ctrl+K 打开命令面板
        if let KeyCode::Char('k') = key.code
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.palette = Some(CommandPalette::new());
            return;
        }
        // v 键切换选择模式（临时关闭鼠标捕获，允许终端原生选中复制）
        if let KeyCode::Char('v') = key.code
            && !key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.toggle_selection_mode();
            return;
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.interrupt_or_quit();
            }
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit_now();
            }
            KeyCode::Esc => {
                if self.selection_mode {
                    self.toggle_selection_mode();
                } else {
                    self.interrupt_or_quit();
                }
            }
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
            }
            KeyCode::Delete => delete_at(&mut self.input, self.cursor_pos),
            KeyCode::PageUp => self.scroll_up = self.scroll_up.saturating_add(10),
            KeyCode::PageDown => self.scroll_up = self.scroll_up.saturating_sub(10),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor_pos = insert_char(&mut self.input, self.cursor_pos, c);
            }
            _ => {}
        }
    }

    /// 鼠标：滚轮滚动 + 左键拖选复制聊天内容。
    pub async fn handle_mouse(&mut self, event: MouseEvent) {
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
    fn update_text_selection(&mut self, row: u16, col: u16, is_down: bool) {
        let inner = self.chat_rect.inner(ratatui::layout::Margin::new(1, 1));
        if !inner.contains(ratatui::layout::Position { x: col, y: row }) {
            return;
        }

        // 自动翻页：贴近视口上下边缘时滚动（拖选超出可视范围时跟随）
        let max_offset = self.chat_lines.len().saturating_sub(inner.height as usize);
        let near_top = row <= inner.y.saturating_add(1);
        let near_bottom = row >= inner.bottom().saturating_sub(2);
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
    async fn copy_text_selection(&mut self) {
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

        match result {
            Ok(()) => {
                // 直接写 entries，保留当前滚动位置（push_entry 会回到底部）
                self.entries
                    .push(Entry::Info(format!("已复制 {count} 行到剪贴板")));
            }
            Err(e) => self.entries.push(Entry::Error(format!("复制失败: {e}"))),
        }
    }

    /// 用户按 Enter 提交。斜杠命令随时可执行；
    /// 普通消息请求中忽略新提交（串行，避免费用与状态混乱）。
    pub fn submit(&mut self) {
        let text = self.input.trim().to_owned();
        self.input.clear();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') {
            self.handle_command(&text);
            return;
        }
        if self.is_inflight() {
            self.push_entry(Entry::Info(
                "请求进行中，请等待完成或 Ctrl+C 中断后再发送".into(),
            ));
            return;
        }
        // R6 预算熔断：发起下一个请求前检查，超限拒绝并提示
        if self.total_cost >= self.max_cost {
            self.push_entry(Entry::Error(format!(
                "已达预算上限 ¥{:.2}（累计 ¥{:.4}），拒绝发送。可在 config.toml 调高 max_cost",
                self.max_cost, self.total_cost
            )));
            return;
        }

        let msg = Message::user(text.clone());
        self.history.push(msg.clone());
        self.push_entry(Entry::User(text));
        self.persist(msg);
        self.spawn_chat();
    }

    /// 最小命令分发：已实现的执行，未实现的按里程碑提示（错误信息即帮助）。
    fn handle_command(&mut self, text: &str) {
        self.push_entry(Entry::User(text.to_owned()));

        let mut parts = text.split_whitespace();
        let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
        let arg = parts.collect::<Vec<_>>().join(" ");

        // inflight 门控：会话切换/替换类命令在请求进行中拒绝，
        // 防止 Done 事件把上一个会话的消息写入新会话（跨会话数据混写）
        const SESSION_SWITCHERS: &[&str] = &["/open", "/load", "/new", "/rename"];
        if SESSION_SWITCHERS.contains(&cmd.as_str()) && self.is_inflight() {
            self.push_entry(Entry::Error(
                "请求进行中，请等待完成或 Ctrl+C 中断后再执行会话切换".into(),
            ));
            return;
        }

        match cmd.as_str() {
            "/help" => {
                for l in [
                    "我能做什么？",
                    "",
                    "【学习】直接打字提问即可，无需命令",
                    "  例: \"解释一下虚拟内存\"  \"这里为什么必须用 mutex\"",
                    "  回答策略: 笔记优先并标注 [n] 引用；笔记没写的会用",
                    "  自身知识补充并明示「（笔记外补充）」",
                    "",
                    "【知识库】",
                    "  /import <目录> [--course 名]   批量导入 md/pdf/pptx",
                    "  /notes                        列出当前分区笔记",
                    "  /delete <id> | /move <id> <课程>  管理笔记（不碰磁盘原文件）",
                    "  /course <课程|all>            切换分区；-new/-delete 管理",
                    "",
                    "【复习】",
                    "  /review <课程> [概念] [--n 数量]  出题（选择+简答，掌握度低优先）",
                    "",
                    "【大纲】",
                    "  /outline [课程] [--export]    生成课程知识大纲",
                    "",
                    "【系统】",
                    "  /model [名]      切换模型      /budget [金额|reset]  预算",
                    "  /new             新会话        /sessions             历史会话",
                    "  /open <id>       恢复会话      /rename <标题>        重命名",
                    "  /export /load    会话导出/导入",
                    "",
                    "快捷键: Ctrl+K 命令面板 · v 选择模式 · Ctrl+C 中断/退出 · Ctrl+Q 强退",
                ] {
                    self.push_entry(Entry::Info(l.into()));
                }
            }
            "/course" => self.handle_course_command(&arg),
            "/model" => self.handle_model_command(arg.trim()),
            "/new" => self.start_new_session(),
            "/sessions" => {
                // 不直接打印缓存的旧列表：等 SessionsLoaded 事件回来再展示最新数据
                self.pending_sessions_print = true;
                self.request_sessions_refresh();
            }
            "/open" => match arg.trim().parse::<i64>() {
                Ok(id) => self.open_session(id),
                Err(_) => self.push_entry(Entry::Error("用法: /open <会话 id>".into())),
            },
            "/export" => self.export_session(),
            "/rename" => self.rename_session(arg.trim()),
            "/load" => {
                if arg.trim().is_empty() {
                    self.push_entry(Entry::Error("用法: /load <导出的 JSON 文件路径>".into()));
                } else {
                    self.load_session_file(arg.trim());
                }
            }
            "/import" => self.handle_import_command(arg.trim()),
            "/delete" => self.handle_delete_command(arg.trim()),
            "/move" => self.handle_move_command(arg.trim()),
            "/budget" => self.handle_budget_command(arg.trim()),
            "/notes" => self.handle_notes_command(),
            "/outline" => self.handle_outline_command(arg.trim()),
            "/search" => self.not_implemented("/search", "M7（检索能力经 agent 工具自动调度）"),
            "/review" => self.handle_review_command(arg.trim()),
            _ => {
                self.push_entry(Entry::Error(format!(
                    "未知命令 `{cmd}`，输入 /help 查看可用命令"
                )));
            }
        }
    }

    fn not_implemented(&mut self, cmd: &str, milestone: &str) {
        self.push_entry(Entry::Error(format!(
            "{cmd} 尚未实现（计划于 {milestone}）"
        )));
    }

    // ---- 预算管理 ----

    /// `/budget`：查/改/重置预算。
    fn handle_budget_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let remaining = (self.max_cost - self.total_cost).max(0.0);
            self.push_entry(Entry::Info(format!(
                "预算: 累计 ¥{:.4} / 上限 ¥{:.2} / 剩余 ¥{:.4}",
                self.total_cost, self.max_cost, remaining
            )));
            return;
        }

        if arg == "reset" {
            let store = Arc::clone(&self.store);
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let result =
                    spawn_blocking(move || store.clear_usage_log().map_err(|e| e.to_string()))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                let _ = tx.send(AppEvent::BudgetReset(result));
            });
            return;
        }

        // 设置新上限
        match arg.parse::<f64>() {
            Ok(val) if val > 0.0 => {
                self.max_cost = val;
                self.push_entry(Entry::Info(format!("预算上限已设为 ¥{val:.2}")));
            }
            _ => {
                self.push_entry(Entry::Error(
                    "用法: /budget [金额 | reset]（金额须为正数）".into(),
                ));
            }
        }
    }

    fn on_budget_reset(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.total_cost = 0.0;
                self.session_cost = 0.0;
                self.push_entry(Entry::Info("累计花费已清零".into()));
            }
            Err(e) => self.push_entry(Entry::Error(format!("清零失败: {e}"))),
        }
    }

    // ---- M8：复习模式 ----

    /// `/review <课程> [概念] [--n 数量]`：启动复习出题。
    fn handle_review_command(&mut self, arg: &str) {
        if self.review.is_some() {
            self.push_entry(Entry::Error("复习进行中，请先完成或 Esc 退出".into()));
            return;
        }

        let tokens: Vec<&str> = arg.split_whitespace().collect();
        if tokens.is_empty() {
            self.push_entry(Entry::Error(
                "用法: /review <课程名> [概念关键词] [--n 数量]".into(),
            ));
            return;
        }

        let course_name = tokens[0].to_owned();
        if course_name == "all" {
            self.push_entry(Entry::Error("复习需指定具体课程，不能为 all".into()));
            return;
        }

        let Some(course_id) = self
            .courses
            .iter()
            .find(|(_, n)| *n == course_name.as_str())
            .map(|(id, _)| *id)
        else {
            self.push_entry(Entry::Error(format!("课程 `{course_name}` 不存在")));
            return;
        };

        // 解析 --n 数量（默认 5）
        let n = tokens
            .windows(2)
            .find(|w| w[0] == "--n")
            .and_then(|w| w[1].parse::<usize>().ok())
            .unwrap_or(5);

        // 概念关键词 = 去掉课程名和 --n 后的部分
        let scope: String = tokens[1..]
            .iter()
            .filter(|s| !s.starts_with("--"))
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let scope = if scope.is_empty() {
            course_name.clone()
        } else {
            scope
        };

        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();

        self.push_entry(Entry::Info(format!(
            "开始出题: 《{course_name}》范围「{scope}」，{n} 题"
        )));

        tokio::spawn(async move {
            review::start_review(
                store,
                provider,
                provider_cfg,
                course_id,
                course_name,
                scope,
                n,
                tx,
                CancellationToken::new(),
            )
            .await;
        });
    }

    /// 复习模式按键处理：选择题 1-4 / 简答题 Enter 提交 / Esc 退出。
    fn handle_review_key(&mut self, key: KeyEvent) {
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

    /// 完成一道题：记录 attempt + 更新掌握度 + 推进到下一题。
    fn finish_review_question(
        &mut self,
        correct: bool,
        score: Option<i64>,
        feedback: &str,
        missing: &[String],
    ) {
        // ① 提取题目数据（不可变借用结束后再操作 self）
        let (db_id, concept_id) = match &self.review {
            Some(rs) => match rs.questions.get(rs.current) {
                Some(q) => (q.db_id, q.concept_id),
                None => return,
            },
            None => return,
        };

        // ② 记录 attempt + 更新掌握度（spawn_blocking，D2）；失败上抛可见
        let store = Arc::clone(&self.store);
        let answer = feedback.to_owned();
        let missing_json = if missing.is_empty() {
            None
        } else {
            serde_json::to_string(missing).ok()
        };
        let is_correct = correct;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result: Result<(), String> = spawn_blocking(move || -> Result<(), String> {
                store
                    .record_attempt(db_id, &answer, score, missing_json.as_deref())
                    .map_err(|e| format!("attempt 写入失败: {e}"))?;
                if let Some(cid) = concept_id {
                    store
                        .update_concept_mastery(cid, is_correct)
                        .map_err(|e| format!("掌握度更新失败: {e}"))?;
                }
                Ok(())
            })
            .await
            .unwrap_or_else(|e| Err(format!("任务错误: {e}")));
            // attempts/mastery 是掌握度闭环的数据源，静默丢失会让复习质量悄悄劣化
            if let Err(msg) = result {
                let _ = tx.send(AppEvent::DatabaseFailed(msg));
            }
        });

        // ③ 更新 review 状态
        let advance = if let Some(rs) = &mut self.review {
            rs.results.push(review::ReviewResult {
                correct,
                score,
                feedback: feedback.to_owned(),
                missing: missing.to_vec(),
            });
            rs.current += 1;
            rs.current >= rs.questions.len()
        } else {
            false
        };

        // ④ 渲染下一题或结束
        if advance {
            self.finish_review();
        } else {
            self.render_current_question();
        }
    }

    fn render_current_question(&mut self) {
        // 提取数据，避免借用冲突
        let info = self.review.as_ref().and_then(|rs| {
            rs.questions
                .get(rs.current)
                .map(|q| (rs.current, rs.questions.len(), q.clone()))
        });
        let Some((current, total, q)) = info else {
            return;
        };

        let type_str = if q.q_type == review::QType::Choice {
            "选择题"
        } else {
            "简答题"
        };
        self.push_entry(Entry::Info(format!(
            "[{}/{}] {type_str}",
            current + 1,
            total
        )));
        self.push_entry(Entry::User(q.question.clone()));
        if q.q_type == review::QType::Choice {
            for (i, opt) in q.options.iter().enumerate() {
                self.push_entry(Entry::Info(format!("  {}. {opt}", i + 1)));
            }
            self.push_entry(Entry::Info("数字键 1-9 作答".into()));
        } else {
            self.push_entry(Entry::Info("输入答案后 Enter 提交（Esc 退出复习）".into()));
        }
    }

    fn finish_review(&mut self) {
        let rs = self.review.take();
        let Some(rs) = rs else { return };

        let total = rs.questions.len();
        let correct_count = rs.results.iter().filter(|r| r.correct).count();
        self.push_entry(Entry::Info(format!(
            "=== 复习完成: {correct_count}/{total} 正确 ==="
        )));
        let missed: Vec<&str> = rs
            .results
            .iter()
            .filter(|r| !r.correct)
            .map(|r| r.feedback.as_str())
            .collect();
        if !missed.is_empty() {
            self.push_entry(Entry::Info("薄弱点:".into()));
            for m in &missed {
                self.push_entry(Entry::Info(format!("  • {m}")));
            }
        }
    }

    fn exit_review(&mut self, msg: &str) {
        self.review = None;
        self.push_entry(Entry::Info(msg.to_owned()));
    }

    fn on_review_ready(&mut self, result: Result<review::ReviewState, String>) {
        self.request_cost_sync();
        match result {
            Ok(rs) => {
                let count = rs.questions.len();
                self.push_entry(Entry::Info(format!("复习开始: 共 {count} 题（Esc 退出）")));
                self.review = Some(rs);
                self.render_current_question();
            }
            Err(e) => {
                self.push_entry(Entry::Error(format!("出题失败: {e}")));
            }
        }
    }

    fn on_review_graded(
        &mut self,
        question_index: usize,
        result: Result<(i64, Vec<String>, Option<String>), String>,
    ) {
        self.review_grading = false;
        self.request_cost_sync();
        // 校验事件索引：过期/错位的批改结果直接丢弃（防 attempts/mastery 污染）
        let idx_ok = self
            .review
            .as_ref()
            .map(|rs| rs.current == question_index)
            .unwrap_or(false);
        if !idx_ok {
            tracing::warn!(question_index, "过期批改结果被丢弃");
            return;
        }
        match result {
            Ok((score, missing, comment)) => {
                let correct = score >= 70;
                let feedback = format!(
                    "得分: {score}/100{}",
                    if let Some(c) = &comment {
                        format!(" | {c}")
                    } else {
                        String::new()
                    }
                );
                self.push_entry(Entry::Info(feedback.clone()));
                if !missing.is_empty() {
                    self.push_entry(Entry::Info(format!("缺失要点: {}", missing.join("；"))));
                }
                self.finish_review_question(correct, Some(score), &feedback, &missing);
            }
            Err(e) => {
                self.push_entry(Entry::Error(format!("批改失败: {e}")));
                self.finish_review_question(false, None, &e, &Vec::new());
            }
        }
    }

    /// `/outline [课程] [--export]`：无参默认当前课程。
    fn handle_outline_command(&mut self, arg: &str) {
        let export = arg.contains("--export");
        let name = arg.replace("--export", "").trim().to_owned();
        let name = if name.is_empty() {
            self.course.clone()
        } else {
            name
        };
        if name == "all" {
            self.push_entry(Entry::Error("大纲需指定具体课程，不能为 all".into()));
            return;
        }
        let Some(course_id) = self
            .courses
            .iter()
            .find(|(_, n)| *n == name.as_str())
            .map(|(id, _)| *id)
        else {
            self.push_entry(Entry::Error(format!("课程 `{name}` 不存在")));
            return;
        };

        let store = Arc::clone(&self.store);
        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let tx = self.tx.clone();
        let course_name = name.clone();

        tokio::spawn(async move {
            let store_clone = Arc::clone(&store);
            let gathered = spawn_blocking(move || {
                let titles = store_clone.list_note_titles_by_course(course_id)?;
                let concepts = store_clone.list_concept_names_by_course(course_id)?;
                Ok::<_, storage::Error>((titles, concepts))
            })
            .await;

            let (titles, concepts) = match gathered {
                Ok(Ok((t, c))) if !t.is_empty() => (t, c),
                _ => {
                    let _ = tx.send(AppEvent::OutlineGenerated(
                        None,
                        course_name,
                        export,
                        Vec::new(),
                    ));
                    return;
                }
            };

            let note_list: String = titles
                .iter()
                .enumerate()
                .map(|(i, (_, t))| format!("[{}] {}", i + 1, t))
                .collect::<Vec<_>>()
                .join("\n");
            let concept_list = concepts.join("、");
            let prompt = format!(
                "以下是《{course_name}》课程的笔记列表和已提取的概念。\n\
                 请生成结构化大纲，输出 JSON：\n\
                 {{\"sections\": [{{\"title\": \"章节\", \"points\": [\"知识点\"], \"refs\": [笔记编号]}}]}}\n\n\
                 笔记列表:\n{note_list}\n\n已提取概念: {concept_list}\n\n\
                 重要要求：\n\
                 1. 每个 section 的 refs 必须包含至少一个笔记编号\n\
                 2. refs 里的数字是上面笔记列表中的 [编号]\n\
                 3. 按知识逻辑组织章节，不要只罗列笔记标题"
            );
            let messages = [
                Message::system("只输出 JSON，不要 markdown 代码块。"),
                Message::user(&prompt),
            ];
            // R6：outline 的 LLM 调用也记 usage + cost
            let pc = provider_cfg.clone();
            let content = match provider.chat_json(&messages).await {
                Ok(resp) => {
                    Self::log_llm_usage(&store, &pc, "outline", &resp.usage);
                    Some(resp.content)
                }
                Err(e) if e.is_json_mode_unsupported() => {
                    match provider.chat(&messages, &[]).await {
                        Ok(resp) => {
                            Self::log_llm_usage(&store, &pc, "outline", &resp.usage);
                            Some(resp.content)
                        }
                        Err(e) => {
                            tracing::warn!("大纲生成失败: {e}");
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("大纲生成失败: {e}");
                    None
                }
            };
            let _ = tx.send(AppEvent::OutlineGenerated(
                content,
                course_name,
                export,
                titles,
            ));
        });
    }

    fn on_outline_generated(
        &mut self,
        content: Option<String>,
        course: String,
        export: bool,
        titles: Vec<(i64, String)>,
    ) {
        self.request_cost_sync();
        let Some(json_str) = content else {
            self.push_entry(Entry::Error("大纲生成失败（LLM 无响应或无笔记）".into()));
            return;
        };
        // D4 兜底：直解失败 → 提取第一个 {...} 块再解
        let outline: serde_json::Value = match serde_json::from_str(json_str.trim()) {
            Ok(v) => v,
            Err(_) => {
                let Some(block) = agent_core::first_json_block(&json_str)
                    .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                else {
                    self.push_entry(Entry::Error("大纲 JSON 解析失败".into()));
                    return;
                };
                block
            }
        };

        if export {
            let md = render_outline_markdown(&outline, &course, &titles);
            let dir = std::path::Path::new("data/exports");
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join(format!("{course}-outline.md"));
            match std::fs::write(&path, &md) {
                Ok(()) => self.push_entry(Entry::Info(format!(
                    "大纲已导出到 {}（{} 字节）",
                    path.display(),
                    md.len()
                ))),
                Err(e) => self.push_entry(Entry::Error(format!("导出失败: {e}"))),
            }
        } else {
            self.push_entry(Entry::Info(format!("《{course}》课程大纲:")));
            for line in render_outline_tree(&outline, &titles) {
                self.push_entry(Entry::Info(line));
            }
        }
    }

    // ---- M6：笔记管理 ----

    /// `/notes`：列出当前课程（或 all）的笔记摘要。
    fn handle_notes_command(&mut self) {
        let cid = if self.course == "all" {
            None
        } else {
            self.courses
                .iter()
                .find(|(_, n)| *n == self.course)
                .map(|(id, _)| *id)
        };
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result =
                spawn_blocking(move || store.list_notes(cid, 50).map_err(|e| e.to_string()))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
            let _ = tx.send(AppEvent::NotesListed(result));
        });
    }

    /// `/delete <id>` 或 `/delete --course <name>`
    fn handle_delete_command(&mut self, arg: &str) {
        if arg.is_empty() {
            self.push_entry(Entry::Error(
                "用法: /delete <笔记 id> 或 /delete --course <课程名>".into(),
            ));
            return;
        }

        if let Some(course_name) = arg
            .strip_prefix("--course ")
            .or_else(|| arg.strip_prefix("--course"))
        {
            let name = course_name.trim();
            if name.is_empty() || name == "all" {
                self.push_entry(Entry::Error("--course 不能为空或 all".into()));
                return;
            }
            let course_id = self
                .courses
                .iter()
                .find(|(_, n)| *n == name)
                .map(|(id, _)| *id);
            let Some(cid) = course_id else {
                self.push_entry(Entry::Error(format!("课程 `{name}` 不存在")));
                return;
            };
            let store = Arc::clone(&self.store);
            let tx = self.tx.clone();
            let name = name.to_owned();
            tokio::spawn(async move {
                let result = spawn_blocking(move || {
                    store
                        .delete_notes_by_course(cid)
                        .map(|n| n > 0)
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                let _ = tx.send(AppEvent::NotesDeleted(result, format!("课程 {name}")));
            });
            return;
        }

        // 单条删除
        match arg.parse::<i64>() {
            Ok(id) => {
                let store = Arc::clone(&self.store);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result =
                        spawn_blocking(move || store.delete_note(id).map_err(|e| e.to_string()))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r);
                    let _ = tx.send(AppEvent::NotesDeleted(result, format!("笔记 #{id}")));
                });
            }
            Err(_) => {
                self.push_entry(Entry::Error(format!(
                    "无法解析 id `{arg}`。用法: /delete <笔记 id>"
                )));
            }
        }
    }

    /// `/move <id> <课程名>`
    fn handle_move_command(&mut self, arg: &str) {
        let parts: Vec<&str> = arg.split_whitespace().collect();
        if parts.len() < 2 {
            self.push_entry(Entry::Error("用法: /move <笔记 id> <目标课程名>".into()));
            return;
        }
        let id = parts[0];
        let target = parts[1..].join(" ");
        let Ok(id) = id.parse::<i64>() else {
            self.push_entry(Entry::Error(format!("无法解析 id `{id}`")));
            return;
        };
        let target_course_id = if target == "all" {
            None
        } else {
            match self.courses.iter().find(|(_, n)| *n == target.as_str()) {
                Some((cid, _)) => Some(*cid),
                None => {
                    self.push_entry(Entry::Error(format!(
                        "课程 `{target}` 不存在。可用: /course -new {target} 新建"
                    )));
                    return;
                }
            }
        };
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        let target = target.to_owned();
        tokio::spawn(async move {
            let result = spawn_blocking(move || {
                store
                    .move_note(id, target_course_id)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = tx.send(AppEvent::NotesMoved(
                result,
                format!("笔记 #{id} → {target}"),
            ));
        });
    }

    /// `/import <dir> [--course <name>]`：启动导入任务（逐文件串行，进度经事件通道上报）。
    fn handle_import_command(&mut self, arg: &str) {
        if self.import_cancel.is_some() {
            self.push_entry(Entry::Error("导入任务进行中，Ctrl+C 可中断".into()));
            return;
        }

        // 解析: <dir> [--course <name>]
        let tokens: Vec<&str> = arg.split_whitespace().collect();
        if tokens.is_empty() {
            self.push_entry(Entry::Error(
                "用法: /import <目录> [--course <课程名>]".into(),
            ));
            return;
        }
        let dir = tokens[0].to_owned();
        let course = tokens
            .windows(2)
            .find(|w| w[0] == "--course")
            .map(|w| w[1].to_owned());

        let path = std::path::PathBuf::from(&dir);
        if !path.exists() {
            self.push_entry(Entry::Error(format!("目录不存在: {dir}")));
            return;
        }

        let cancel = CancellationToken::new();
        self.import_cancel = Some(cancel.clone());

        let store = Arc::clone(&self.store);
        let provider = Arc::clone(&self.provider);
        let provider_cfg = self.provider_cfg.clone();
        let config = importer::ImportConfig {
            dir: path,
            course: course.clone(),
            max_cost: self.max_cost,
        };

        self.push_entry(Entry::Info(format!(
            "开始导入: {dir}{}",
            course
                .as_deref()
                .map(|c| format!(" → 课程 {c}"))
                .unwrap_or_default()
        )));

        let (import_tx, mut import_rx) = unbounded_channel::<importer::ImportEvent>();
        let tx_main = self.tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = import_rx.recv().await {
                let _ = tx_main.send(AppEvent::ImportProgress(ev));
            }
        });
        tokio::spawn(async move {
            importer::import_directory(store, provider, provider_cfg, config, import_tx, cancel)
                .await;
        });
    }

    fn handle_import_event(&mut self, ev: importer::ImportEvent) {
        use importer::ImportEvent::*;
        match ev {
            Started { total, course } => {
                self.push_entry(Entry::Info(format!(
                    "导入开始: 共 {total} 个文件{}",
                    course
                        .as_deref()
                        .map(|c| format!(" → 课程 {c}"))
                        .unwrap_or_default()
                )));
            }
            FileStart { name, index, total } => {
                self.push_entry(Entry::Info(format!("[{index}/{total}] 导入: {name}")));
            }
            FileDone { name, concepts } => {
                self.push_entry(Entry::Info(format!("  ✓ {name} → {} 个概念", concepts)));
            }
            FileSkipped { name } => {
                self.push_entry(Entry::Info(format!("  ⊘ {name}（已存在，跳过）")));
            }
            ChunkFail { name, err } => {
                self.push_entry(Entry::Error(format!(
                    "  ⚠ {name}: 切片入库失败，该笔记暂不可检索: {err}"
                )));
            }
            FileFail { name, err } => {
                self.push_entry(Entry::Error(format!("  ✗ {name}: {err}")));
            }
            Finished { ok, skipped, fail } => {
                self.import_cancel = None;
                self.request_sessions_refresh();
                // 导入期 LLM 调用的花费只落 usage_log，对账进内存熔断口径
                self.request_cost_sync();
                self.push_entry(Entry::Info(format!(
                    "导入完成: ✓ {ok} 新建 / ⊘ {skipped} 跳过 / ✗ {fail} 失败"
                )));
            }
            Cancelled => {
                self.import_cancel = None;
                self.push_entry(Entry::Info("[导入已中断]".into()));
            }
        }
    }

    /// `/model [名称]`：无参列出全部 provider（当前打标），带参运行时切换。
    /// `/model`：无参弹窗选择；`<名>` 直接切换。
    fn handle_model_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let selected = self
                .all_providers
                .iter()
                .position(|p| p.name == self.provider_cfg.name)
                .unwrap_or(0);
            self.model_picker = Some(ModelPicker {
                options: self.all_providers.clone(),
                selected,
            });
            return;
        }
        self.apply_provider_switch(arg);
    }

    /// 面板按键：字符进过滤串、↑↓ 选择、Enter 执行、Tab 填入输入框、Esc 关闭。
    fn handle_palette_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Up => {
                if let Some(p) = &mut self.palette {
                    p.selected = p.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(p) = &mut self.palette
                    && !p.filtered.is_empty()
                {
                    p.selected = (p.selected + 1).min(p.filtered.len() - 1);
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = &mut self.palette {
                    p.filter.pop();
                    p.refilter();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(p) = &mut self.palette {
                    p.filter.push(c);
                    p.refilter();
                }
            }
            KeyCode::Enter | KeyCode::Tab => self.palette_execute(),
            _ => {}
        }
    }

    /// 执行面板选中项：带参命令填入输入框等用户补全，无参命令直接提交。
    fn palette_execute(&mut self) {
        let Some(p) = &self.palette else {
            return;
        };
        let Some(&idx) = p.filtered.get(p.selected) else {
            return;
        };
        let item = &p.items[idx];
        let (cmd, needs_arg) = (item.command, item.needs_arg);
        self.palette = None;
        self.input = cmd.to_owned();
        self.cursor_pos = self.input.chars().count();
        if !needs_arg {
            self.submit();
        }
    }

    /// 弹窗按键：↑↓/j/k 移动、数字直选、Enter 确认、Esc 关闭。
    fn handle_picker_key(&mut self, key: KeyEvent) {
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

    /// 执行 provider 切换：解析 key、构建客户端、替换当前 provider。
    fn apply_provider_switch(&mut self, name: &str) {
        let Some(cfg) = self.all_providers.iter().find(|p| p.name == name).cloned() else {
            let names: Vec<&str> = self.all_providers.iter().map(|p| p.name.as_str()).collect();
            self.push_entry(Entry::Error(format!(
                "未知 provider `{name}`。可用: {}",
                names.join(", ")
            )));
            return;
        };
        match OpenAiClient::new(cfg.clone()) {
            Ok(client) => {
                self.provider = Arc::new(client);
                let old = self.provider_cfg.name.clone();
                self.provider_cfg = cfg;
                self.push_entry(Entry::Info(format!(
                    "已从 `{old}` 切换到 `{} │ {}`（思考模式: {}）",
                    self.provider_cfg.name,
                    self.provider_cfg.model,
                    if self.provider_cfg.thinking {
                        "开"
                    } else {
                        "关"
                    }
                )));
            }
            Err(e) => self.push_entry(Entry::Error(format!("切换失败: {e}"))),
        }
    }

    /// `/rename <标题>`：重命名当前会话；仅 Ready 状态可改。
    fn rename_session(&mut self, title: &str) {
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

    fn on_title_renamed(&mut self, ok: bool, title: String) {
        if ok {
            self.push_entry(Entry::Info(format!("会话已重命名为: {title}")));
            self.request_sessions_refresh();
        } else {
            self.push_entry(Entry::Error("重命名失败：会话不存在".into()));
        }
    }

    /// `/new`：丢弃当前上下文，开启新会话。
    fn start_new_session(&mut self) {
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
    fn open_session(&mut self, id: i64) {
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

    fn on_session_opened(&mut self, opened: Result<(i64, Option<i64>, Vec<Message>), String>) {
        match opened {
            Ok((id, course_id, msgs)) => {
                let count = msgs.len();
                self.history = msgs.clone();
                self.entries.clear();
                for m in &msgs {
                    match m.role {
                        agent_core::Role::System => {
                            if let Some(c) = &m.content {
                                self.entries.push(Entry::Info(c.clone()));
                            }
                        }
                        agent_core::Role::User => {
                            if let Some(c) = &m.content {
                                self.entries.push(Entry::User(c.clone()));
                            }
                        }
                        agent_core::Role::Assistant => {
                            self.entries.push(Entry::Assistant {
                                content: m.content.clone().unwrap_or_default(),
                                reasoning_chars: None,
                            });
                        }
                        agent_core::Role::Tool => {
                            self.entries.push(Entry::Info("[工具结果]".into()));
                        }
                    }
                }
                // R5：恢复会话时连课程分区一起还原
                if let Some(cid) = course_id {
                    if let Some(name) = self
                        .courses
                        .iter()
                        .find(|(id, _)| *id == cid)
                        .map(|(_, n)| n.clone())
                    {
                        self.course = name;
                    } else {
                        self.course = "all".into();
                    }
                } else {
                    self.course = "all".into();
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
    fn export_session(&mut self) {
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
    fn load_session_file(&mut self, path: &str) {
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
        self.entries.clear();
        for m in &msgs {
            match m.role {
                agent_core::Role::User => {
                    if let Some(c) = &m.content {
                        self.entries.push(Entry::User(c.clone()));
                    }
                }
                agent_core::Role::Assistant => {
                    self.entries.push(Entry::Assistant {
                        content: m.content.clone().unwrap_or_default(),
                        reasoning_chars: None,
                    });
                }
                _ => {}
            }
        }

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

    /// `/course`：`-list` 列出全部；`-new <名>` 新建并切换；`-delete <名>` 删除；
    /// `<课程|all>` 切换到已有分区。
    fn handle_course_command(&mut self, arg: &str) {
        let known: Vec<String> = self.courses.iter().map(|(_, n)| n.clone()).collect();
        match parse_course_action(arg, &known) {
            CourseAction::List => {
                let names: Vec<&str> = std::iter::once("all")
                    .chain(self.courses.iter().map(|(_, n)| n.as_str()))
                    .collect();
                self.push_entry(Entry::Info(format!(
                    "当前课程: {} │ 可用: {} │ 用法: /course <课程名|all> │ \
                     -new <名> 新建 │ -delete <名> 删除",
                    self.course,
                    names.join(", ")
                )));
            }
            CourseAction::Switch(name) => {
                self.course = name.clone();
                self.push_entry(Entry::Info(format!("已切换到课程: {name}")));
            }
            CourseAction::Create(name) => {
                let was_current = self.course == name;
                self.run_course_op(move |store| {
                    store
                        .get_or_create_course(&name)
                        .map_err(|e| e.to_string())?;
                    let list = store.list_courses().map_err(|e| e.to_string())?;
                    let verb = if list.iter().any(|(_, n)| n == &name) && was_current {
                        "已存在，切换到"
                    } else {
                        "已创建并切换到"
                    };
                    Ok((format!("{verb}课程: {name}"), list, Some(name)))
                });
            }
            CourseAction::Delete(name) => {
                let is_current = self.course == name;
                self.run_course_op(move |store| {
                    let deleted = store.delete_course(&name).map_err(|e| e.to_string())?;
                    if !deleted {
                        return Err(format!("课程 `{name}` 不存在"));
                    }
                    let list = store.list_courses().map_err(|e| e.to_string())?;
                    // 笔记/概念的 course_id 由外键 ON DELETE SET NULL 回落 all 区，数据不丢
                    let msg = format!("已删除课程: {name}（其笔记已回落 all 区）");
                    let switch_to = is_current.then(|| "all".to_owned());
                    Ok((msg, list, switch_to))
                });
            }
            CourseAction::Invalid(msg) => self.push_entry(Entry::Error(msg)),
        }
    }

    /// 课程管理操作的统一异步执行壳：DB 进 blocking 线程池（D2），
    /// 完成后经事件通道回填（消息 + 刷新列表 + 需切换的分区）。
    fn run_course_op<F>(&mut self, op: F)
    where
        F: FnOnce(&Store) -> CourseOpOutcome + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let outcome = spawn_blocking(move || op(&store))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let _ = tx.send(AppEvent::CourseManaged(outcome));
        });
    }

    fn on_course_managed(&mut self, outcome: CourseOpOutcome) {
        match outcome {
            Ok((msg, list, switch_to)) => {
                self.courses = list;
                if let Some(c) = switch_to {
                    self.course = c;
                }
                self.push_entry(Entry::Info(msg));
            }
            Err(e) => self.push_entry(Entry::Error(format!("课程操作失败: {e}"))),
        }
    }

    fn spawn_chat(&mut self) {
        let provider: Arc<dyn Provider> = self.provider.clone();
        let store = Arc::clone(&self.store);
        let course_id = self.current_course_id();
        let history = self.history.clone();
        let tx = self.tx.clone();
        let token = CancellationToken::new();
        let token_for_task = token.clone();

        let system_prompt = self.build_rag_system_prompt();
        let tools = ToolBox::new()
            .register(Box::new(SearchNotesTool::new(
                Arc::clone(&store),
                course_id,
            )))
            .register(Box::new(ListCoursesTool::new(store)));

        let agent = Agent::new(provider)
            .with_tools(tools)
            .with_system_prompt(system_prompt);

        tokio::spawn(async move {
            let _ = tx.send(AppEvent::Agent(AgentEvent::Thinking));
            tokio::select! {
                res = agent.run_messages(&history) => match res {
                    Ok(result) => {
                        let _ = tx.send(AppEvent::Agent(AgentEvent::Done {
                            content: result.content,
                            reasoning_chars: result.reasoning_chars,
                            usage: result.total_usage,
                            new_messages: result.new_messages,
                            tool_trace: result.tool_trace,
                        }));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Agent(AgentEvent::Failed(e.to_string())));
                    }
                },
                _ = token_for_task.cancelled() => {
                    let _ = tx.send(AppEvent::Agent(AgentEvent::Interrupted));
                }
            }
        });

        self.inflight = Some(token);
    }

    /// 当前课程分区对应的 course_id（all = None = 不过滤）。
    fn current_course_id(&self) -> Option<i64> {
        if self.course == "all" {
            None
        } else {
            self.courses
                .iter()
                .find(|(_, n)| *n == self.course)
                .map(|(id, _)| *id)
        }
    }
    /// RAG system prompt：AI Tutor 教学模式（非笔记搬运工）。
    fn build_rag_system_prompt(&self) -> String {
        let course_list: Vec<&str> = std::iter::once("all")
            .chain(self.courses.iter().map(|(_, n)| n.as_str()))
            .collect();
        format!(
            "你是一名课程学习导师。当前课程: {}。可切换: {}。\n\n\
             目标：帮助学生理解知识，而非总结笔记。\n\n\
             回答结构（严格遵循）：\n\
             ## 心智模型\n先建立一个直觉性的图景（如：变量--拥有-->数据）\n\n\
             ## 为什么需要\n问题背景：没有它会怎样？\n\n\
             ## 核心机制\n课程语境下的定义与关键规则\n\n\
             ## 示例\n一个最小代码例子，展示核心机制（选最有教学价值的，不要罗列）\n\n\
             ## 常见误区\n学生容易踩的坑\n\n\
             ## 相关知识\n关联概念，提示下一步学习方向\n\n\
             重要约束：\n\
             - 像老师一样重新讲解，绝不重复检索片段中的原句\n\
             - 覆盖 20% 的核心并讲清楚，胜过覆盖 100% 但像文档摘要\n\
             - 笔记优先：笔记中有相关内容时优先依据笔记讲解，并在关键事实后\
               标注一次 [n] 引用（[n] 只指向笔记片段）\n\
             - 笔记未覆盖的部分可以用你自己的知识补充，但必须在该部分末尾\
               明确标注「（笔记外补充）」，绝不能伪装成笔记内容\n\
             - 一个例子讲透核心思想，胜过五个浅例子\n\n\
             回答前调用一次 search_notes 检索相关笔记即可。",
            self.course,
            course_list.join(", ")
        )
    }

    pub fn handle_agent_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Thinking => {}
            AgentEvent::Done {
                content,
                reasoning_chars,
                usage,
                new_messages,
                tool_trace,
            } => {
                self.total_usage.prompt_tokens += usage.prompt_tokens;
                self.total_usage.completion_tokens += usage.completion_tokens;
                let delta = estimate_cost(&self.provider_cfg, &usage);
                self.total_cost += delta;
                self.session_cost += delta;
                for m in &new_messages {
                    self.history.push(m.clone());
                    self.persist(m.clone());
                }
                if !tool_trace.is_empty() {
                    let total_hits: usize = tool_trace.iter().map(|t| t.hit_count).sum();
                    let lines: Vec<String> = tool_trace
                        .iter()
                        .map(|t| {
                            format!(
                                "  {} {} → {} 条",
                                if t.ok { "✓" } else { "✗" },
                                t.tool_name,
                                t.hit_count
                            )
                        })
                        .collect();
                    self.push_entry(Entry::Info(format!(
                        "Agent trace ({total_hits} 条命中):\n{}",
                        lines.join("\n")
                    )));
                }
                self.push_entry(Entry::Assistant {
                    content,
                    reasoning_chars,
                });
                self.inflight = None;
                self.record_usage(usage);
            }
            AgentEvent::Failed(e) => {
                self.push_entry(Entry::Error(format!("请求失败: {e}")));
                self.inflight = None;
            }
            AgentEvent::Interrupted => {
                self.push_entry(Entry::Info("[已中断当前请求]".into()));
                self.inflight = None;
            }
        }
    }

    // ---- 会话持久化（R5）----

    /// 把消息交给会话状态机落库：无会话则先建，创建中则缓冲，就绪则直发单写泵。
    fn persist(&mut self, msg: Message) {
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

    fn send_to_saver(&mut self, msg: Message) {
        if let SessionState::Ready { saver, .. } = &self.session_state {
            let _ = saver.send(msg);
        }
    }

    /// 建会话失败：回退 Pending → None，告知用户哪些消息未持久化。
    /// 不回滚 history（对话体验不受影响），仅放弃落库。
    fn on_session_create_failed(&mut self, err: String) {
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
    fn on_session_ready(&mut self, id: i64) {
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
    fn spawn_saver_pump(&mut self, session_id: i64, mut rx: UnboundedReceiver<Message>) {
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

    fn enter_ready_session(&mut self, id: i64, buffered: Vec<Message>) {
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
    fn on_sessions_loaded(&mut self, list: Vec<storage::SessionMeta>) {
        self.sidebar_sessions = list;
        if self.pending_sessions_print {
            self.pending_sessions_print = false;
            self.print_session_list();
        }
    }

    fn print_session_list(&mut self) {
        let lines: Vec<String> = self
            .sidebar_sessions
            .iter()
            .map(|s| format!("  #{} {}", s.id, s.title.as_deref().unwrap_or("(未命名)")))
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

    /// 从 usage_log 对账累计成本（R6）：review/outline/import/grade 的花费
    /// 不经过 chat 的 Done 事件路径，靠读取 DB 补齐熔断口径。
    /// 取 max 防止对账时该笔 usage 尚未落库导致回退（成本单调递增）。
    fn request_cost_sync(&self) {
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let total = spawn_blocking(move || store.total_recorded_cost())
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            if let Ok(v) = total {
                let _ = tx.send(AppEvent::CostSynced(v));
            }
        });
    }

    /// 后台任务里的 usage 落库统一壳（chat 以外各 LLM 命令共用）。
    fn log_llm_usage(store: &Arc<Store>, pc: &ProviderConfig, kind: &str, u: &Usage) {
        let store = Arc::clone(store);
        let pc = pc.clone();
        let kind = kind.to_owned();
        let (prompt, completion) = (u.prompt_tokens, u.completion_tokens);
        let kind_ref = kind.to_owned();
        tokio::spawn(async move {
            let cost = estimate_cost(
                &pc,
                &Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                },
            );
            let result = spawn_blocking(move || {
                store.append_usage(&pc.name, &pc.model, &kind, prompt, completion, cost)
            })
            .await;
            // kind 在闭包内被消耗，日志改用模型名定位
            if let Err(e) = result {
                tracing::warn!("{kind_ref} usage_log 写入失败: {e}");
            }
        });
    }

    /// 侧栏会话列表刷新。
    fn request_sessions_refresh(&mut self) {
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

    fn record_usage(&self, usage: Usage) {
        let pc = self.provider_cfg.clone();
        let cost = estimate_cost(&pc, &usage);
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            let result = spawn_blocking(move || {
                store.append_usage(
                    &pc.name,
                    &pc.model,
                    "chat",
                    usage.prompt_tokens,
                    usage.completion_tokens,
                    cost,
                )
            })
            .await;
            if let Err(e) = result {
                tracing::warn!("usage_log 写入失败: {e}");
            }
        });
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

/// 复制文本到系统剪贴板。
/// 优先 arboard（原生 X11/Wayland），fd 1/2 重定向到 /dev/null 防止污染 TUI；
/// 失败则回退命令行工具（stdout/stderr 已 null）。
/// 全程 catch_unwind：内部任何 panic 转为 Err 并记日志，绝不杀死 TUI。
fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // arboard 优先：重定向 fd 1/2 → /dev/null，防止任何输出污染 TUI
        // 安全性：此函数在 spawn_blocking 线程执行且被 await，主线程不会 draw
        with_silent_stdout(|| {
            let mut cb = arboard::Clipboard::new().map_err(|e| format!("arboard init: {e}"))?;
            cb.set_text(text.to_owned())
                .map_err(|e| format!("arboard set: {e}"))
        })
    }));

    let arboard_result = match result {
        Ok(r) => r,
        Err(panic_payload) => {
            let msg = panic_payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            tracing::error!("剪贴板 arboard panic: {msg}");
            return Err(format!("剪贴板内部错误: {msg}"));
        }
    };
    if arboard_result.is_ok() {
        return Ok(());
    }

    // 回退：命令行工具（stdout/stderr → /dev/null）
    use std::io::Write;
    use std::process::{Command, Stdio};
    let candidates: [(&str, Vec<&str>); 4] = [
        ("xclip", vec!["-selection", "clipboard"]),
        ("wl-copy", vec![]),
        ("pbcopy", vec![]),
        ("clip", vec![]),
    ];
    for (cmd, args) in &candidates {
        let r = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(text.as_bytes())?;
                }
                child.wait()
            });
        match r {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("启动 {cmd} 失败: {e}")),
        }
    }
    Err("无可用剪贴板（arboard 初始化失败且未安装 xclip/wl-copy）".into())
}

/// 在 fd 1/2 重定向到 /dev/null 的环境下执行闭包。
/// 异常安全：闭包 panic 时也保证 fd 恢复（否则后续 draw 永远输出到 /dev/null），
/// panic 再向外传播由上层 catch_unwind 兜底。
fn with_silent_stdout<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    unsafe {
        let saved_out = libc::dup(1);
        let saved_err = libc::dup(2);
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if devnull >= 0 {
            libc::dup2(devnull, 1);
            libc::dup2(devnull, 2);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if saved_out >= 0 {
            libc::dup2(saved_out, 1);
            libc::close(saved_out);
        }
        if saved_err >= 0 {
            libc::dup2(saved_err, 2);
            libc::close(saved_err);
        }
        if devnull >= 0 {
            libc::close(devnull);
        }
        match result {
            Ok(r) => r,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// 渲染缩进树 + 末尾脚注列表。
fn render_outline_tree(outline: &serde_json::Value, titles: &[(i64, String)]) -> Vec<String> {
    let mut lines = Vec::new();
    let empty: Vec<serde_json::Value> = Vec::new();
    let sections = outline
        .get("sections")
        .and_then(|s| s.as_array())
        .unwrap_or(&empty);

    let mut all_refs: Vec<usize> = Vec::new();

    for section in sections {
        let title = section
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("(未命名)");
        lines.push(format!("├ {title}"));

        if let Some(points) = section.get("points").and_then(|p| p.as_array()) {
            for point in points {
                let p = point.as_str().unwrap_or("");
                lines.push(format!("│  ├ {p}"));
            }
        }

        // refs 兼容 int 和 string 两种形式
        let refs: Vec<usize> = section
            .get("refs")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        r.as_u64().map(|n| n as usize).or_else(|| {
                            r.as_str().and_then(|s| {
                                s.trim()
                                    .trim_start_matches('[')
                                    .trim_end_matches(']')
                                    .parse()
                                    .ok()
                            })
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        if !refs.is_empty() {
            let ref_str: Vec<String> = refs.iter().map(|n| n.to_string()).collect();
            lines.push(format!("│  └ 引用: [{}]", ref_str.join("] [")));
            all_refs.extend(refs.iter());
        }
    }

    // 末尾脚注：列出每个引用编号对应的笔记标题
    if !all_refs.is_empty() {
        lines.push(String::new());
        lines.push("引用来源:".into());
        for n in all_refs
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
        {
            let title = titles
                .get(n.saturating_sub(1))
                .map(|(_, t)| t.as_str())
                .unwrap_or("未知");
            lines.push(format!("  [{n}] {title}"));
        }
    }

    lines
}

/// 渲染 Markdown + 末尾脚注。
fn render_outline_markdown(
    outline: &serde_json::Value,
    course: &str,
    titles: &[(i64, String)],
) -> String {
    let mut md = format!("# {course} 课程大纲\n\n");
    let empty: Vec<serde_json::Value> = Vec::new();
    let sections = outline
        .get("sections")
        .and_then(|s| s.as_array())
        .unwrap_or(&empty);

    let mut all_refs: Vec<usize> = Vec::new();

    for section in sections {
        let title = section
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("(未命名)");
        md.push_str(&format!("## {title}\n"));

        if let Some(points) = section.get("points").and_then(|p| p.as_array()) {
            for point in points {
                md.push_str(&format!("- {}\n", point.as_str().unwrap_or("")));
            }
        }

        let refs: Vec<usize> = section
            .get("refs")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        r.as_u64().map(|n| n as usize).or_else(|| {
                            r.as_str().and_then(|s| {
                                s.trim()
                                    .trim_start_matches('[')
                                    .trim_end_matches(']')
                                    .parse()
                                    .ok()
                            })
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        if !refs.is_empty() {
            let ref_str: Vec<String> = refs.iter().map(|n| format!("[{n}]")).collect();
            md.push_str(&format!("\n> 参考: {}\n", ref_str.join(" ")));
            all_refs.extend(refs.iter());
        }
        md.push('\n');
    }

    // 末尾脚注
    if !all_refs.is_empty() {
        md.push_str("## 引用来源\n\n");
        for n in all_refs
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
        {
            let title = titles
                .get(n.saturating_sub(1))
                .map(|(_, t)| t.as_str())
                .unwrap_or("未知");
            md.push_str(&format!("- [{n}] {title}\n"));
        }
    }

    md
}
mod tests {
    #[allow(unused_imports)]
    use super::{byte_offset, delete_at, delete_before, insert_char};

    #[test]
    fn byte_offset_ascii_and_cjk() {
        let s = "ab中文cd"; // a b 中 文 c d → 6 字符
        assert_eq!(byte_offset(s, 0), 0);
        assert_eq!(byte_offset(s, 2), 2); // "中" 起点
        assert_eq!(byte_offset(s, 3), 5); // "文" 起点（中占 3 字节）
        assert_eq!(byte_offset(s, 6), 10); // 末尾（总长 1+1+3+3+1+1=10）
        assert_eq!(byte_offset(s, 99), 10); // 越界钳到串长
        assert_eq!(byte_offset("", 0), 0);
    }

    #[test]
    fn insert_char_at_head_middle_tail() {
        let mut s = String::from("ac");
        assert_eq!(insert_char(&mut s, 1, 'b'), 2);
        assert_eq!(s, "abc");

        // 中文插入不破坏 UTF-8
        let mut s = String::from("内存");
        assert_eq!(insert_char(&mut s, 1, '安'), 2);
        assert_eq!(s, "内安存");

        // 头部 / 尾部 / 越界钳制
        let mut s = String::from("bc");
        assert_eq!(insert_char(&mut s, 0, 'a'), 1);
        assert_eq!(s, "abc");
        assert_eq!(insert_char(&mut s, 99, 'd'), 4);
        assert_eq!(s, "abcd");
    }

    #[test]
    fn delete_before_various_positions() {
        // 行首不动
        let mut s = String::from("abc");
        assert_eq!(delete_before(&mut s, 0), 0);
        assert_eq!(s, "abc");

        // 中间删除
        assert_eq!(delete_before(&mut s, 2), 1);
        assert_eq!(s, "ac");

        // 删除中文字符（多字节整体删除，不留残字节）
        let mut s = String::from("内存");
        assert_eq!(delete_before(&mut s, 2), 1);
        assert_eq!(s, "内");

        // 空串 / 越界安全
        let mut s = String::new();
        assert_eq!(delete_before(&mut s, 0), 0);
        assert_eq!(delete_before(&mut s, 5), 0);
    }

    #[test]
    fn delete_at_keeps_cursor() {
        let mut s = String::from("abc");
        delete_at(&mut s, 1);
        assert_eq!(s, "ac");

        // 删除中文
        let mut s = String::from("内存好");
        delete_at(&mut s, 1);
        assert_eq!(s, "内好");

        // 末尾越界不动
        delete_at(&mut s, 2);
        assert_eq!(s, "内好");
        delete_at(&mut s, 99);
        assert_eq!(s, "内好");
    }
}
