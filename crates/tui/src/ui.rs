//! ratatui 绘制：状态栏 + 课程侧栏 + 聊天流 + 输入框。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::app::{App, Entry, ModelPicker};
use crate::markdown;
use crate::palette::ListPicker;

use crate::theme;

// 语义别名（既有代码引用点不动，值全部来自 theme）
const ACCENT: Color = theme::PRIMARY; // 选中/结构/应用名
const DIM: Color = theme::MUTED; // metadata/提示
const USER: Color = theme::USER; // 用户输入/Review
const ERROR: Color = theme::ERROR; // 错误
const SUCCESS: Color = theme::SUCCESS; // Ready/✓/成功

/// Braille spinner 字符（8 帧）。
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
fn spinner_char(tick: usize) -> &'static str {
    SPINNER[tick % SPINNER.len()]
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let root = f.area();
    // 输入区高度：随输入折行数动态增长（长答案/长问题自动换行不被截断）。
    // 简答题作答中加高（答案框主视觉）；反馈停留态收窄回 3。
    let input_w = (root.width.saturating_sub(2)).max(4) as usize;
    let wrapped_lines = wrap(&app.input, input_w).len();
    let review_short = app.review.as_ref().is_some_and(|rs| {
        !rs.awaiting_feedback()
            && rs
                .questions
                .get(rs.current)
                .map(|q| q.q_type == crate::review::QType::ShortAnswer)
                .unwrap_or(false)
    });
    let floor = if review_short { 6 } else { 3 };
    let input_h = (wrapped_lines + 1).clamp(floor, 8) as u16;
    let [header, main_area, input_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(input_h),
    ])
    .areas(root);

    draw_header(f, header, app);
    if app.workspace == crate::app::Workspace::Home {
        // Home = 独立 Launchpad：全宽主区，不显示课程侧栏
        draw_home(f, main_area, app);
    } else {
        let [sidebar, chat] =
            Layout::horizontal([Constraint::Length(28), Constraint::Min(20)]).areas(main_area);
        draw_sidebar(f, sidebar, app);
        match app.workspace {
            crate::app::Workspace::Course => draw_course(f, chat, app),
            crate::app::Workspace::Session => {
                if app.review.is_some() {
                    draw_review_workspace(f, chat, app);
                } else {
                    draw_chat(f, chat, app);
                }
            }
            crate::app::Workspace::Home => unreachable!(),
        }
    }
    draw_input(f, input_area, app);

    if let Some(picker) = &app.model_picker {
        draw_model_picker(f, picker, &app.provider_cfg.name);
    }
    if app.palette.is_some() {
        draw_command_palette(f, app);
    }
    if let Some(lp) = &app.list_picker {
        draw_list_picker(f, lp, &app.input);
    }
    if app.wizard.is_some() {
        draw_wizard(f, app);
    }
    if app.note_browser.is_some() {
        draw_note_browser(f, app);
    }
    if app.session_browser.is_some() {
        draw_session_browser(f, app);
    }
    if app.setup.is_some() {
        draw_setup(f, app);
    }
    if app.toast.is_some() {
        draw_toast(f, app);
    }
}

/// Home workspace（Launchpad）：Continue learning / Your courses / + New Course。
/// 全宽主区、无课程侧栏、无聊天/输入框——只回答"我接下来要做什么"。
fn draw_home(f: &mut Frame, area: Rect, app: &mut App) {
    use crate::app::Workspace;
    if app.workspace != Workspace::Home {
        return;
    }
    // 居中偏上：内容列水平居中（最大 76 列，避免文字无限拉开），垂直留白半屏
    let col_w = (area.width * 3 / 4).clamp(20, 76);
    let x = area.x + area.width.saturating_sub(col_w) / 2;
    let content = Rect {
        x,
        y: area.y,
        width: col_w,
        height: area.height,
    };
    f.render_widget(Paragraph::new(home_lines(app)), content);
}

/// 无意义 session 标题 → "Recent conversation"（首条测试消息如「你好」不当首页主标题）。
/// 规则：空 / <3 字符 / 常见问候与测试词。纯展示 fallback，不改 DB。
fn session_display_title(title: Option<&str>) -> String {
    const MEANINGLESS: &[&str] = &[
        "你好",
        "hello",
        "hi",
        "hey",
        "test",
        "ok",
        "okay",
        "hi there",
        "hello world",
        "testing",
        "asdf",
        "qwerty",
        "abc",
        "1",
        "123",
    ];
    let t = title.unwrap_or("").trim();
    if t.is_empty() || t.chars().count() < 3 {
        return "Recent conversation".into();
    }
    if MEANINGLESS.contains(&t.to_ascii_lowercase().as_str()) {
        return "Recent conversation".into();
    }
    t.to_owned()
}

/// Home 视图内容行（纯函数，供 draw + 单测/预览复用）。
/// 三级层级：PRIMARY Continue learning → SECONDARY Your courses → TERTIARY + New Course。
/// 无重复产品名（header 已有）；选中指示用轻量 ●；不暴露 CLI 命令。
fn home_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = vec![Line::default(), Line::default(), Line::default()];
    // 顶部留白（垂直重心偏上，不用真居中）

    // AI 待配置引导（文档 §二/§九：无有效配置时 Set up AI 为首个可聚焦项）。
    // 索引语义与 home_cursor_count/home_activate 一致：Setup 恒占位 0，其余内容顺延。
    let setup_offset = if app.ai_needs_setup() { 1 } else { 0 };
    if app.ai_needs_setup() {
        let selected = app.home_cursor == 0;
        lines.push(Line::from(vec![
            Span::styled(if selected { "▶ " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled(
                "Set up AI — choose a provider and model to start",
                if selected {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme::FG)
                },
            ),
        ]));
        lines.push(Line::from(Span::styled(
            "   Enter to configure · then start learning",
            Style::new().fg(DIM),
        )));
        lines.push(home_divider());
        lines.push(Line::default());
    }

    if app.courses.is_empty() {
        // 新用户：Welcome + 创建入口（不暴露 CLI 语法）
        lines.push(Line::from(Span::styled(
            "Welcome.",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Learn from your own materials.",
            Style::new().fg(theme::FG),
        )));
        lines.push(Line::from(Span::styled(
            "Create a course to get started.",
            Style::new().fg(DIM),
        )));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  1  Create a course",
            Style::new().fg(DIM),
        )));
        lines.push(Line::from(Span::styled(
            "  2  Import your materials",
            Style::new().fg(DIM),
        )));
        lines.push(Line::from(Span::styled(
            "  3  Ask questions",
            Style::new().fg(DIM),
        )));
        lines.push(Line::from(Span::styled(
            "  4  Review what you've learned",
            Style::new().fg(DIM),
        )));
        lines.push(Line::default());
        lines.push(Line::default());
        lines.push(button_line(
            "[ + New Course ]",
            app.home_cursor == setup_offset,
        ));
        return lines;
    }

    // ── PRIMARY：Continue learning（首页第一视觉焦点）──
    let mut idx = setup_offset;
    if let Some((_, course, title)) = app.continue_session() {
        let title = session_display_title(Some(&title));
        let selected = app.home_cursor == idx;
        lines.push(Line::from(Span::styled(
            "Continue learning",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.push(home_divider());
        lines.push(Line::default());
        // 课程名 + 会话标题分行（mockup：Rust / Ownership & Borrowing）
        lines.push(Line::from(vec![
            Span::styled(if selected { "● " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled(
                course,
                if selected {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme::FG)
                },
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  {title}"),
            Style::new().fg(theme::FG),
        )));
        lines.push(Line::from(Span::styled(
            format!("  {}", last_studied(app)),
            Style::new().fg(DIM),
        )));
        lines.push(Line::default());
        lines.push(button_line("[ Continue ]", selected));
        lines.push(Line::default());
        lines.push(home_divider());
        lines.push(Line::default());
        idx += 1;
    } else {
        // 有课程但无 session：轻量引导（无空 Continue 区）
        lines.push(Line::from(Span::styled(
            "Start learning",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.push(home_divider());
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Choose a course to begin.",
            Style::new().fg(theme::FG),
        )));
        lines.push(Line::default());
        lines.push(home_divider());
        lines.push(Line::default());
    }

    // ── SECONDARY：Your courses（课程名 = 主要信息，统计 = secondary）──
    lines.push(Line::from(Span::styled(
        "Your courses",
        Style::new().fg(theme::MUTED).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());
    for (cid, name) in &app.courses {
        let selected = app.home_cursor == idx;
        let mark = if selected { "● " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(mark, Style::new().fg(ACCENT)),
            Span::styled(
                name.clone(),
                if selected {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme::FG)
                },
            ),
        ]));
        if let Some((n, c)) = app.sidebar_course_stats.get(cid) {
            lines.push(Line::from(Span::styled(
                format!("   {n} notes · {c} concepts"),
                Style::new().fg(DIM),
            )));
        }
        lines.push(Line::default());
        idx += 1;
    }

    // ── TERTIARY：New Course ──
    lines.push(button_line("[ + New Course ]", app.home_cursor == idx));

    lines
}

/// Home 区块分隔线（mockup 的 `──────`）。
fn home_divider() -> Line<'static> {
    Line::from(Span::styled(
        "────────────────────────────",
        Style::new().fg(DIM),
    ))
}

/// 括弧按钮行（mockup 的 `[ Continue ]` / `[ + New Course ]`）。
fn button_line(label: &'static str, selected: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled("  ", Style::new().fg(DIM)),
        Span::styled(
            label,
            if selected {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme::MUTED)
            },
        ),
    ])
}

