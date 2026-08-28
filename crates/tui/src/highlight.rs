//! 代码块语法高亮（syntect，fancy-regex 纯 Rust 后端，无 C 依赖）。
//!
//! 主题 = base16-ocean.dark（One Dark 系，与 theme.rs 语义色板同族）；
//! token 前景色直接映射 ratatui Rgb，背景沿用终端黑（不整块铺色，符合
//! "亮色面积少"的 70/20/10 原则）。未知语言回退 plain text。

use std::sync::OnceLock;

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

struct Engine {
    ps: SyntaxSet,
    theme: syntect::highlighting::Theme,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| Engine {
        ps: SyntaxSet::load_defaults_newlines(),
        theme: ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default(),
    })
}

fn find_syntax<'a>(ps: &'a SyntaxSet, lang: &str) -> &'a SyntaxReference {
    let tok = lang.trim();
    ps.find_syntax_by_token(tok)
        .or_else(|| ps.find_syntax_by_extension(tok.trim_start_matches('.')))
        .unwrap_or_else(|| ps.find_syntax_plain_text())
}

/// 高亮一段代码：返回每行 `[(前景色, 文本)]`（文本不含换行）。
/// 单行解析失败时整行回退 CODE_FG，不影响其余行。
pub fn highlight_lines(code: &str, lang: &str) -> Vec<Vec<(Color, String)>> {
    let e = engine();
    let syntax = find_syntax(&e.ps, lang);
    let mut hl = HighlightLines::new(syntax, &e.theme);
    let mut out = Vec::new();
    for line in LinesWithEndings::from(code) {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
        let spans: Vec<(Color, String)> = match hl.highlight_line(trimmed, &e.ps) {
            Ok(ranges) => ranges
                .into_iter()
                .map(|(style, text)| {
                    let c = style.foreground;
                    (Color::Rgb(c.r, c.g, c.b), text.to_owned())
                })
                .collect(),
            Err(_) => vec![(Color::Rgb(0xE5, 0xC0, 0x7B), trimmed.to_owned())],
        };
        out.push(spans);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// fence info string → 语言标签（"rust,ignore" → "rust"；空/纯空白 → None）。
pub fn lang_tag(info: &str) -> Option<String> {
    let first = info.split(',').next().unwrap_or("").trim().to_lowercase();
    if first.is_empty() { None } else { Some(first) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keywords_get_colored() {
        let lines = highlight_lines("let s1 = String::from(\"hi\");\n", "rust");
        assert_eq!(lines.len(), 1);
        // 至少出现多个不同颜色的 token（关键字/标识符/字符串分色）
        let colors: Vec<_> = lines[0].iter().map(|(c, _)| *c).collect();
        assert!(colors.len() >= 3, "token 未分色: {colors:?}");
        // 关键字 let 应为紫系（base16-ocean keyword #B48EAD）
        let let_color = lines[0]
            .iter()
            .find(|(_, t)| t == "let")
            .map(|(c, _)| *c)
            .expect("应有 let token");
        assert_eq!(let_color, Color::Rgb(0xB4, 0x8E, 0xAD));
    }

    #[test]
    fn unknown_lang_falls_back_to_plain() {
        let lines = highlight_lines("hello\n", "not-a-lang");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn empty_code_yields_one_row() {
        assert_eq!(highlight_lines("", "rust").len(), 1);
    }

    #[test]
    fn lang_tag_extraction() {
        assert_eq!(lang_tag("rust,ignore"), Some("rust".into()));
        assert_eq!(lang_tag("  Python "), Some("python".into()));
        assert_eq!(lang_tag(""), None);
    }
}
