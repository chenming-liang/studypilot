//! Markdown → ratatui Line 渲染器。
//! pulldown-cmark 解析 AST → 按元素类型映射到 Line/Span + 色彩分层。
//! 不做语法高亮（syntect 太重），代码块只改背景色 + 前景色。

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme;
// Markdown 语义：标题=Secondary 紫（层级渐淡）、正文=FG 浅灰、
// 行内代码=暖白、[n] 引用=Reference 青（RAG 品牌色）
const HEAD: Color = theme::SECONDARY;
const HEAD2: Color = theme::SECONDARY_DIM;
const BODY: Color = theme::FG;
const REF: Color = theme::REFERENCE;

/// 把 Markdown 文本渲染为 ratatui Line 列表（已按显示宽度折行由调用方处理）。
pub fn render_markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(text, opts);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current = LineBuffer::new();
    let mut in_code_block = false;
    let mut heading_level: u32 = 0;

    let mut list_depth = 0usize;
    // 跨事件文本聚合：pulldown 会把 [n] 拆成 "["、"n"、"]" 多个独立 Text 事件，
    // 直接逐事件推送永远拼不出完整括号对；聚合到下一个非文本事件再统一解析。
    let mut text_acc = String::new();

    // 结构事件边界：把已聚合的文本按当前状态灌入缓冲。
    // 必须发生在状态翻转之前——否则刚离开的代码块正文会被误走 [n] 解析路径。
    macro_rules! drain_text {
        () => {
            if !text_acc.is_empty() {
                let s = std::mem::take(&mut text_acc);
                if in_code_block || heading_level > 0 {
                    current.push_str(&s);
                } else {
                    current.push_text_with_refs(&s);
                }
            }
        };
    }

    // 代码块上下文：Start 记录语言标签，Text 累积进独立缓冲（不走 [n] 解析），
    // End 拦截整块 → syntect 高亮 → 逐 token 染色渲染
    let mut code_lang: Option<String> = None;
    let mut code_buf = String::new();

    for event in parser {
        // 代码块结束：先于通用 drain 拦截——代码文本直接走高亮路径
        if matches!(&event, Event::End(TagEnd::CodeBlock)) {
            let code = std::mem::take(&mut code_buf);
            let lang = code_lang.take().unwrap_or_default();
            in_code_block = false;
            // 块首语言标签 + 高亮行 + 块尾分隔线（GPT 第五节"轻量块"）
            if let Some(tag) = crate::highlight::lang_tag(&lang) {
                lines.push(Line::from(Span::styled(
                    format!("── {tag} ──"),
                    Style::new().fg(theme::MUTED),
                )));
            }
            for hl_row in crate::highlight::highlight_lines(&code, &lang) {
                let spans: Vec<Span<'static>> =
                    std::iter::once(Span::styled("  ".to_owned(), Style::new().fg(theme::MUTED)))
                        .chain(
                            hl_row
                                .into_iter()
                                .map(|(c, t)| Span::styled(t, Style::new().fg(c))),
                        )
                        .collect();
                lines.push(Line::from(spans));
            }
            lines.push(Line::from(Span::styled(
                "────────────".to_owned(),
                Style::new().fg(theme::MUTED),
            )));
            lines.push(Line::default());
            continue;
        }
        if let Event::Text(t) = &event {
            // 代码块内文本累积进独立缓冲（不走普通 [n] 聚合）
            if in_code_block {
                code_buf.push_str(t);
            } else {
                text_acc.push_str(t);
            }
            continue;
        }
        drain_text!();

        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading_level = level as u32;
                current.clear();
            }
            Event::End(TagEnd::Heading(level)) => {
                // 层级视觉：H1 紫 + 粗；H2+ 淡紫 + 粗（自然形成 紫→淡紫→正文 渐变）
                let color = if level as u32 == 1 { HEAD } else { HEAD2 };
                let style = Style::new().fg(color).add_modifier(Modifier::BOLD);
                for l in current.flush_as_lines(width, style) {
                    lines.push(l);
                }
                heading_level = 0;
                current.clear();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(info) => {
                        crate::highlight::lang_tag(&info)
                    }
                    pulldown_cmark::CodeBlockKind::Indented => None,
                };
            }
            Event::Code(code_text) => {
                // 行内代码：无背景、纯 fg 染色（Orange）——DarkGray 打底在黑底上突兀
                current.push_style(Style::new().fg(theme::CODE_INLINE));
                current.push_str(&code_text);
                current.pop_style();
            }
            Event::Start(Tag::Paragraph) => {
                current.clear();
            }
            Event::End(TagEnd::Paragraph) => {
                let style = Style::new().fg(BODY);
                for l in current.flush_as_lines(width, style) {
                    lines.push(l);
                }
                lines.push(Line::default());
                current.clear();
            }
            Event::Start(Tag::List(_)) => {
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
            }
            Event::Start(Tag::Item) => {
                current.clear();
                current.push_str(&"  ".repeat(list_depth.saturating_sub(1)));
                current.push_str("• ");
            }
            Event::End(TagEnd::Item) => {
                let style = Style::new().fg(BODY);
                for l in current.flush_as_lines(width, style) {
                    lines.push(l);
                }
                current.clear();
            }
            Event::Start(Tag::Strong) => {
                current.toggle_bold();
            }
            Event::End(TagEnd::Strong) => {
                current.toggle_bold();
            }
            Event::Start(Tag::Emphasis) => {
                current.toggle_italic();
            }
            Event::End(TagEnd::Emphasis) => {
                current.toggle_italic();
            }
            Event::SoftBreak | Event::HardBreak => {
                drain_text!();
                current.newline();
            }
            _ => {}
        }
    }

    // 残余内容：先冲刷聚合文本，再刷缓冲
    drain_text!();
    if !current.is_empty() {
        let style = if heading_level > 0 {
            Style::new().fg(HEAD).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(BODY)
        };
        for l in current.flush_as_lines(width, style) {
            lines.push(l);
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            text.to_owned(),
            Style::new().fg(BODY),
        )));
    }

    lines
}