/// "Last studied …" 相对时间（sessions.created_at，SQLite UTC datetime）。
/// 解析失败或会话过旧时降级为简要文案。
fn last_studied(app: &App) -> String {
    if app.continue_session().is_none() {
        return String::new();
    }
    let Some(ts) = app.sidebar_sessions.first().map(|s| s.created_at.clone()) else {
        return "Last studied recently".into();
    };
    // "YYYY-MM-DD HH:MM:SS" → epoch
    let Some(d) = ts.get(..10) else {
        return "Last studied recently".into();
    };
    let Some(t) = ts.get(11..19) else {
        return "Last studied recently".into();
    };
    let Ok(y) = d[0..4].parse::<i64>() else {
        return "Last studied recently".into();
    };
    let Ok(mo) = d[5..7].parse::<u32>() else {
        return "Last studied recently".into();
    };
    let Ok(day) = d[8..10].parse::<u32>() else {
        return "Last studied recently".into();
    };
    let Ok(h) = t[0..2].parse::<u32>() else {
        return "Last studied recently".into();
    };
    let Ok(mi) = t[3..5].parse::<u32>() else {
        return "Last studied recently".into();
    };
    let Ok(sec) = t[6..8].parse::<u32>() else {
        return "Last studied recently".into();
    };
    let days_in = |mo: u32| -> u32 {
        match mo {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => 28, // 近似，TUI 首页不必精确闰年
            _ => 30,
        }
    };
    let ts = y * 365 * 86400
        + (0..mo).map(days_in).sum::<u32>() as i64 * 86400
        + day as i64 * 86400
        + h as i64 * 3600
        + mi as i64 * 60
        + sec as i64;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = (now - ts).max(0);
    if diff < 60 {
        "Last studied just now".into()
    } else if diff < 3600 {
        format!("Last studied {} min ago", diff / 60)
    } else if diff < 86400 {
        format!("Last studied {} hr ago", diff / 3600)
    } else {
        format!("Last studied {} d ago", diff / 86400)
    }
}

/// Course workspace（课程上下文）：统计 / Continue / Knowledge / Materials / Recent learning。
/// 决定"这门课里做什么"，再进入 Session 执行。
fn draw_course(f: &mut Frame, area: Rect, app: &mut App) {
    use crate::app::Workspace;
    if app.workspace != Workspace::Course {
        return;
    }
    let course_id = app.current_course_id();
    let course_name = app.course.clone();
    let stats = course_id
        .and_then(|id| app.sidebar_course_stats.get(&id))
        .copied()
        .unwrap_or((0, 0));
    let weak = course_id
        .and_then(|id| app.weak_stats.get(&id))
        .copied()
        .unwrap_or(0);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "← Back to Home",
        Style::new().fg(DIM),
    )));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        course_name.clone(),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "{} notes · {} concepts · {} need reinforcement",
            stats.0, stats.1, weak
        ),
        Style::new().fg(DIM),
    )));
    lines.push(Line::default());

    let mut idx = 0usize;
    // 每个动作行：● 当前项 / 空格其他；选中项 accent 加粗（与 Home 同款轻量指示）
    let push_action = |lines: &mut Vec<Line<'static>>, idx: usize, label: &str, sub: &str| {
        let selected = app.course_cursor == idx;
        let marker = if selected { "● " } else { "  " };
        let style = if selected {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::FG)
        };
        let sub_span = if sub.is_empty() {
            String::new()
        } else {
            format!("   {sub}")
        };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::new().fg(ACCENT)),
            Span::styled(label.to_string(), style),
            Span::styled(sub_span, Style::new().fg(DIM)),
        ]));
    };

    // Continue learning（本课程最近 session）
    if let Some(s) = app
        .sidebar_sessions
        .iter()
        .find(|s| s.course_id == course_id)
    {
        push_action(
            &mut lines,
            idx,
            "Continue learning",
            s.title.as_deref().unwrap_or("(未命名)"),
        );
        idx += 1;
    }
    push_action(&mut lines, idx, "New conversation", "");
    idx += 1;
    push_action(&mut lines, idx, "Knowledge · Review Map", "/review-map");
    idx += 1;
    push_action(&mut lines, idx, "Knowledge · Outline", "/outline");
    idx += 1;
    push_action(&mut lines, idx, "Materials · Import", "/import");
    idx += 1;

    // Recent learning（本课程 session）
    let recent: Vec<&storage::SessionMeta> = app
        .sidebar_sessions
        .iter()
        .filter(|s| s.course_id == course_id)
        .take(3)
        .collect();
    if !recent.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Recent learning",
            Style::new().fg(DIM),
        )));
        for s in recent {
            push_action(
                &mut lines,
                idx,
                s.title.as_deref().unwrap_or("(未命名)"),
                "",
            );
            idx += 1;
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "↑↓ select · Enter open · Esc back to Home",
        Style::new().fg(DIM),
    )));
    f.render_widget(Paragraph::new(lines), area);
}

