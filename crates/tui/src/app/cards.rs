//! 产品化引导卡片（纯函数，供测试）。
//!
//! 三张卡共用 `Entry::Markdown` 渲染管线，视觉语言与 Review/Outline 卡片一致：
//! - Welcome Guide：空库首启（course 数 == 0）的产品入口卡
//! - Course Summary：进入/创建课程后的上下文反馈卡（反映真实库状态）
//! - Empty State：空课程上执行 /review /outline 时的行动引导卡
//!
//! 全部数据来自现有 course_stats / concepts mastery / sessions 查询，
//! 不新增表、统计系统或 first_run 类状态（决策：课程数为 0 即"尚未使用"）。

/// 空课程引导检测标记（presentation 层匹配既有引擎错误文案，不侵入引擎）。
pub(crate) const EMPTY_COURSE_MARKER: &str = "还没有笔记";
/// 有笔记但概念未抽取时的引导检测标记（/outline 路径）。
pub(crate) const NO_CONCEPT_MARKER: &str = "还没有概念";

/// 空库首启欢迎卡。
pub(crate) fn welcome_guide() -> String {
    [
        "# Welcome to StudyPilot",
        "",
        "Learn from your own materials.",
        "",
        "Get started:",
        "",
        "**1. Create a course** — `/course -new <course name>`",
        "",
        "**2. Import your materials** — `/import --dir <path>`",
        "",
        "**3. Ask questions** about your materials",
        "",
        "**4. Review what you've learned** — `/review` · `/review-map`",
        "",
        "Tip: Type `/help` anytime to see all commands.",
    ]
    .join("\n")
}

/// 进入/创建课程后的上下文卡（三态自适应，反映真实库状态）。
///
/// - 0 材料：`No materials yet` + 导入引导
/// - 有材料但无学习 session：`Your course is ready` + Get started 动作行
/// - 有最近 session：`**Continue:** {标题}` + 下一步命令（突出继续，非 KPI）
///
/// `weak` = 需巩固概念数（有作答且累计正确率 <70%）。
/// `last_session` 为 None 时不渲染 Continue 行（不造假数据）。
pub(crate) fn course_summary(
    course: &str,
    notes: usize,
    concepts: usize,
    weak: usize,
    last_session: Option<&str>,
) -> String {
    let mut lines = vec![format!("# {course}"), String::new()];
    if notes == 0 && concepts == 0 {
        // 0 材料态
        lines.push("0 notes · 0 concepts".into());
        lines.push(String::new());
        lines.push("No materials yet.".into());
        lines.push(String::new());
        lines.push("`/import --dir <path>` to add your first material.".into());
    } else {
        // 统计只是上下文，不放大成 KPI
        lines.push(format!(
            "{notes} notes · {concepts} concepts · △ {weak} need reinforcement"
        ));
        lines.push(String::new());
        if let Some(title) = last_session {
            // 已有学习历史 → 突出"继续"
            lines.push(format!("**Continue:** {title}"));
            lines.push(String::new());
            lines.push("`/review` · `/review-map` · `/outline` · `/import`".into());
        } else {
            // 有材料但尚无 session → 空态引导
            lines.push("Your course is ready.".into());
            lines.push(String::new());
            lines.push("**Ask** about your materials".into());
            lines.push("`/review` to practice".into());
            lines.push("`/outline` to view your knowledge map".into());
            lines.push("`/import` to add more materials".into());
        }
    }
    lines.join("\n")
}

/// /review 空课程引导。
pub(crate) fn empty_review() -> String {
    [
        "# Nothing to review yet",
        "",
        "This course doesn't have any materials yet.",
        "",
        "Import learning materials first:",
        "",
        "`/import --dir <path>`",
        "",
        "After importing, StudyPilot can build your knowledge map and generate reviews.",
    ]
    .join("\n")
}

/// /outline 空课程引导。
pub(crate) fn empty_outline() -> String {
    [
        "# Nothing to outline yet",
        "",
        "There is nothing to outline yet.",
        "",
        "Import some learning materials first:",
        "",
        "`/import --dir <path>`",
        "",
        "After importing, StudyPilot can build your knowledge map.",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::render_markdown;

    /// 折叠渲染结果便于断言内容不丢不重。
    fn plain(md: &str) -> String {
        render_markdown(md, 60)
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

    /// 卡片渲染不吞内容：`<course name>` 这类尖括号不得被当作 inline HTML 丢弃。
    #[test]
    fn cards_render_preserve_all_content() {
        let w = plain(&welcome_guide());
        for needle in [
            "StudyPilot",
            "Create a course",
            "<course name>",
            "<path>",
            "/help",
        ] {
            assert!(w.contains(needle), "welcome 渲染丢内容: {needle}\n{w}");
        }

        let c = plain(&course_summary(
            "Rust",
            5,
            65,
            3,
            Some("Ownership & Borrowing"),
        ));
        for needle in [
            "Rust",
            "5 notes",
            "△ 3 need reinforcement",
            "Continue",
            "Ownership & Borrowing",
            "/review-map",
        ] {
            assert!(c.contains(needle), "summary 渲染丢内容: {needle}\n{c}");
        }

        // 有材料但无 session：Get started 引导态（渲染后 inline code 剥掉反引号）
        let ready = plain(&course_summary("Rust", 5, 65, 0, None));
        for needle in ["Your course is ready", "/review to practice", "/import"] {
            assert!(
                ready.contains(needle),
                "ready 渲染丢内容: {needle}\n{ready}"
            );
        }

        for md in [empty_review(), empty_outline()] {
            let p = plain(&md);
            assert!(p.contains("<path>"), "empty 渲染丢内容:\n{p}");
        }
    }

    /// 首屏视觉检查（人工 + 保真断言）：实际运行所见的渲染内容。
    #[test]
    #[ignore = "cargo test -p tui preview_screen -- --ignored --nocapture"]
    fn preview_screen() {
        for (label, md) in [
            ("WELCOME · first launch (0 courses)", welcome_guide()),
            (
                "CONTEXT CARD · restart into existing course",
                course_summary("rust", 5, 65, 3, Some("Ownership & Borrowing")),
            ),
            (
                "CONTEXT CARD · ready, no session yet",
                course_summary("rust", 5, 65, 0, None),
            ),
            ("EMPTY STATE · /review on empty course", empty_review()),
        ] {
            println!("\n── {label} ──");
            for l in render_markdown(&md, 44) {
                let s: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                println!("  {s}");
            }
        }
    }
}