/// 临时缓冲区：收集文本 + 样式状态，最终 flush 为折行的 Line 列表。
struct LineBuffer {
    spans: Vec<(String, StyleState)>,
    bold: bool,
    italic: bool,
    style_stack: Vec<Style>,
}

#[derive(Clone, Debug, PartialEq)]
struct StyleState {
    bold: bool,
    italic: bool,
    extra: Option<Style>,
}

impl LineBuffer {
    fn new() -> Self {
        Self {
            spans: Vec::new(),
            bold: false,
            italic: false,
            style_stack: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.spans.clear();
    }

    fn is_empty(&self) -> bool {
        self.spans.iter().all(|(s, _)| s.is_empty())
    }

    fn push_str(&mut self, text: &str) {
        let state = StyleState {
            bold: self.bold,
            italic: self.italic,
            extra: self.style_stack.last().copied(),
        };
        if let Some(last) = self.spans.last_mut()
            && last.1 == state
        {
            last.0.push_str(text);
            return;
        }
        self.spans.push((text.to_owned(), state));
    }

    fn toggle_bold(&mut self) {
        self.bold = !self.bold;
    }

    fn toggle_italic(&mut self) {
        self.italic = !self.italic;
    }

    fn push_style(&mut self, style: Style) {
        self.style_stack.push(style);
    }

    fn pop_style(&mut self) {
        self.style_stack.pop();
    }

    fn newline(&mut self) {
        self.push_str("\n");
    }

    /// 推送正文文本，检测 [n] 引用标记并涂蓝色（只推一遍，不重复）。
    fn push_text_with_refs(&mut self, text: &str) {
        let mut remaining = text;
        while let Some(pos) = remaining.find('[') {
            if pos > 0 {
                self.push_str(&remaining[..pos]);
            }
            let after = &remaining[pos..];
            if let Some(end) = after.find(']') {
                let bracket_content = &after[1..end];
                if bracket_content.chars().all(|c| c.is_ascii_digit())
                    && !bracket_content.is_empty()
                {
                    // [n] 引用标记 → 蓝色
                    self.push_style(Style::new().fg(REF));
                    self.push_str(format!("[{bracket_content}]").as_str());
                    self.pop_style();
                    remaining = &after[end + 1..];
                    continue;
                }
            }
            // 不是引用标记，原样输出
            self.push_str("[");
            remaining = &after[1..];
        }
        if !remaining.is_empty() {
            self.push_str(remaining);
        }
    }

    /// 把已收集的带样式 spans 渲染为折行后的 Line 列表。
    /// 以字符为单位按显示宽度流式折行，跨 span 保留各自样式——
    /// bold/italic/行内代码/`[n]` 引用着色都在最终 Line 上生效。
    fn flush_as_lines(&self, width: usize, base_style: Style) -> Vec<Line<'static>> {
        if self.spans.is_empty() {
            return Vec::new();
        }

        // ① 展开为逐字符样式流：base 提供块级前景色，span 状态叠加修饰、覆盖着色
        let stream: Vec<(char, Style)> = self
            .spans
            .iter()
            .flat_map(|(text, state)| {
                let mut st = base_style;
                if state.bold {
                    st = st.add_modifier(Modifier::BOLD);
                }
                if state.italic {
                    st = st.add_modifier(Modifier::ITALIC);
                }
                // 行内 code / [n] 引用的专用配色覆盖正文色
                if let Some(extra) = state.extra {
                    st = st.patch(extra);
                }
                text.chars().map(move |ch| (ch, st))
            })
            .collect();

        // ② 按 '\n' 切逻辑段（SoftBreak/HardBreak 产生；段内折行保持样式）
        let mut segments: Vec<&[(char, Style)]> = Vec::new();
        let mut start = 0usize;
        for (i, &(ch, _)) in stream.iter().enumerate() {
            if ch == '\n' {
                segments.push(&stream[start..i]);
                start = i + 1;
            }
        }
        segments.push(&stream[start..]);

        // ③ 每段按显示宽度折行；同样式相邻字符合并为同一 Span
        let mut out = Vec::new();
        for (si, seg) in segments.iter().enumerate() {
            // 原实现语义：首空行保留，其余内部空行跳过
            if si > 0 && seg.is_empty() {
                continue;
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut wsum = 0usize;
            for &(ch, st) in *seg {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if cw > 0 && wsum + cw > width && wsum > 0 {
                    out.push(Line::from(std::mem::take(&mut spans)));
                    wsum = 0;
                }
                match spans.last_mut() {
                    Some(last) if last.style == st => last.content.to_mut().push(ch),
                    _ => spans.push(Span::styled(String::from(ch), st)),
                }
                wsum += cw;
            }
            out.push(Line::from(spans));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 折叠全部 span 文本便于断言内容不丢不重。
    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn bold_survives_flushing() {
        let lines = render_markdown("plain **bold part** end", 40);
        // 段落结束会追加一个默认空行，join 后带尾换行
        assert_eq!(plain(&lines).trim_end_matches('\n'), "plain bold part end");
        let has_bold_span = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content == "bold part" && s.style.add_modifier.contains(Modifier::BOLD))
        });
        assert!(has_bold_span, "加粗修饰丢失: {lines:?}");
    }