/// Review workspace：进度点 → 标签 → 题干 → 选项/反馈 → Sources。
/// 交互状态语义：蓝=可选项、黄=当前/题目、绿=正确、红=错误、灰=非重点。
fn draw_review_workspace(f: &mut Frame, area: Rect, app: &mut App) {
    use crate::review::QType;
    let Some(rs) = app.review.as_ref() else {
        return;
    };
    let inner_w = area.width.saturating_sub(4) as usize;
    let awaiting = rs.awaiting_feedback();
    let total = rs.planned;

    // 进度圆点（总槽位 = planned；未生成的槽位画 ○）
    let mut dots: Vec<Span<'static>> = Vec::new();
    for r in rs.results.iter() {
        dots.push(Span::styled(
            if r.correct { "✓" } else { "✗" },
            Style::new().fg(if r.correct { theme::SUCCESS } else { ERROR }),
        ));
        dots.push(Span::raw(" "));
    }
    for i in rs.results.len()..total {
        dots.push(Span::styled(
            if i == rs.current && !awaiting {
                "●"
            } else {
                "○"
            },
            Style::new().fg(if i == rs.current && !awaiting {
                theme::USER
            } else {
                theme::MUTED
            }),
        ));
        if i + 1 < total {
            dots.push(Span::raw(" "));
        }
    }

    // 逐题生成等待态：下一题尚未到位
    let Some(q) = rs.questions.get(rs.current) else {
        let mut body: Vec<Line<'static>> = Vec::new();
        let mut dots_line = vec![Span::raw("  ")];
        dots_line.extend(dots);
        body.push(Line::from(dots_line));
        body.push(Line::default());
        body.push(Line::from(vec![
            Span::styled(
                format!("Question {}/{}", rs.current + 1, total),
                Style::new().fg(theme::USER).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  出题中…", Style::new().fg(theme::MUTED)),
        ]));
        body.push(Line::default());
        body.push(Line::from(Span::styled(
            "  下一题正在后台生成（你作答时即已开始）…",
            Style::new().fg(theme::MUTED),
        )));
        body.push(Line::from(Span::styled(
            "  Esc 退出复习",
            Style::new().fg(theme::MUTED),
        )));
        let para = Paragraph::new(body);
        f.render_widget(para, area);
        return;
    };

    // ② 标签行
    let type_str = if q.q_type == QType::Choice {
        "选择题"
    } else {
        "简答题"
    };
    let tags = vec![
        Span::styled(
            format!("Question {}/{}", rs.current + 1, total),
            Style::new().fg(theme::USER).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::new().fg(theme::MUTED)),
        Span::styled(type_str, Style::new().fg(theme::SECONDARY)),
    ];
    // ③ 题干
    let mut body: Vec<Line<'static>> = Vec::new();
    let mut dots_line = vec![Span::raw("  ")];
    dots_line.extend(dots);
    body.push(Line::from(dots_line));
    body.push(Line::default());
    body.push(Line::from(tags));
    body.push(Line::default());
    // 题干走 markdown 管线（与聊天流一致：行内代码/代码块/加粗都有样式）
    let mut q_lines = markdown::render_markdown(&q.question, inner_w.saturating_sub(2).max(1));
    // render 段落结束自带空行，去掉尾部空行避免与下方间隔叠加
    while q_lines.last().is_some_and(|l| l.spans.is_empty()) {
        q_lines.pop();
    }
    for l in q_lines {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(l.spans);
        body.push(Line::from(spans));
    }
    body.push(Line::default());

    if awaiting {
        // ④a 选择题选项正误染色：✓ 正确答案绿 · ✗ 用户误选红 · 其余灰
        if q.q_type == QType::Choice
            && let Some(r) = rs.results.get(rs.current)
        {
            let correct_idx = q
                .answer
                .filter(|&a| a >= 0 && (a as usize) < q.options.len())
                .map(|a| a as usize);
            for (i, opt) in q.options.iter().enumerate() {
                let is_correct = correct_idx == Some(i);
                let is_pick = r.user_choice == Some(i);
                let (mark, color) = if is_correct {
                    ("✓ ", theme::SUCCESS)
                } else if is_pick {
                    ("✗ ", ERROR)
                } else {
                    ("  ", theme::MUTED)
                };
                let letter = (b'A' + i as u8) as char;
                // 选项文本走 markdown 管线（行内代码样式化，不再显示反引号）
                let mut first = true;
                for l in markdown::render_markdown(opt, inner_w.saturating_sub(8).max(1)) {
                    if l.spans.is_empty() {
                        continue;
                    }
                    let mut spans = vec![
                        Span::raw("  "),
                        Span::styled(
                            if first {
                                format!("{mark}{letter} ")
                            } else {
                                "        ".to_owned()
                            },
                            Style::new().fg(color),
                        ),
                    ];
                    spans.extend(l.spans);
                    body.push(Line::from(spans));
                    first = false;
                }
            }
            body.push(Line::default());
        }
        // ④b 反馈卡
        if let Some(r) = rs.results.get(rs.current) {
            let mark = if r.correct {
                "✓ Correct"
            } else {
                "✗ Incorrect"
            };
            let color = if r.correct { theme::SUCCESS } else { ERROR };
            // feedback 自带 ✓/✗ 前缀时去掉，避免与 mark 重复
            let detail = r
                .feedback
                .strip_prefix("✓ ")
                .or_else(|| r.feedback.strip_prefix("✗ "))
                .unwrap_or(&r.feedback);
            body.push(Line::from(Span::styled(
                format!("  {mark}  {detail}"),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            )));
            if let Some(s) = r.score {
                body.push(Line::from(Span::styled(
                    format!("  得分 {s}/100"),
                    Style::new().fg(color),
                )));
            }
            if let Some(exp) = &q.explanation {
                // 解析同样走 markdown 管线（与题干一致，可能含代码）
                // 宽度按前缀 8 列修正（"  解析: " 2+4+2）——此前只减 2，行超宽末字被裁
                let mut exp_lines =
                    markdown::render_markdown(exp, inner_w.saturating_sub(8).max(1));
                while exp_lines.last().is_some_and(|l| l.spans.is_empty()) {
                    exp_lines.pop();
                }
                for (i, l) in exp_lines.into_iter().enumerate() {
                    let mut spans = vec![Span::styled(
                        if i == 0 { "  解析: " } else { "        " },
                        Style::new().fg(theme::MUTED),
                    )];
                    spans.extend(l.spans);
                    body.push(Line::from(spans));
                }
            }
            if !r.missing.is_empty() {
                body.push(Line::from(Span::styled(
                    "  缺失要点：",
                    Style::new().fg(theme::USER),
                )));
                for m in &r.missing {
                    body.push(Line::from(Span::styled(
                        format!("  ● {m}"),
                        Style::new().fg(theme::SUCCESS),
                    )));
                }
            }
        }
        // ④c 追问对话（当前题，切题清空）：问句 USER 黄，回答走 markdown 管线
        if !rs.followups.is_empty() {
            body.push(Line::default());
            for turn in &rs.followups {
                body.push(Line::from(Span::styled(
                    format!("  你 › {}", turn.question),
                    Style::new().fg(theme::USER),
                )));
                match turn.answer.as_ref() {
                    None => {
                        // 回答生成中：占位思考行（问题 5：问句先显示）
                        body.push(Line::from(Span::styled(
                            format!("  {} 正在生成回答…", spinner_char(app.tick)),
                            Style::new().fg(DIM),
                        )));
                    }
                    Some(Ok(a)) => {
                        let mut ans_lines =
                            markdown::render_markdown(a, inner_w.saturating_sub(2).max(1));
                        while ans_lines.last().is_some_and(|l| l.spans.is_empty()) {
                            ans_lines.pop();
                        }
                        for l in ans_lines {
                            let mut spans = vec![Span::raw("  ")];
                            spans.extend(l.spans);
                            body.push(Line::from(spans));
                        }
                        // 引用脚注：[n] → 笔记来源（此前只有编号没有来源）
                        if !turn.citations.is_empty() {
                            body.push(Line::from(Span::styled(
                                "  ── 引用来源 ──",
                                Style::new().fg(theme::MUTED),
                            )));
                            for (n, src) in &turn.citations {
                                body.push(Line::from(Span::styled(
                                    format!("  [{n}] {src}"),
                                    Style::new().fg(theme::MUTED),
                                )));
                            }
                        }
                    }
                    Some(Err(e)) => {
                        body.push(Line::from(Span::styled(
                            format!("  ⚠ {e}"),
                            Style::new().fg(ERROR),
                        )));
                    }
                }
                body.push(Line::default());
            }
        }
        body.push(Line::default());
        body.push(Line::from(Span::styled(
            "  空 Enter 下一题 · 输入可追问 · Esc 退出复习",
            Style::new().fg(theme::MUTED),
        )));
    } else if q.q_type == QType::Choice {
        // ⑤ 选项列表：字母蓝、文字浅灰、当前 ❯ 黄（行内代码样式化——`..x` 不再显示反引号）
        for (i, opt) in q.options.iter().enumerate() {
            let letter = (b'A' + i as u8) as char;
            let is_cursor = rs.selected_option.unwrap_or(0) == i;
            let mark = if is_cursor { "❯ " } else { "  " };
            let letter_style = Style::new().fg(if is_cursor { theme::USER } else { ACCENT });
            let mut first = true;
            for l in markdown::render_markdown(opt, inner_w.saturating_sub(8).max(1)) {
                if l.spans.is_empty() {
                    continue;
                }
                let mut spans = vec![
                    Span::styled(
                        format!("  {mark}"),
                        Style::new().fg(if is_cursor { theme::USER } else { theme::MUTED }),
                    ),
                    Span::styled(
                        if first {
                            format!("{letter}  ")
                        } else {
                            "      ".to_owned()
                        },
                        letter_style,
                    ),
                ];
                spans.extend(l.spans);
                body.push(Line::from(spans));
                first = false;
            }
        }
        body.push(Line::default());
        body.push(Line::from(Span::styled(
            "  ↑↓ 选择 · A-D 作答 · Esc 退出复习",
            Style::new().fg(theme::MUTED),
        )));
    } else {
        // 简答题：答案输入区在底部（draw_input 专属 hint + 输入行），此处只提示
        body.push(Line::default());
        body.push(Line::from(Span::styled(
            "  在下方输入你的答案，Enter 提交批改 · Esc 退出复习",
            Style::new().fg(theme::MUTED),
        )));
    }

    // 滚动视口 + 文本行存储：复用聊天流的同一套机制（滚轮/拖选复制都依赖
    // chat_lines + chat_rect + scroll_up，此前 workspace 没存导致滚不动、
    // 拖选复制到的是过期聊天内容）。scroll_up=0 跟随最新，向上滚看历史。
    let viewport = area.height as usize;
    let max_offset = body.len().saturating_sub(viewport);
    let top = max_offset.saturating_sub(app.scroll_up as usize);
    app.chat_lines = body
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    app.chat_rect = area;
    let sel = app.text_selection;
    let visible: Vec<Line> = body
        .into_iter()
        .enumerate()
        .skip(top)
        .take(viewport.max(1))
        .map(|(i, mut line)| {
            if let Some((start, end)) = sel
                && i >= start
                && i <= end
            {
                // 蓝底高亮：与聊天流拖选同款
                for span in &mut line.spans {
                    span.style = span.style.fg(Color::White).bg(Color::Blue);
                }
            }
            line
        })
        .collect();

    f.render_widget(Paragraph::new(visible), area);
}

/// 右上角临时通知：复制成功/失败等一次性反馈，2.5s（错误 4s）自动消失。
fn draw_toast(f: &mut Frame, app: &App) {
    use std::time::Instant;
    let Some(toast) = &app.toast else { return };
    let area = f.area();
    let msg_w = display_width(&toast.message) as u16;
    let width = (msg_w + 4).min(area.width.saturating_sub(2));
    let height = 3u16; // 边框 + 一行文本
    let x = area.x + area.width.saturating_sub(width + 1);
    let y = area.y + 4; // header 之下，贴右上
    let pop = Rect::new(x, y, width, height);
    f.render_widget(ratatui::widgets::Clear, pop);
    let color = if toast.error { ERROR } else { SUCCESS };
    let remaining = toast
        .expires_at
        .saturating_duration_since(Instant::now())
        .as_secs();
    let title = if toast.error {
        " ⚠ 复制失败 "
    } else {
        " ✓ 已复制 "
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            display_truncate(&toast.message, (width - 2) as usize),
            Style::new().fg(color),
        )))
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(color))
                .title(Span::styled(title, Style::new().fg(color))),
        ),
        pop,
    );
    // 剩余秒数提示（可选调试信息，保持简洁不渲染）
    let _ = remaining;
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    // 状态由覆盖层状态推导（复习/导入/思考中/就绪），符号+语义色（◌ 进行中 / ● 就绪）
    let (status_text, status_color) = app.status_label();

    // 左侧 = 位置面包屑：Home / Course / Session 三级清晰可见。
    // Global scope 显示 "Global"（文档：all 不是 Course）。
    let scope_label = app.current_scope_label();
    let left = match app.workspace {
        crate::app::Workspace::Home => " StudyPilot".to_string(),
        crate::app::Workspace::Course => format!(" StudyPilot │ {scope_label}"),
        crate::app::Workspace::Session => {
            let session = app
                .current_session_id()
                .and_then(|id| app.sidebar_sessions.iter().find(|s| s.id == id))
                .and_then(|s| s.title.as_deref())
                .unwrap_or("New conversation");
            format!(" StudyPilot │ {scope_label} │ {session}")
        }
    };
    let right = format!(
        "{} │ ¥{:.2}/{:.0} │ {} ",
        app.provider_cfg.model, app.total_cost, app.max_cost, status_text,
    );
    let left_w = display_width(&left);
    let right_w = display_width(&right);
    let pad = (area.width as usize).saturating_sub(left_w + right_w + 2);

    let line = Line::from(vec![
        Span::styled(left, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::styled(
            format!("{} │ ", app.provider_cfg.model),
            Style::new().fg(theme::SECONDARY_DIM),
        ),
        Span::styled(
            format!("¥{:.2}/{:.0} │ ", app.total_cost, app.max_cost),
            Style::new().fg(if app.total_cost > app.max_cost * 0.8 {
                ERROR
            } else {
                theme::MUTED
            }),
        ),
        Span::styled(
            status_text,
            Style::new().fg(status_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]);
    f.render_widget(
        Paragraph::new(line).block(Block::new().borders(Borders::ALL)),
        area,
    );
}

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = Vec::new();
    // 分区标题：大写 + 暗色（导航语义，不抢正文注意力）。
    // 只列真实课程（文档：all 不是 Course entity，侧栏不出现）。
    items.push(ListItem::new(Line::from(Span::styled(
        " COURSES",
        Style::new().fg(theme::MUTED).add_modifier(Modifier::BOLD),
    ))));

    let current_cid = app.current_course_id();
    // 会话归属课程下：展开当前课程的学习活动，其他课程仅名称（Session 不再与 Course 平级）
    for (id, name) in &app.courses {
        let is_current = *name == app.course;
        let style = sidebar_style(name, &app.course);
        let mark = if is_current { "●" } else { " " };
        items.push(ListItem::new(Line::from(Span::styled(
            format!(" {mark} {name}"),
            style,
        ))));
        if let Some((notes, concepts)) = app.sidebar_course_stats.get(id) {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("    {notes} notes · {concepts} concepts"),
                Style::new().fg(DIM),
            ))));
        }
        // 当前课程下挂最近学习活动（Session 是 Course 的子内容）
        if is_current && Some(*id) == current_cid {
            let row_w = area.width as usize;
            for s in app.sidebar_sessions.iter().take(8) {
                if s.course_id != Some(*id) {
                    continue;
                }
                let title = s.title.as_deref().unwrap_or("(未命名)");
                let is_open = app.current_session_id() == Some(s.id);
                let prefix = "    ▸ ";
                let avail = row_w.saturating_sub(display_width(prefix)).max(4);
                let short = display_truncate(title, avail);
                let title_style = if is_open {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme::FG)
                };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("{prefix}{short}"),
                    title_style,
                ))));
            }
        }
    }

    f.render_widget(List::new(items), area);
}

fn sidebar_style(name: &str, current: &str) -> Style {
    if name == current {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme::MUTED)
    }
}

