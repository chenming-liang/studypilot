//! Markdown 清洗：wikilink 剥离、目录段剔除、头部引用块去除。
//! 规划「真实语料」一节定义的清洗规则。
//!
//! 两级清洗（规划.md：content="清洗后原文（wikilink 已剥离）"；"FTS 内容应跳过目录段"）：
//! - content 层：只剥 wikilink，保留原文结构（目录段、元信息仍在，供展示）
//! - FTS 层：在 content 基础上再去目录段 + 头部引用块（检索噪声）

use regex::Regex;

static WIKILINK_PIPE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"\[\[[^\]|]+\|([^\]]+)\]\]").unwrap());

static WIKILINK_PLAIN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());

/// content 层：只剥 wikilink，保留原文结构。
pub fn clean_for_content(raw: &str) -> String {
    strip_wikilinks(raw).trim().to_owned()
}

/// FTS 层：剥 wikilink + 目录段 + 头部引用块（检索噪声）。
pub fn clean_for_fts(raw: &str) -> String {
    let text = strip_wikilinks(raw);
    let text = strip_directory_section(&text);
    let text = strip_header_quotes(&text);
    text.trim().to_owned()
}

/// 按 `##` 标题拆分 FTS 清洗后的文本为 chunk（heading + content）。
/// 标题前的引导文本归入第一个无标题 chunk。
/// 兼容 PDF 提取文本：`N.N 标题` 编号行也视作 chunk 边界（PDF 无 Markdown 标记）。
pub fn split_into_chunks(fts_text: &str) -> Vec<(String, String)> {
    static NUMBERED_HEADING: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        // 匹配 "2.1 所有权" / "10.3.2 xxx" 等 PDF 大纲编号行
        Regex::new(r"^(\d{1,2}\.\d{1,2}(\.\d)?)\s+\S").unwrap()
    });

    let mut chunks: Vec<(String, String)> = Vec::new();
    let mut current_heading = String::new();
    let mut current_body = String::new();

    for line in fts_text.lines() {
        let trimmed = line.trim_start();
        let hash_count = trimmed.chars().take_while(|c| *c == '#').count();
        let is_md_heading = (2..=6).contains(&hash_count) && {
            let after = &trimmed[hash_count..];
            after.starts_with(' ') || after.is_empty()
        };
        // PDF 编号标题：如 "2.1 所有权"（排除目录点线和纯数字）
        let is_numbered_heading = !in_toc_like(trimmed)
            && NUMBERED_HEADING.is_match(trimmed)
            && !trimmed.contains(". . .");

        if is_md_heading || is_numbered_heading {
            // 新 chunk 边界：保存上一个
            let body = current_body.trim().to_owned();
            if !body.is_empty() {
                chunks.push((current_heading.clone(), body));
            }
            current_heading = if is_md_heading {
                trimmed[hash_count..].trim().to_owned()
            } else {
                trimmed.to_owned()
            };
            current_body.clear();
            continue;
        }
        current_body.push_str(line);
        current_body.push('\n');
    }
    let body = current_body.trim().to_owned();
    if !body.is_empty() {
        chunks.push((current_heading.clone(), body));
    }
    chunks
}

/// 判断是否是目录行（大量 ". . ." 点线或连续页码）。
fn in_toc_like(line: &str) -> bool {
    line.matches(". . ").count() >= 2 || line.contains("……")
}

/// 剥离 Obsidian wikilink，保留显示文本：
/// `[[#anchor|显示]]` → `显示`；`[[path|别名]]` → `别名`；`[[path]]` → `path`。
pub fn strip_wikilinks(text: &str) -> String {
    // 先处理带 pipe 的（有显示文本），再处理无 pipe 的
    let text = WIKILINK_PIPE.replace_all(text, "$1");
    let text = WIKILINK_PLAIN.replace_all(&text, |c: &regex::Captures<'_>| {
        let inner = c.get(1).unwrap().as_str();
        // 去掉 # 前缀（Obsidian 块引用锚点）
        inner
            .rsplit('/')
            .next()
            .unwrap_or(inner)
            .trim_start_matches('#')
            .to_owned()
    });
    text.into_owned()
}