    #[test]
    fn ref_marker_is_blue_across_wrap() {
        // [1] 恰好落在折行边界前后也应保持蓝色（走同一样式合并路径）
        let long_text = format!("{}{}", "字".repeat(28), "引用见[1]结束");
        let lines = render_markdown(&long_text, 30);
        let blue_found = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content.contains("[1]") && s.style.fg == Some(REF))
        });
        assert!(blue_found, "[n] 引用未涂蓝: {lines:?}");
    }

    #[test]
    fn ref_marker_detected_across_pulldown_event_splits() {
        // pulldown 把 [7] 发成 "["、"7"、"]" 独立 Text 事件；聚合后必须仍能识别
        let lines = render_markdown("前[7]后", 60);
        let blue_found = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content == "[7]" && s.style.fg == Some(REF))
        });
        assert!(blue_found, "跨事件拆分的 [n] 未识别: {lines:?}");
    }

    #[test]
    fn inline_code_gets_dedicated_fg_no_bg() {
        // 行内代码：无背景 + One Dark 橙前景（DarkGray 打底在黑底终端上刺眼，已移除）
        let lines = render_markdown("使用 `let x = 1;` 声明", 60);
        let code_styled = lines.iter().any(|l| {
            l.spans.iter().any(|s| {
                s.content.contains("let x")
                    && s.style.fg == Some(theme::CODE_INLINE)
                    && s.style.bg.is_none()
            })
        });
        assert!(code_styled, "行内代码应无背景且橙色前景: {lines:?}");
    }

    #[test]
    fn wrapping_splits_long_paragraph_by_width() {
        let lines = render_markdown(&"甲".repeat(10), 4);
        // 尾部是段落结束的空行；10 个宽 2 字符按宽 4 → 每 2 字一行共 5 行
        let rows: Vec<_> = lines
            .iter()
            .filter(|l| plain(std::slice::from_ref(l)).chars().count() > 0)
            .collect();
        assert_eq!(rows.len(), 5, "折行数不符: {lines:?}");
        assert!(
            rows.iter()
                .all(|l| plain(std::slice::from_ref(l)).chars().count() == 2),
            "每行应为宽 4（2 个全角字符）"
        );
    }
}

#[cfg(test)]
mod codeblock_tests {
    use super::*;

    #[test]
    fn fenced_rust_block_is_highlighted() {
        let lines = render_markdown("```rust\nlet x = 1;\n```\n", 80);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // 语言标签行 + 代码行 + 分隔线
        assert!(text.iter().any(|t| t.contains("── rust ──")), "{text:?}");
        let code_row = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("let")))
            .expect("代码行应存在");
        // let 关键字 token 应为紫系（base16-ocean keyword）
        let has_keyword_color = code_row
            .spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Rgb(0xB4, 0x8E, 0xAD)) && s.content.contains("let"));
        assert!(has_keyword_color, "let 未按关键字染色: {code_row:?}");
        // 分隔线
        assert!(text.iter().any(|t| t.starts_with("───")));
    }

    #[test]
    fn unknown_lang_still_renders() {
        let lines = render_markdown("```\nplain text\n```\n", 80);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("plain text")))
        );
    }
}