/// 聊天流：手动折行 + 选中高亮 + 存储文本行供复制提取。
fn draw_chat(f: &mut Frame, area: Rect, app: &mut App) {
    // 聊天区无边框：内容区 = 整个 area（拖选坐标映射与此同系）
    let inner_width = area.width.saturating_sub(2) as usize;
    let viewport = area.height as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut text_lines: Vec<String> = Vec::new();
    if inner_width > 0 && viewport > 0 {
        for entry in &app.entries {
            let before = lines.len();
            append_entry_lines(entry, inner_width, &mut lines);
            for line in &lines[before..] {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                text_lines.push(text);
            }
        }
        // inflight 时追加 spinner 行
        if app.is_inflight() {
            let spinner = spinner_char(app.tick);
            lines.push(Line::from(vec![
                Span::styled(format!("{spinner} "), Style::new().fg(theme::USER)),
                Span::styled("思考中…", Style::new().fg(DIM)),
            ]));
            text_lines.push(format!("{spinner} 思考中…"));
        }
    }

    let max_offset = lines.len().saturating_sub(viewport);
    let top = max_offset.saturating_sub(app.scroll_up as usize);

    // 存储供 handle_mouse 使用
    app.chat_lines = text_lines;
    app.chat_rect = area;

    // 高亮选中行
    let sel = app.text_selection;
    let visible: Vec<Line> = lines
        .into_iter()
        .enumerate()
        .skip(top)
        .take(viewport.max(1))
        .map(|(i, mut line)| {
            if let Some((start, end)) = sel
                && i >= start
                && i <= end
            {
                // 蓝底高亮：所有终端上都醒目可见
                for span in &mut line.spans {
                    span.style = span.style.fg(Color::White).bg(Color::Blue);
                }
            }
            line
        })
        .collect();

    f.render_widget(Paragraph::new(visible), area);
}

fn append_entry_lines(entry: &Entry, width: usize, out: &mut Vec<Line<'static>>) {
    // 消息块宽度 = 可用宽 - 色条占位（"▎ " 2 列）
    let w = width.saturating_sub(2);
    // 用户/AI/系统：左色条 + 前缀着色（语义），正文回归浅灰——两套视觉语言
    match entry {
        Entry::Info(text) => push_prefixed_wrapped(out, "· ", DIM, text, theme::MUTED, width, None),
        Entry::User(text) => {
            push_prefixed_wrapped(out, "你 › ", USER, text, theme::FG, w, Some(USER));
            out.push(Line::default()); // 块间空行：黄条与下一条消息隔开
        }
        Entry::Assistant {
            content,
            reasoning_chars,
        } => {
            if let Some(n) = reasoning_chars {
                out.push(Line::from(vec![
                    Span::styled("▎ ", Style::new().fg(theme::SECONDARY)),
                    Span::styled(format!("  (已思考 {n} 字符)"), Style::new().fg(DIM)),
                ]));
            }
            // AI 前缀：Secondary 蓝紫（与用户暖黄形成两套视觉语言）
            out.push(Line::from(vec![
                Span::styled("▎ ", Style::new().fg(theme::SECONDARY)),
                Span::styled(
                    "StudyPilot ›".to_owned(),
                    Style::new()
                        .fg(theme::SECONDARY)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            // Markdown 渲染（块内：来源脚注与思考行同属该块）
            let mut lines: Vec<Line<'static>> = Vec::new();
            let md_lines = markdown::render_markdown(content, w);
            lines.extend(md_lines);
            for l in lines {
                let mut spans: Vec<Span<'static>> =
                    vec![Span::styled("▎ ", Style::new().fg(theme::SECONDARY))];
                spans.extend(l.spans);
                out.push(Line::from(spans));
            }
            out.push(Line::default()); // 块间空行：紫条与下一条消息隔开
        }
        Entry::Advice {
            mastered,
            consolidate,
            next,
        } => {
            // 复习小结：标题行分组着色（绿/黄/蓝），内容行默认前景色（用户反馈：内容也上色太花）
            let head = |mark: &'static str, color: ratatui::style::Color| {
                Line::from(vec![
                    Span::styled("▎ ", Style::new().fg(theme::PRIMARY)),
                    Span::styled(
                        format!("  {mark}"),
                        Style::new().fg(color).add_modifier(Modifier::BOLD),
                    ),
                ])
            };
            let item = |text: &str| {
                Line::from(vec![
                    Span::styled("▎ ", Style::new().fg(theme::PRIMARY)),
                    Span::styled(format!("    {text}"), Style::new().fg(theme::FG)),
                ])
            };
            out.push(head("── 复习小结 ──", theme::PRIMARY));
            if !mastered.is_empty() {
                out.push(head("✓ 已掌握", theme::SUCCESS));
                for t in mastered {
                    out.push(item(t));
                }
            }
            if !consolidate.is_empty() {
                out.push(head("△ 需巩固", theme::USER));
                for t in consolidate {
                    out.push(item(t));
                }
            }
            if let Some(n) = next
                && !n.trim().is_empty()
            {
                out.push(head("→ 下一步", theme::SECONDARY));
                // 建议较长：按【显示宽度】折行（中文 2 列——此前按字符数导致超宽被裁）
                let w = width.saturating_sub(8).max(1);
                let mut cur = String::new();
                let mut wsum = 0usize;
                for ch in n.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if wsum + cw > w && wsum > 0 {
                        out.push(item(&cur));
                        cur.clear();
                        wsum = 0;
                    }
                    cur.push(ch);
                    wsum += cw;
                }
                if !cur.is_empty() {
                    out.push(item(&cur));
                }
            }
            out.push(Line::default());
        }
        Entry::Markdown(text) => {
            // 富文本块（摘要卡/大纲）：PRIMARY 色条 + markdown 管线
            let md_lines = markdown::render_markdown(text, w);
            for l in md_lines {
                let mut spans: Vec<Span<'static>> =
                    vec![Span::styled("▎ ", Style::new().fg(theme::PRIMARY))];
                spans.extend(l.spans);
                out.push(Line::from(spans));
            }
            out.push(Line::default());
        }
        Entry::Tool { text, ok } => {
            // 工具活动：◌ 蓝紫进行中 / ✓ 绿 / ✗ 红
            let (mark, color) = match ok {
                None => ("◌ ", theme::PRIMARY),
                Some(true) => ("✓ ", theme::SUCCESS),
                Some(false) => ("✗ ", ERROR),
            };
            out.push(Line::from(Span::styled(
                format!("  {mark}{text}"),
                Style::new().fg(color),
            )));
        }
        Entry::Citation(text) => {
            // RAG 品牌色：引用来源行整体 Reference 青（同属 AI 块色条）
            out.push(Line::from(vec![
                Span::styled("▎ ", Style::new().fg(theme::SECONDARY)),
                Span::styled(format!("  {text}"), Style::new().fg(theme::REFERENCE)),
            ]));
        }
        Entry::Error(text) => {
            push_prefixed_wrapped(out, "✗ ", ERROR, text, theme::FG, w, Some(ERROR))
        }
    }
}

/// 前缀着色 + 正文浅灰的折行：首行带彩色前缀，续行按前缀宽度缩进对齐。
fn push_prefixed_wrapped(
    out: &mut Vec<Line<'static>>,
    prefix: &str,
    prefix_color: Color,
    text: &str,
    body_color: Color,
    width: usize,
    bar: Option<Color>,
) {
    // 左侧色条（"▎ "）占 2 列，块内所有行统一携带 → 形成连续竖线的消息块
    let bar_span = bar.map(|c| Span::styled("▎ ", Style::new().fg(c)));
    let bar_w = if bar.is_some() { 2 } else { 0 };
    let pw = display_width(prefix);
    let body_w = width.saturating_sub(pw + bar_w);
    let body_style = Style::new().fg(body_color);
    let prefix_span = Span::styled(prefix.to_owned(), Style::new().fg(prefix_color));
    let indent = format!("{}{}", " ".repeat(bar_w), " ".repeat(pw));
    let mut first = true;
    for seg in wrap(text, body_w.max(4)) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let Some(bs) = &bar_span {
            spans.push(bs.clone());
        }
        if first {
            spans.push(prefix_span.clone());
            first = false;
        } else {
            spans.push(Span::raw(indent.clone()));
        }
        spans.push(Span::styled(seg, body_style));
        out.push(Line::from(spans));
    }
}

/// 按 unicode 显示宽度折行（CJK 宽 2），处理 \n。
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if width == 0 {
            out.push(raw.to_owned());
            continue;
        }
        let mut cur = String::new();
        let mut w = 0usize;
        for ch in raw.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
            if w + cw > width && w > 0 {
                out.push(std::mem::take(&mut cur));
                w = 0;
            }
            cur.push(ch);
            w += cw;
        }
        out.push(cur);
    }
    out
}

