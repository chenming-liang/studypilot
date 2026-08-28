//! 大纲渲染：缩进树 / Markdown 导出。

/// 渲染缩进树 + 末尾脚注列表。
pub(crate) fn render_outline_tree(
    outline: &serde_json::Value,
    titles: &[(i64, String)],
) -> Vec<String> {
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
pub(crate) fn render_outline_markdown(
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