/// 剔除 `## 目录` 到下一个 `---` 或 `## ` 之间的内容。
fn strip_directory_section(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_toc = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("## 目录") || trimmed.eq_ignore_ascii_case("# 目录") {
            in_toc = true;
            continue;
        }
        if in_toc
            && (trimmed == "---"
                || (trimmed.starts_with("## ") && !trimmed.eq_ignore_ascii_case("## 目录")))
        {
            in_toc = false;
        }
        if !in_toc {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 去除文件头部（`# 标题` 之后、第一个 `##`+ 级内容标题之前）的 `> ` 引用块。
/// `# ` 是标题不算内容；`## `/`### `/… 才是内容起点。
fn strip_header_quotes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut seen_content = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        // 二级及更深标题（2~6 个 # 后接空格）标志内容区开始
        let hash_count = trimmed.chars().take_while(|c| *c == '#').count();
        if (2..=6).contains(&hash_count) {
            let after = &trimmed[hash_count..];
            if after.starts_with(' ') || after.is_empty() {
                seen_content = true;
            }
        }
        if !seen_content && trimmed.starts_with('>') {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 从 markdown 原文提取标题（第一个 `# ` 行）。
pub fn extract_title(raw: &str) -> Option<String> {
    raw.lines()
        .find(|l| l.trim_start().starts_with("# "))
        .map(|l| l.trim_start_matches("# ").trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikilink_with_pipe_strips_to_display() {
        assert_eq!(
            strip_wikilinks("[[#1. 异常与系统调用机制|异常与系统调用机制]]"),
            "异常与系统调用机制"
        );
        assert_eq!(
            strip_wikilinks("见 [[t大课程/README|返回索引]]"),
            "见 返回索引"
        );
    }

    #[test]
    fn wikilink_without_pipe_strips_path() {
        assert_eq!(strip_wikilinks("[[所有权]]"), "所有权");
        assert_eq!(strip_wikilinks("[[#总结]]"), "总结");
    }

    #[test]
    fn full_clean_strips_noise() {
        let raw = r#"# 测试笔记

> 来源：测试
> 整理日期：2026-01-01

---

## 目录

- [[#第一节|第一节]]
- [[#第二节|第二节]]

---

## 第一节

正文内容 [[#第二节|见第二节]] 结束。
"#;
        // FTS 层：全部噪声清除
        let fts = clean_for_fts(raw);
        assert!(!fts.contains("来源"), "FTS 头部引用块应剔除");
        assert!(!fts.contains("目录"), "FTS 目录段应剔除");
        assert!(!fts.contains("[["), "FTS wikilink 应剥离");
        assert!(fts.contains("正文内容"), "FTS 正文应保留");
        assert!(fts.contains("见第二节"), "FTS 显示文本应保留");

        // content 层：只剥 wikilink，保留目录段和元信息
        let content = clean_for_content(raw);
        assert!(content.contains("来源"), "content 元信息应保留");
        assert!(content.contains("目录"), "content 目录段应保留");
        assert!(!content.contains("[["), "content wikilink 应剥离");
        assert!(content.contains("见第二节"), "content 显示文本应保留");
    }

    #[test]
    fn extract_title_from_heading() {
        assert_eq!(
            extract_title("# 第 8 章：异常控制流\n\n正文"),
            Some("第 8 章：异常控制流".into())
        );
        assert_eq!(extract_title("无标题"), None);
    }
}

#[cfg(test)]
mod pdf_chunk_tests {
    use super::*;

    #[test]
    fn pdf_numbered_headings_split_chunks() {
        let pdf_text = "第 2 讲：所有权与结构化数据\n\n本讲内容\n2.1 所所有权 . . . . 2\n\n2.1 所有权\n内存资源的三种分配方式：\n- 全局对象\n- 局部对象\n2.2 借用\n借用规则：任一时刻要么多个只读，要么一个可变。";
        let chunks = split_into_chunks(pdf_text);
        // 目录行 ". . . ." 不应成为边界
        assert!(
            chunks.len() >= 2,
            "应至少拆出 2 个 chunk，实际 {}",
            chunks.len()
        );
        assert!(
            chunks.iter().any(|(h, _)| h.contains("2.1")),
            "应有编号标题 chunk"
        );
    }

    #[test]
    fn toc_dot_lines_not_boundaries() {
        let toc = "2.1 所有权 . . . . . . . . 28\n2.2 结构化数据 . . . . 50";
        let chunks = split_into_chunks(toc);
        assert_eq!(chunks.len(), 1, "目录点线不应触发拆分");
    }
}