/// 输入按显示宽度折行 + 光标定位：返回 (每行文本, 光标所在行, 光标在行内显示宽度)。
/// 光标边界 = 字符索引 `cursor`（位于前字符之后、下字符之前）。折行规则与 `wrap` 一致。
fn wrap_input_with_cursor(input: &str, cursor: usize, width: usize) -> (Vec<String>, usize, usize) {
    let mut vis_lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut w = 0usize;
    let mut idx = 0usize;
    let mut cursor_line = 0usize;
    let mut cursor_col = 0usize;
    let mut placed = false;
    for ch in input.chars() {
        if idx == cursor && !placed {
            cursor_line = vis_lines.len();
            cursor_col = w;
            placed = true;
        }
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
        if width > 0 && w + cw > width && w > 0 {
            vis_lines.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(ch);
        w += cw;
        idx += 1;
    }
    if idx == cursor && !placed {
        cursor_line = vis_lines.len();
        cursor_col = w;
    }
    vis_lines.push(cur);
    (vis_lines, cursor_line, cursor_col)
}

fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    // Home / Course workspace：不渲染输入框，只给操作提示（Ask 输入框只在 Session 出现）
    if app.workspace != crate::app::Workspace::Session {
        let hint = match app.workspace {
            crate::app::Workspace::Home => {
                "↑↓ navigate    Enter open    d delete    Ctrl+K commands"
            }
            crate::app::Workspace::Course => {
                "↑↓ navigate    Enter open    Esc back    Ctrl+K commands"
            }
            _ => unreachable!(),
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, Style::new().fg(DIM)))),
            area,
        );
        return;
    }
    // Review 模式：底部状态栏专属化；反馈停留态输入行即追问框（前缀"追问："）
    let in_feedback = app.review.as_ref().is_some_and(|rs| rs.awaiting_feedback());
    let (hint, show_input) = match &app.review {
        Some(rs) => {
            let is_choice = rs
                .questions
                .get(rs.current)
                .map(|q| q.q_type == crate::review::QType::Choice)
                .unwrap_or(false);
            if app.is_review_grading() {
                ("批改中…", false)
            } else if rs.awaiting_feedback() {
                if app.followup_pending.is_some() {
                    ("正在生成回答 · Esc 中断追问", true)
                } else {
                    ("空 Enter 下一题 · 输入可追问 · Esc 退出复习", true)
                }
            } else if is_choice {
                ("↑↓ 选择 · A-D 作答 · Enter 提交 · Esc 退出", false)
            } else {
                ("输入答案 · Enter 提交批改 · Esc 退出复习", true)
            }
        }
        None => {
            if app.is_inflight() {
                ("Ctrl+C/Esc 中断请求", true)
            } else {
                ("Enter 发送 · Ctrl+K 命令面板 · Ctrl+C 退出", true)
            }
        }
    };
    // 输入行前缀：反馈停留态 = 追问框，其余 = 普通输入
    let prefix = if in_feedback { "追问：" } else { "› " };
    let prefix_w = display_width(prefix);

    // 底线样式：上边一条分隔线，无框；输入过长按显示宽度折行（多行渲染 + 光标跟随）
    let input_w = (area.width.saturating_sub(2)).max(4) as usize;
    let mut lines = vec![Line::from(Span::styled(
        hint.to_owned(),
        Style::new().fg(DIM),
    ))];
    let mut cursor_line = 0usize;
    let mut cursor_col = 0usize;
    if show_input {
        let (vis_lines, cl, cc) = wrap_input_with_cursor(&app.input, app.cursor_pos, input_w);
        cursor_line = cl;
        cursor_col = cc;
        if app.input.is_empty() {
            let ph = if in_feedback {
                String::new() // 追问：前缀本身即说明
            } else if app.review.is_some() {
                "输入你的答案…".to_owned()
            } else if app.is_inflight() {
                String::new()
            } else {
                format!("Ask {}...", app.current_scope_label())
            };
            if !ph.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(prefix.to_owned(), Style::new().fg(ACCENT)),
                    Span::styled(ph, Style::new().fg(DIM)),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    prefix.to_owned(),
                    Style::new().fg(ACCENT),
                )));
            }
        } else {
            for (i, seg) in vis_lines.iter().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(prefix.to_owned(), Style::new().fg(ACCENT)),
                        Span::raw(seg.clone()),
                    ]));
                } else {
                    lines.push(Line::from(Span::raw(seg.clone())));
                }
            }
        }
    }
    f.render_widget(Paragraph::new(lines), area);

    // 光标定位到折行后的正确可视行/列（首行带前缀，占 prefix_w 列）
    if show_input {
        let x = area.x + if cursor_line == 0 { prefix_w as u16 } else { 0 } + cursor_col as u16;
        let y = area.y + 1 + cursor_line as u16;
        if x < area.right() && y < area.bottom() {
            f.set_cursor_position((x, y));
        }
    }
}

/// /model 弹窗：居中覆盖层，当前 provider 打 →，选中项高亮。
/// scope 标签（文档 §24）：Global → " · Global"；CurrentCourse → " · Rust"；
/// CurrentSession → " · 当前会话"。全部 dim。
fn palette_scope_suffix(scope: crate::palette::PaletteScope, current_course: &str) -> String {
    use crate::palette::PaletteScope as S;
    let tag = match scope {
        S::Global => "Global".to_owned(),
        S::CurrentCourse => current_course.to_owned(),
        S::CurrentSession => "当前会话".to_owned(),
    };
    format!(" · {tag}")
}

/// First-run AI Setup Wizard 覆盖层（键盘第一：↑↓/Enter/Esc）。
fn draw_setup(f: &mut Frame, app: &mut App) {
    use crate::app::setup::SetupStep;
    let Some(s) = &app.setup else { return };
    let area = f.area();
    let width = 52u16.min(area.width.saturating_sub(4));
    let height = 18u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);

    f.render_widget(ratatui::widgets::Clear, pop);

    let mut items: Vec<ListItem> = Vec::new();
    let (title, hint): (String, String) = match s.step {
        SetupStep::Provider => {
            items.push(ListItem::new(Line::from(Span::styled(
                "Choose a provider",
                Style::new().fg(DIM),
            ))));
            for (i, (_, name)) in s.provider_options().iter().enumerate() {
                let sel = i == s.cursor;
                let mark = if sel { "▸ " } else { "  " };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("{mark}{name}"),
                    if sel {
                        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(Color::Gray)
                    },
                ))));
            }
            (
                "AI Setup".to_owned(),
                "↑↓ select · Enter continue · Esc back".into(),
            )
        }
        SetupStep::Model => {
            items.push(ListItem::new(Line::from(Span::styled(
                "Choose a model",
                Style::new().fg(DIM),
            ))));
            let opts = s.model_options();
            if opts.is_empty() {
                items.push(ListItem::new(Line::from(Span::styled(
                    "  No preset models available.",
                    Style::new().fg(DIM),
                ))));
                items.push(ListItem::new(Line::from(Span::styled(
                    "  (configure via Custom provider)",
                    Style::new().fg(DIM),
                ))));
            } else {
                for (i, m) in opts.iter().enumerate() {
                    let sel = i == s.cursor;
                    let mark = if sel { "▸ " } else { "  " };
                    items.push(ListItem::new(Line::from(Span::styled(
                        format!("{mark}{m}"),
                        if sel {
                            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                        } else {
                            Style::new().fg(Color::Gray)
                        },
                    ))));
                }
            }
            (
                "AI Setup · Model".to_owned(),
                "↑↓ select · Enter continue · Esc back".into(),
            )
        }
        SetupStep::Credentials => {
            items.push(ListItem::new(Line::from(Span::styled(
                "API Key",
                Style::new().fg(DIM),
            ))));
            let masked: String = s.secret_buf.chars().map(|_| '•').collect();
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  {masked}▍"),
                Style::new().fg(Color::Gray),
            ))));
            (
                "AI Setup · API Key".to_owned(),
                "type key · Enter continue · Esc back".into(),
            )
        }
        SetupStep::Test => {
            items.push(ListItem::new(Line::from(Span::styled(
                "Test connection",
                Style::new().fg(DIM),
            ))));
            let provider = s.provider.as_deref().unwrap_or("-");
            let model = s.model.as_deref().unwrap_or("-");
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  Provider: {provider}"),
                Style::new().fg(Color::Gray),
            ))));
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  Model: {model}"),
                Style::new().fg(Color::Gray),
            ))));
            items.push(ListItem::new(Line::default()));
            if s.testing {
                items.push(ListItem::new(Line::from(Span::styled(
                    "  ⟳ Connecting…",
                    Style::new().fg(theme::PRIMARY),
                ))));
            } else if let Some(r) = &s.test_result {
                let ok = r.starts_with('✓');
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("  {r}"),
                    Style::new().fg(if ok { theme::SUCCESS } else { theme::ERROR }),
                ))));
            }
            (
                "AI Setup · Test".to_owned(),
                "Enter test / continue · Esc back".into(),
            )
        }
        SetupStep::Done => {
            items.push(ListItem::new(Line::from(Span::styled(
                "You're ready.",
                Style::new().fg(theme::SUCCESS).add_modifier(Modifier::BOLD),
            ))));
            items.push(ListItem::new(Line::from(Span::styled(
                format!(
                    "  Provider: {} · Model: {}",
                    s.provider.as_deref().unwrap_or("-"),
                    s.model.as_deref().unwrap_or("-")
                ),
                Style::new().fg(DIM),
            ))));
            (
                "AI Setup · Ready".to_owned(),
                "Enter → Start Learning".into(),
            )
        }
    };

    f.render_widget(
        List::new(items).block(
            Block::new()
                .borders(Borders::ALL)
                .title(Span::styled(title, Style::new().fg(ACCENT)))
                .title_bottom(
                    Span::styled(format!(" {hint} "), Style::new().fg(DIM))
                        .into_left_aligned_line(),
                ),
        ),
        pop,
    );
}
fn draw_command_palette(f: &mut Frame, app: &mut App) {
    use crate::palette::{PaletteGroup, PaletteItem};
    // 先取当前课程（避免与 palette 可变借用冲突；scope 标签只需课程名）
    let current_course = app.course.clone();
    let Some(palette) = app.palette.as_mut() else {
        return;
    };
    // 过滤串 = 聊天框共享缓冲，▍ 画在光标位
    let mut shown = String::new();
    for (i, ch) in app.input.chars().enumerate() {
        if i == app.cursor_pos {
            shown.push('▍');
        }
        shown.push(ch);
    }
    if app.cursor_pos >= app.input.chars().count() {
        shown.push('▍');
    }
    let area = f.area();
    let height = 24u16.min(area.height.saturating_sub(2));
    let visible = height.saturating_sub(2) as usize;

    // 显示行序列：无过滤时插组头分隔行，过滤时纯条目列表
    enum Disp<'a> {
        Group(&'a str),
        Item(usize),
    }
    let show_groups = app.input.is_empty();
    let selected_idx = palette.filtered.get(palette.selected).copied();
    let mut disp: Vec<Disp> = Vec::new();
    let mut last_group: Option<PaletteGroup> = None;
    let mut sel_row = 0usize;
    for &idx in &palette.filtered {
        let g = palette.items[idx].group;
        if show_groups && last_group != Some(g) {
            disp.push(Disp::Group(g.label()));
            last_group = Some(g);
        }
        if Some(idx) == selected_idx {
            sel_row = disp.len();
        }
        disp.push(Disp::Item(idx));
    }

    // 滚动窗口：选中条目始终可见
    let max_scroll = disp.len().saturating_sub(visible);
    if sel_row < palette.scroll {
        palette.scroll = sel_row;
    } else if sel_row >= palette.scroll + visible {
        palette.scroll = sel_row + 1 - visible;
    }
    palette.scroll = palette.scroll.min(max_scroll);

    // 动态宽度：列表行宽 = 标记 2 + label 显示宽 + 3 + desc 显示宽
    let cmd_w = palette
        .items
        .iter()
        .map(|i| display_width(i.label))
        .max()
        .unwrap_or(10);
    let desc_w = palette
        .items
        .iter()
        .map(|i| display_width(i.desc))
        .max()
        .unwrap_or(10);
    let list_w = 2 + cmd_w + 3 + desc_w;
    let detail_w = selected_idx
        .map(|i| {
            display_width(&format!(
                "{} — {}",
                palette.items[i].command, palette.items[i].desc
            ))
        })
        .unwrap_or(0)
        + 2;
    let width = list_w.max(detail_w).max(40) as u16 + 2;
    let width = width.min(area.width.saturating_sub(2));
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);

    f.render_widget(ratatui::widgets::Clear, pop);

    let desc_avail = (width as usize).saturating_sub(2 + cmd_w + 3 + 3);
    let items: Vec<ListItem> = disp
        .iter()
        .skip(palette.scroll)
        .take(visible)
        .map(|d| match d {
            Disp::Group(label) => ListItem::new(Span::styled(
                format!(" ── {label} ──"),
                Style::new().fg(DIM),
            )),
            Disp::Item(idx) => {
                let item: &PaletteItem = &palette.items[*idx];
                let is_sel = Some(*idx) == selected_idx;
                let mark = if is_sel { "▸ " } else { "  " };
                // scope 标签（文档 §24）：Global / Current course · Rust / Session
                let scope_suffix = palette_scope_suffix(item.scope, &current_course);
                let desc = display_truncate(item.desc, desc_avail);
                let label = format!(
                    "{mark}{}  {}{}",
                    display_pad(item.label, cmd_w),
                    desc,
                    scope_suffix
                );
                let style = if is_sel {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::Gray)
                };
                ListItem::new(Span::styled(label, style))
            }
        })
        .collect();

    let detail = selected_idx
        .map(|i| {
            let it = &palette.items[i];
            display_truncate(
                &format!(
                    "{} — {}{}",
                    it.label,
                    it.desc,
                    palette_scope_suffix(it.scope, &current_course)
                ),
                width as usize - 4,
            )
        })
        .unwrap_or_else(|| "（无匹配命令）".into());
    let title = format!(
        " 命令面板 {} ",
        if app.input.is_empty() {
            format!("({} 项)", palette.filtered.len())
        } else {
            format!("· 过滤: {shown}（{} 项）", palette.filtered.len())
        }
    );
    f.render_widget(
        List::new(items).block(
            Block::new()
                .borders(Borders::ALL)
                .title(Span::styled(title, Style::new().fg(ACCENT)))
                .title_bottom(
                    Span::styled(format!(" {detail} "), Style::new().fg(DIM))
                        .into_left_aligned_line(),
                ),
        ),
        pop,
    );
}

