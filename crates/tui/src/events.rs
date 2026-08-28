//! 事件类型：后台任务 → TUI 的统一通道载体。

use agent_core::{LoopEvent, Message, Usage};
use crossterm::event::Event as CtEvent;

use crate::review;

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
    /// 笔记浏览器异步搜索结果（seq 丢弃过期响应）
    BrowserResults {
        seq: u64,
        result: Result<Vec<storage::NoteSummary>, String>,
    },
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
    /// 课程列表+统计刷新完成（导入可能新建课程；删除/移动笔记改变统计）
    CoursesRefreshed(
        Vec<(i64, String)>,
        std::collections::HashMap<i64, (usize, usize)>,
    ),
    /// 浏览器批量动作完成：(范围名, 结果消息)
    BrowserActionDone {
        scope_label: String,
        result: Result<String, String>,
    },
    /// agent loop 过程事件（工具活动流：◌ 进行中 / ✓ 完成 / ✗ 失败）
    AgentActivity(LoopEvent),
    /// 会话浏览器异步搜索结果（seq 丢弃过期响应）
    SessionBrowserResults {
        seq: u64,
        result: Result<Vec<storage::SessionMeta>, String>,
    },
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
    /// RAG 来源脚注行（"[n] 标题 · 小节"，Reference 青色渲染）
    Citation(String),
    /// 工具活动行：◌ 进行中（蓝紫） / ✓ 成功（绿） / ✗ 失败（红）
    Tool {
        text: String,
        ok: Option<bool>,
    },
    Error(String),
}
