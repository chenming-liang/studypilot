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
    let [sidebar, chat] =
        Layout::horizontal([Constraint::Length(28), Constraint::Min(20)]).areas(main_area);

    draw_header(f, header, app);
    draw_sidebar(f, sidebar, app);
    if app.review.is_some() {
        draw_review_workspace(f, chat, app);
    } else {
        draw_chat(f, chat, app);
    }
    draw_input(f, input_area, app);

    if let Some(picker) = &app.model_picker {
        draw_model_picker(f, picker, &app.provider_cfg.name);
    }
    if app.palette.is_some() {
        draw_command_palette(f, app);
    }
    if let Some(lp) = &app.list_picker {
        draw_list_picker(f, lp);
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
    if app.toast.is_some() {
        draw_toast(f, app);
    }
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
                body.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{mark}{letter}"), Style::new().fg(color)),
                    Span::styled(format!("  {opt}"), Style::new().fg(color)),
                ]));
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
                let mut exp_lines =
                    markdown::render_markdown(exp, inner_w.saturating_sub(2).max(1));
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
        // ⑤ 选项列表：字母蓝、文字浅灰、当前 ❯ 黄
        for (i, opt) in q.options.iter().enumerate() {
            let letter = (b'A' + i as u8) as char;
            let is_cursor = rs.selected_option.unwrap_or(0) == i;
            let mark = if is_cursor { "❯ " } else { "  " };
            let letter_style = Style::new().fg(if is_cursor { theme::USER } else { ACCENT });
            let text_style = if is_cursor {
                Style::new().fg(theme::FG)
            } else {
                Style::new().fg(theme::MUTED)
            };
            body.push(Line::from(vec![
                Span::styled(
                    format!("  {mark}"),
                    Style::new().fg(if is_cursor { theme::USER } else { theme::MUTED }),
                ),
                Span::styled(format!("{letter}  "), letter_style),
                Span::styled(opt.clone(), text_style),
            ]));
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

    // 左侧 = 学习位置（我是谁、在哪门课）；右侧 = 模型 + 费用 + 状态
    let left = format!(" StudyPilot │ {}", app.course);
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
    // 分区标题：大写 + 暗色（导航语义，不抢正文注意力）
    items.push(ListItem::new(Line::from(Span::styled(
        " COURSES",
        Style::new().fg(theme::MUTED).add_modifier(Modifier::BOLD),
    ))));
    items.push(ListItem::new(Line::from(Span::styled(
        "  all",
        sidebar_style("all", &app.course),
    ))));

    for (id, name) in &app.courses {
        let is_current = *name == app.course;
        let style = sidebar_style(name, &app.course);
        let mark = if is_current { "●" } else { " " };
        items.push(ListItem::new(Line::from(Span::styled(
            format!(" {mark} {name}"),
            style,
        ))));
        if let Some((notes, concepts)) = app.sidebar_course_stats.get(id) {
            // 紧凑一行，适配窄侧栏
            items.push(ListItem::new(Line::from(Span::styled(
                format!("    {notes}n · {concepts}c"),
                Style::new().fg(DIM),
            ))));
        }
    }

    items.push(ListItem::new(Line::default())); // 空行
    items.push(ListItem::new(Line::from(Span::styled(
        " RECENT",
        Style::new().fg(theme::MUTED).add_modifier(Modifier::BOLD),
    ))));
    let row_w = area.width as usize;
    for s in app.sidebar_sessions.iter().take(6) {
        let title = s.title.as_deref().unwrap_or("(未命名)");
        // 行尾 dim 课程标注（会话发生时的分区）
        let course = app.course_label(s.course_id);
        let prefix = format!(" #{} ", s.id);
        let suffix = format!(" · {course}");
        let avail = row_w
            .saturating_sub(display_width(&prefix) + display_width(&suffix))
            .max(4);
        let short = display_truncate(title, avail);
        let ellipsis = if display_width(title) > avail {
            "…"
        } else {
            ""
        };
        // 当前打开的会话高亮（与侧栏当前课程同风格）
        let is_current = app.current_session_id() == Some(s.id);
        let title_style = if is_current {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let mark = if is_current { "▸" } else { " " };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("{mark}{prefix}{short}{ellipsis}"), title_style),
            Span::styled(suffix, Style::new().fg(DIM)),
        ])));
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
                format!("Ask {}...", app.course)
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
/// 命令面板弹窗：动态宽度、显示宽度对齐，选中项完整信息在底行展示。
fn draw_command_palette(f: &mut Frame, app: &mut App) {
    use crate::palette::{PaletteGroup, PaletteItem};
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

    // 动态宽度：列表行宽 = 标记 2 + command 显示宽 + 3 + desc 显示宽
    let cmd_w = palette
        .items
        .iter()
        .map(|i| display_width(i.command))
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
                let desc = display_truncate(item.desc, desc_avail);
                let label = format!("{mark}{}  {}", display_pad(item.command, cmd_w), desc);
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
            display_truncate(&format!("{} — {}", it.command, it.desc), width as usize - 4)
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
fn draw_list_picker(f: &mut Frame, lp: &ListPicker) {
    let area = f.area();
    let height = (lp.items.len() as u16 + 4).min(area.height.saturating_sub(2));
    let width = 52u16.min(area.width.saturating_sub(2));
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    let pop = Rect::new(x, y, width, height);

    f.render_widget(ratatui::widgets::Clear, pop);

    let items: Vec<ListItem> = lp
        .items
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mark = if i == lp.selected { "▸ " } else { "  " };
            let label = format!("{mark}{}", c.label);
            let style = if i == lp.selected {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            ListItem::new(Span::styled(label, style))
        })
        .collect();
    f.render_widget(
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