/// 列表选择器弹窗：↑↓ 选择、Enter 提交绑定命令。
fn draw_list_picker(f: &mut Frame, lp: &ListPicker, input: &str) {
    let area = f.area();
    // 弹窗高度按过滤后条目数适配（搜索后弹窗随之缩小）
    let visible = lp.visible(input);
    let height = (visible.len() as u16 + 5).min(area.height.saturating_sub(2));
    let width = 52u16.min(area.width.saturating_sub(2));
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);

    f.render_widget(ratatui::widgets::Clear, pop);

    // 渲染过滤后列表（selected 为过滤后位置）；空结果给占位行
    // 搜索串镜像显示在弹窗内（与 /notes /sessions 一致：> 搜索串▍）
    let items: Vec<ListItem> = {
        let mut out: Vec<ListItem> = Vec::new();
        if !input.is_empty() {
            let mut shown = String::new();
            // picker 无光标语义：串尾固定 ▍ 表示搜索中
            shown.push_str(input);
            shown.push('▍');
            out.push(ListItem::new(Span::styled(
                format!("  > {shown}"),
                Style::new().fg(theme::FG),
            )));
        }
        if visible.is_empty() {
            out.push(ListItem::new(Span::styled(
                "  （无匹配项）",
                Style::new().fg(DIM),
            )));
        }
        for (vi, &i) in visible.iter().enumerate() {
            let c = &lp.items[i];
            let mark = if vi == lp.selected { "▸ " } else { "  " };
            let label = format!("{mark}{}", c.label);
            let style = if vi == lp.selected {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            out.push(ListItem::new(Span::styled(label, style)));
        }
        out
    };
    // ListState 跟随 selected 自动滚动视口（长列表翻页可见，修 65+ 概念选择器）
    let mut state = ratatui::widgets::ListState::default().with_selected(Some(lp.selected));
    f.render_stateful_widget(
        List::new(items).block(
            Block::new()
                .borders(Borders::ALL)
                .title(Span::styled(
                    format!(" {} ", lp.title),
                    Style::new().fg(ACCENT),
                ))
                .title_bottom(
                    Span::styled(" ↑↓ 选择 · Enter 确认 · Esc 取消 ", Style::new().fg(DIM))
                        .into_centered_line(),
                ),
        ),
        pop,
        &mut state,
    );
}

/// 会话浏览器：搜索 → 选择（恢复/重命名/删除）。
fn draw_session_browser(f: &mut Frame, app: &App) {
    use crate::session_browser::{SESSION_LIST_VISIBLE, SessionBrowserMode};
    let Some(browser) = app.session_browser.as_ref() else {
        return;
    };
    let area = f.area();
    let width = 60u16.min(area.width.saturating_sub(2));
    let height = 20u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);
    f.render_widget(ratatui::widgets::Clear, pop);

    let current_id = app.current_session_id();
    let mut title = format!(" 会话浏览器 · {} ", browser.results.len());
    let mut body: Vec<Line<'static>> = Vec::new();
    let footer: String = match browser.mode {
        SessionBrowserMode::Search => {
            title.push_str("· 搜索");
            let mut shown = String::new();
            for (i, ch) in app.input.chars().enumerate() {
                if i == app.cursor_pos {
                    shown.push('▍');
                }
                shown.push(ch);
            }
            if app.cursor_pos >= app.input.chars().count() {
                shown.push('▍');
            }
            body.push(Line::from(Span::styled(
                format!("  > {shown}"),
                Style::new().fg(theme::FG),
            )));
            body.push(Line::default());
            if browser.loading {
                body.push(Line::from(Span::styled("  搜索中…", Style::new().fg(DIM))));
            }
            let visible = SESSION_LIST_VISIBLE.saturating_sub(3);
            for m in browser.results.iter().skip(browser.scroll).take(visible) {
                let course = app.course_label(m.course_id);
                let t = m.title.as_deref().unwrap_or("(未命名)");
                body.push(Line::from(Span::styled(
                    format!("  #{id} {t} · {course}", id = m.id),
                    Style::new().fg(Color::Gray),
                )));
            }
            let total = browser.results.len();
            if total > visible {
                body.push(Line::from(Span::styled(
                    format!(
                        "  … {}/{}（↑↓ 滚动）",
                        (browser.scroll + visible).min(total),
                        total
                    ),
                    Style::new().fg(DIM),
                )));
            }
            " 输入过滤 · Enter 进入选择 · Esc 关闭 ".into()
        }
        SessionBrowserMode::Select => {
            title.push_str("· 选择");
            for (i, m) in browser
                .results
                .iter()
                .enumerate()
                .skip(browser.scroll)
                .take(SESSION_LIST_VISIBLE)
            {
                let mark = if i == browser.cursor { "▸ " } else { "  " };
                let course = app.course_label(m.course_id);
                let t = m.title.as_deref().unwrap_or("(未命名)");
                let is_current = current_id == Some(m.id);
                let style = if i == browser.cursor {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else if is_current {
                    Style::new().fg(ACCENT)
                } else {
                    Style::new().fg(Color::Gray)
                };
                let cur_mark = if is_current { "（当前）" } else { "" };
                body.push(Line::from(Span::styled(
                    format!("{mark}#{id} {t}{cur_mark} · {course}", id = m.id),
                    style,
                )));
            }
            if browser.results.is_empty() {
                body.push(Line::from(Span::styled(
                    "  （无会话）",
                    Style::new().fg(DIM),
                )));
            }
            " Enter 恢复 · r 重命名 · d 删除 · / 搜索 · Esc 返回 ".into()
        }
        SessionBrowserMode::Rename => {
            title.push_str("· 重命名");
            body.push(Line::from(Span::styled(
                format!("  会话 #{}", browser.rename_id.unwrap_or(0)),
                Style::new().fg(DIM),
            )));
            body.push(Line::default());
            let mut shown = String::new();
            for (i, ch) in app.input.chars().enumerate() {
                if i == app.cursor_pos {
                    shown.push('▍');
                }
                shown.push(ch);
            }
            if app.cursor_pos >= app.input.chars().count() {
                shown.push('▍');
            }
            body.push(Line::from(Span::styled(
                format!("  新标题 > {shown}"),
                Style::new().fg(theme::FG),
            )));
            " Enter 确认 · Esc 取消 ".into()
        }
        SessionBrowserMode::ConfirmDelete => {
            let meta = browser.current();
            title.push_str("· ⚠ 确认删除");
            if let Some(m) = meta {
                body.push(Line::from(Span::styled(
                    format!(
                        "  删除会话 #{id} {t}？",
                        id = m.id,
                        t = m.title.as_deref().unwrap_or("(未命名)")
                    ),
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
            }
            body.push(Line::from(Span::styled(
                "  ⚠ 该会话的全部聊天记录将被删除且无法恢复。",
                Style::new().fg(DIM),
            )));
            " Enter 确认删除 · Esc 取消 ".into()
        }
    };
    f.render_widget(
        Paragraph::new(body).block(
            Block::new()
                .borders(Borders::ALL)
                .title(Span::styled(title, Style::new().fg(ACCENT)))
                .title_bottom(Span::styled(footer, Style::new().fg(DIM)).into_left_aligned_line()),
        ),
        pop,
    );
}

/// 笔记浏览器：搜索 → 多选 → 动作确认，四态渲染。
fn draw_note_browser(f: &mut Frame, app: &mut App) {
    use crate::note_browser::{BatchAction, BrowserMode};
    // PickTarget 渲染需要 app.courses，先于 browser 可变借用取好
    let is_pick = matches!(
        app.note_browser.as_ref().map(|b| b.mode),
        Some(crate::note_browser::BrowserMode::PickTarget)
    );
    let pick_items: Vec<(i64, String)> = if is_pick { pick_items(app) } else { Vec::new() };
    let Some(browser) = app.note_browser.as_mut() else {
        return;
    };
    let area = f.area();
    let width = 66u16.min(area.width.saturating_sub(2));
    let height = 18u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);
    f.render_widget(ratatui::widgets::Clear, pop);

    let inner_w = width.saturating_sub(4) as usize;
    let mut title = format!(" 浏览笔记 · 范围: {} ", browser.scope_label);
    let mut body: Vec<Line<'static>> = Vec::new();
    let footer: String = match browser.mode {
        BrowserMode::Search => {
            title.push_str("· 搜索");
            // 搜索词在光标位插 ▍（共享聊天框缓冲）
            let mut shown = String::new();
            for (i, ch) in app.input.chars().enumerate() {
                if i == app.cursor_pos {
                    shown.push('▍');
                }
                shown.push(ch);
            }
            if app.cursor_pos >= app.input.chars().count() {
                shown.push('▍');
            }
            body.push(Line::from(Span::styled(
                format!("  > {shown}"),
                Style::new().fg(theme::FG),
            )));
            body.push(Line::default());
            if browser.loading {
                body.push(Line::from(Span::styled("  搜索中…", Style::new().fg(DIM))));
            }
            for n in &browser.results {
                let course = app
                    .courses
                    .iter()
                    .find(|(id, _)| Some(*id) == n.course_id)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| "all".into());
                body.push(Line::from(Span::styled(
                    format!("  {} ({})", n.title, course),
                    Style::new().fg(Color::Gray),
                )));
            }
            " 输入过滤 · Enter 进入选择 · Esc 关闭 ".into()
        }
        BrowserMode::Select => {
            // 光标可见性对齐滚动窗口
            browser.ensure_cursor_visible(crate::note_browser::LIST_VISIBLE);
            title.push_str(&format!(
                "· 结果 {} · 已选 {}",
                browser.results.len(),
                browser.selected_count()
            ));
            for (i, n) in browser
                .results
                .iter()
                .enumerate()
                .skip(browser.scroll)
                .take(crate::note_browser::LIST_VISIBLE)
            {
                let mark = if i == browser.cursor { "▸ " } else { "  " };
                let check = if browser.selected.contains(&n.id) {
                    "[✓] "
                } else {
                    "[ ] "
                };
                let style = if i == browser.cursor {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else if browser.selected.contains(&n.id) {
                    Style::new().fg(ACCENT)
                } else {
                    Style::new().fg(Color::Gray)
                };
                body.push(Line::from(Span::styled(
                    format!("{mark}{check}{}", n.title),
                    style,
                )));
            }
            if browser.results.is_empty() {
                body.push(Line::from(Span::styled(
                    "  （无匹配笔记）",
                    Style::new().fg(DIM),
                )));
            }
            " Space 选中 · Ctrl+A 全选 · m 移动 · d 删除 · / 搜索 · Esc 返回 ".into()
        }
        BrowserMode::PickTarget => {
            title.push_str("· 移动到");
            for (i, (_id, name)) in pick_items.into_iter().enumerate() {
                let mark = if i == browser.pick_cursor {
                    "❯ "
                } else {
                    "  "
                };
                let style = if i == browser.pick_cursor {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::Gray)
                };
                body.push(Line::from(Span::styled(format!("{mark}{name}"), style)));
            }
            " ↑↓ 选择 · Enter 确认 · Esc 返回 ".into()
        }
        BrowserMode::Confirm => {
            let (ids, titles) = browser.action_targets();
            let n = ids.len();
            match browser.action {
                Some(BatchAction::Move) => {
                    title.push_str("· 确认移动");
                    body.push(Line::from(Span::styled(
                        format!("  将移动 {n} 篇笔记 → {}", browser.move_target_label),
                        Style::new().fg(theme::FG).add_modifier(Modifier::BOLD),
                    )));
                }
                Some(BatchAction::Delete) => {
                    let strong = n > crate::note_browser::STRONG_CONFIRM_THRESHOLD;
                    title.push_str(if strong {
                        "· ⚠ 强确认"
                    } else {
                        "· 确认删除"
                    });
                    body.push(Line::from(Span::styled(
                        format!("  将删除 {n} 篇笔记"),
                        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )));
                    body.push(Line::from(Span::styled(
                        "  ⚠ 从知识库移除后无法检索；原始文件不会被删除。",
                        Style::new().fg(DIM),
                    )));
                    if strong {
                        body.push(Line::from(Span::styled(
                            format!("  请输入 DELETE 确认：{}", app.input),
                            Style::new().fg(theme::USER),
                        )));
                    }
                }
                None => {}
            }
            // 操作预览（前 8 条）
            body.push(Line::default());
            for t in titles.iter().take(8) {
                body.push(Line::from(Span::styled(
                    format!("  · {t}"),
                    Style::new().fg(Color::Gray),
                )));
            }
            if n > 8 {
                body.push(Line::from(Span::styled(
                    format!("  · …等 {n} 篇"),
                    Style::new().fg(DIM),
                )));
            }
            if matches!(browser.action, Some(BatchAction::Delete))
                && n > crate::note_browser::STRONG_CONFIRM_THRESHOLD
            {
                " 输入 DELETE 后 Enter 执行 · Esc 取消 ".into()
            } else {
                " Enter 执行 · Esc 返回 ".into()
            }
        }
    };
    let _ = inner_w;
    f.render_widget(
        Paragraph::new(body).block(
            Block::new()
                .borders(Borders::ALL)
                .title(Span::styled(title, Style::new().fg(ACCENT)))
                .title_bottom(Span::styled(footer, Style::new().fg(DIM)).into_left_aligned_line()),
        ),
        pop,
    );
}

/// pick_items 与 keys.rs 共用（all 哨兵 -1）。
fn pick_items(app: &App) -> Vec<(i64, String)> {
    let mut items = vec![(-1i64, "all（全部）".to_owned())];
    items.extend(app.courses.iter().cloned());
    items
}

/// 参数向导弹窗：单步文本输入（预填默认值），Enter 推进 / Esc 回退。
fn draw_wizard(f: &mut Frame, app: &App) {
    let wizard = app.wizard.as_ref().expect("调用方保证 wizard 已打开");
    let area = f.area();
    let height = 7u16.min(area.height.saturating_sub(2));
    let width = 58u16.min(area.width.saturating_sub(2));
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);

    f.render_widget(ratatui::widgets::Clear, pop);

    let step_prompt = wizard.steps[wizard.current].prompt;
    let inner_w = width.saturating_sub(4) as usize;
    // 输入 = 聊天框共享缓冲；▍ 画在光标位，超宽时滑动窗口保证可见
    let chars: Vec<char> = app.input.chars().collect();
    let cursor_at_tail = app.cursor_pos >= chars.len();
    let mut shown: Vec<char> = Vec::with_capacity(chars.len() + 1);
    for (i, ch) in chars.iter().enumerate() {
        if i == app.cursor_pos {
            shown.push('▍');
        }
        shown.push(*ch);
    }
    if cursor_at_tail {
        shown.push('▍');
    }
    let view = cursor_window(&shown, app.cursor_pos, inner_w.saturating_sub(2));
    let value: String = view.into_iter().collect();

    let lines = vec![
        Line::from(Span::styled(
            format!(
                "步骤 {}/{} · {}",
                wizard.current + 1,
                wizard.steps.len(),
                step_prompt
            ),
            Style::new().fg(DIM),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  > {value}"),
            Style::new().fg(theme::FG),
        )),
        Line::from(""),
        Line::from(Span::styled(
            if wizard.current == 0 {
                " Enter 确认 · Esc 取消向导".to_owned()
            } else {
                " Enter 确认 · Esc 返回上一步".to_owned()
            },
            Style::new().fg(DIM),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).block(Block::new().borders(Borders::ALL).title(Span::styled(
            format!(" {} ", wizard.title),
            Style::new().fg(ACCENT),
        ))),
        pop,
    );
}

/// unicode 显示宽度（中文按 2 列）。
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// 按显示宽度右侧补空格。
fn display_pad(s: &str, w: usize) -> String {
    let dw = display_width(s);
    let mut out = s.to_owned();
    for _ in dw..w {
        out.push(' ');
    }
    out
}

/// 按显示宽度截断（保序丢尾部字符）。
fn display_truncate(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out
}

/// 行编辑可视窗口：保证 `cursor_idx`（▍ 所在的字符索引）落在显示窗口内。
/// 输入超宽时窗口跟随光标右移（尾部对齐），避免 ▍ 被截断"看不见移动"。
fn cursor_window(chars: &[char], cursor_idx: usize, w: usize) -> Vec<char> {
    if w == 0 {
        return Vec::new();
    }
    if chars.len() <= w {
        return chars.to_vec();
    }
    // ▍ 插入后光标所在索引（在 shown 序列中即 cursor_idx，▍ 排在其前）
    let start = if cursor_idx >= w {
        cursor_idx + 1 - w
    } else {
        0
    };
    let end = (start + w).min(chars.len());
    // 终点回退一个字符宽，防 ▍ 恰好被挤出（简化：字符级近似即可）
    chars[start..end].to_vec()
}

fn draw_model_picker(f: &mut Frame, picker: &ModelPicker, current: &str) {
    let area = f.area();
    let height = (picker.options.len() + 4).min(area.height as usize) as u16;
    let width = 52u16.min(area.width);
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);

    // 清除背景
    f.render_widget(ratatui::widgets::Clear, pop);

    let items: Vec<ListItem> = picker
        .options
        .iter()
        .enumerate()
        .map(|(i, pc)| {
            let mark = if pc.name == current {
                "→"
            } else if i == picker.selected {
                "▸"
            } else {
                " "
            };
            let label = format!(
                " {} {:<14} {:<18} {}",
                mark,
                pc.name,
                pc.model,
                if pc.thinking { "思考" } else { "" }
            );
            let style = if i == picker.selected {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            ListItem::new(Span::styled(label, style))
        })
        .collect();

    let title = " 选择模型 (↑↓ 移动 · 数字直选 · Enter 确认 · Esc 取消) ";
    f.render_widget(
        List::new(items).block(
            Block::new()
                .borders(Borders::ALL)
                .title(Span::styled(title, Style::new().fg(DIM))),
        ),
        pop,
    );
}

#[cfg(test)]
mod cursor_window_tests {
    use super::cursor_window;

    #[test]
    fn short_input_untouched() {
        let chars: Vec<char> = "abc".chars().collect();
        assert_eq!(cursor_window(&chars, 0, 10), vec!['a', 'b', 'c']);
    }

    #[test]
    fn cursor_beyond_width_shifts_window() {
        // 8 字符，窗口宽 4；光标在 7（尾部）→ 窗口应含尾部而非截断丢失
        let chars: Vec<char> = "12345678".chars().collect();
        let win = cursor_window(&chars, 7, 4);
        assert_eq!(win, vec!['5', '6', '7', '8']);
        // 光标在 0 → 头部窗口
        assert_eq!(cursor_window(&chars, 0, 4), vec!['1', '2', '3', '4']);
    }
}

#[cfg(test)]
mod wrap_cursor_tests {
    use super::wrap_input_with_cursor;

    #[test]
    fn single_line_no_wrap() {
        let (lines, cl, cc) = wrap_input_with_cursor("abc", 2, 10);
        assert_eq!(lines, vec!["abc"]);
        assert_eq!((cl, cc), (0, 2));
    }

    #[test]
    fn wraps_to_multiple_lines() {
        // 宽 4：每行 2 个全角字符（甲=宽2）→ 3 行
        let s = "甲乙丙丁戊";
        let (lines, _, _) = wrap_input_with_cursor(s, 0, 4);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn cursor_follows_wrapped_lines() {
        let s = "甲乙丙丁戊";
        // 光标在字符索引 4（丁之后、戊之前）：落在「丙丁」行尾（第 1 行，col 4）
        let (_, cl, cc) = wrap_input_with_cursor(s, 4, 4);
        assert_eq!((cl, cc), (1, 4));
        // 光标在字符索引 2（乙之后）：第 1 行行尾（宽 4）
        let (_, cl, cc) = wrap_input_with_cursor(s, 2, 4);
        assert_eq!((cl, cc), (0, 4));
        // 光标在尾部（索引 5）
        let (_, cl, cc) = wrap_input_with_cursor(s, 5, 4);
        assert_eq!((cl, cc), (2, 2));
    }

    #[test]
    fn empty_input_cursor_at_origin() {
        let (lines, cl, cc) = wrap_input_with_cursor("", 0, 10);
        assert_eq!(lines, vec![""]);
        assert_eq!((cl, cc), (0, 0));
    }
}

#[cfg(test)]
mod home_lines_tests {
    use super::*;
    use crate::app::App;

    /// 两门课 + 一门有历史 session 的 App（复用 commands 测试的构造方式）。
    fn app_with_courses() -> App {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
        };
        let store = std::sync::Arc::new(storage::Store::open_in_memory().unwrap());
        store.get_or_create_course("rust").unwrap();
        store.get_or_create_course("csapp").unwrap();
        let client = std::sync::Arc::new(agent_providers::OpenAiClient::new(cfg.clone()).unwrap());
        let mut app = App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            5.0,
            vec![(1, "rust".into()), (2, "csapp".into())],
        );
        app.sidebar_course_stats.insert(1, (5, 65));
        app.sidebar_course_stats.insert(2, (12, 48));
        app
    }

    fn flatten(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn home_has_launchpad_hierarchy() {
        let app = app_with_courses();
        let text = flatten(&home_lines(&app)).join("\n");
        // 主入口（无 session → 轻量引导；有 session 见下一测试）
        assert!(
            !text.contains("StudyPilot"),
            "header 已有产品名，正文不重复"
        );
        assert!(text.contains("Start learning"));
        assert!(
            text.contains("Choose a course to begin"),
            "无 session 时给轻量引导"
        );
        // 次级导航 + 内容项
        assert!(text.contains("Your courses"));
        assert!(text.contains("rust"));
        assert!(text.contains("5 notes · 65 concepts"));
        assert!(text.contains("csapp"));
        // 新建
        assert!(text.contains("+ New Course"));
        // 不暴露 CLI 命令 / 不用 ▶
        assert!(!text.contains("/course -new"), "Home 不暴露 CLI 命令");
        assert!(!text.contains("▶"), "不用列表选择器箭头");
    }

    #[test]
    fn home_continue_shows_session_title_not_id() {
        let mut app = app_with_courses();
        app.sidebar_sessions = vec![storage::SessionMeta {
            id: 26,
            title: Some("Ownership & Borrowing".into()),
            course_id: Some(1),
            created_at: "2026-09-02 10:00:00".into(),
        }];
        let text = flatten(&home_lines(&app)).join("\n");
        assert!(text.contains("Ownership & Borrowing"), "显示人类可读标题");
        assert!(!text.contains("#26"), "不显示 session ID");
        assert!(text.contains("[ Continue ]"), "显示 Continue 按钮");
        assert!(text.contains("Last studied"), "显示相对时间");
    }

    #[test]
    fn home_meaningless_session_title_falls_back() {
        let mut app = app_with_courses();
        app.sidebar_sessions = vec![storage::SessionMeta {
            id: 3,
            title: Some("你好".into()),
            course_id: Some(1),
            created_at: "2026-09-02 10:00:00".into(),
        }];
        let text = flatten(&home_lines(&app)).join("\n");
        assert!(!text.contains("你好"), "无意义测试标题不得作为首页主标题");
        assert!(text.contains("Recent conversation"), "fallback 文案");
    }

    #[test]
    fn home_welcome_when_no_courses() {
        let app = App::new(
            std::sync::Arc::new(
                agent_providers::OpenAiClient::new(agent_providers::ProviderConfig {
                    name: "t".into(),
                    endpoint: "http://localhost".into(),
                    api_key: Some("k".into()),
                    api_key_env: None,
                    model: "m".into(),
                    price_prompt: Some(0.0),
                    price_completion: Some(0.0),
                    price_prompt_cached: Some(0.0),
                    context_length: 1000,
                    thinking: false,
                })
                .unwrap(),
            ),
            std::sync::Arc::new(storage::Store::open_in_memory().unwrap()),
            agent_providers::ProviderConfig {
                name: "t".into(),
                endpoint: "http://localhost".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                model: "m".into(),
                price_prompt: Some(0.0),
                price_completion: Some(0.0),
                price_prompt_cached: Some(0.0),
                context_length: 1000,
                thinking: false,
            },
            vec![],
            5.0,
            vec![],
        );
        let text = flatten(&home_lines(&app)).join("\n");
        assert!(text.contains("Welcome."));
        assert!(text.contains("Create a course to get started."));
        assert!(text.contains("+ New Course"));
        assert!(!text.contains("Your courses"), "空库不显示课程区");
    }

    /// 回归：AI 待配置（首启）时，「Set up AI」与「+ New Course」索引互斥——光标只高亮一项。
    fn app_needs_setup() -> App {
        App::new(
            std::sync::Arc::new(
                agent_providers::OpenAiClient::new(agent_providers::ProviderConfig {
                    name: "t".into(),
                    endpoint: "http://localhost".into(),
                    api_key: None,
                    api_key_env: None,
                    model: "m".into(),
                    price_prompt: Some(0.0),
                    price_completion: Some(0.0),
                    price_prompt_cached: Some(0.0),
                    context_length: 1000,
                    thinking: false,
                })
                .unwrap(),
            ),
            std::sync::Arc::new(storage::Store::open_in_memory().unwrap()),
            agent_providers::ProviderConfig {
                name: "t".into(),
                endpoint: "http://localhost".into(),
                api_key: None,
                api_key_env: None,
                model: "m".into(),
                price_prompt: Some(0.0),
                price_completion: Some(0.0),
                price_prompt_cached: Some(0.0),
                context_length: 1000,
                thinking: false,
            },
            vec![],
            5.0,
            vec![],
        )
    }

    fn line_text(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn selected_lines(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .filter(|l| {
                l.spans.iter().any(|s| {
                    s.style.fg == Some(ACCENT) && s.style.add_modifier.contains(Modifier::BOLD)
                })
            })
            .map(line_text)
            .collect()
    }

    #[test]
    fn home_setup_and_new_course_highlight_are_exclusive() {
        let app = app_needs_setup();
        assert!(app.ai_needs_setup(), "前置：无 key 需要 Setup");
        // 光标在 Set up AI（0）→ 只有 Setup 高亮，New Course 不亮
        let sel0 = selected_lines(&home_lines(&app));
        assert!(
            sel0.iter().any(|t| t.contains("Set up AI")),
            "光标 0 高亮 Setup"
        );
        assert!(
            sel0.iter().all(|t| !t.contains("New Course")),
            "光标 0 时 New Course 不得同时高亮，实际: {sel0:?}"
        );
        // 光标在 New Course（setup_offset=1）→ 只有 New Course 高亮
        let mut app1 = app_needs_setup();
        app1.home_cursor = 1;
        assert_eq!(app1.home_cursor_count(), 2, "Setup + New Course 共 2 项");
        let sel1 = selected_lines(&home_lines(&app1));
        assert!(
            sel1.iter().any(|t| t.contains("New Course")),
            "光标 1 高亮 New Course"
        );
        assert!(
            sel1.iter().all(|t| !t.contains("Set up AI")),
            "光标 1 时 Set up AI 不得同时高亮，实际: {sel1:?}"
        );
    }

    /// 人工检查：三态 Home 的实际渲染文本（headless 无法截图，用纯函数输出核对）。
    #[test]
    #[ignore = "cargo test -p tui preview_home -- --ignored --nocapture"]
    fn preview_home() {
        // 态 A：空库
        let empty = App::new(
            std::sync::Arc::new(
                agent_providers::OpenAiClient::new(agent_providers::ProviderConfig {
                    name: "t".into(),
                    endpoint: "http://localhost".into(),
                    api_key: Some("k".into()),
                    api_key_env: None,
                    model: "m".into(),
                    price_prompt: Some(0.0),
                    price_completion: Some(0.0),
                    price_prompt_cached: Some(0.0),
                    context_length: 1000,
                    thinking: false,
                })
                .unwrap(),
            ),
            std::sync::Arc::new(storage::Store::open_in_memory().unwrap()),
            agent_providers::ProviderConfig {
                name: "t".into(),
                endpoint: "http://localhost".into(),
                api_key: Some("k".into()),
                api_key_env: None,
                model: "m".into(),
                price_prompt: Some(0.0),
                price_completion: Some(0.0),
                price_prompt_cached: Some(0.0),
                context_length: 1000,
                thinking: false,
            },
            vec![],
            5.0,
            vec![],
        );
        // 态 B：有课程无 session
        let mut has_courses = app_with_courses();
        has_courses.workspace = crate::app::Workspace::Home;
        // 态 C：有课程 + 最近 session
        let mut has_session = app_with_courses();
        has_session.workspace = crate::app::Workspace::Home;
        has_session.sidebar_sessions = vec![storage::SessionMeta {
            id: 26,
            title: Some("Ownership & Borrowing".into()),
            course_id: Some(1),
            created_at: "2026-09-02 10:00:00".into(),
        }];
        for (label, app) in [
            ("A · 空库 Welcome", &empty),
            ("B · 有课程无 session", &has_courses),
            ("C · 有课程 + 最近 session", &has_session),
        ] {
            println!("\n── {label} ──");
            for l in home_lines(app) {
                let s: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                if !s.trim().is_empty() {
                    println!("  {s}");
                }
            }
        }
    }
}
