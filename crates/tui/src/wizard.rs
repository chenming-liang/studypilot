//! 参数向导：多步文本输入（预填默认值），合成标准 slash 命令。

/// 参数向导：每步一个文本输入（预填默认值），最后合成标准 slash 命令提交。
/// 课程自动取当前分区，LLM 零参与（D6）；Tab 填入文本模式仍可绕过向导。
pub struct WizardStep {
    pub prompt: &'static str,
    pub default: String,
}

#[derive(Clone, PartialEq, Debug)]
enum WizardKind {
    Review,
    Import,
    /// 单步自由文本：完成后合成 `{command} {输入}`
    Plain {
        command: String,
    },
}

pub struct Wizard {
    kind: WizardKind,
    /// 当前分区（合成命令时使用）
    course: String,
    pub title: String,
    pub steps: Vec<WizardStep>,
    pub current: usize,
    /// 每步已确认的值（Enter 写入，Esc 回退保留）
    values: Vec<Option<String>>,
}

impl Wizard {
    pub(crate) fn new_review(course: String) -> Self {
        Self::build(
            WizardKind::Review,
            course,
            "复习出题".to_owned(),
            vec![
                WizardStep {
                    prompt: "概念关键词（留空 = 全部概念）",
                    default: String::new(),
                },
                WizardStep {
                    prompt: "题目数量",
                    default: "5".to_owned(),
                },
            ],
        )
    }

    pub(crate) fn new_import(course: String) -> Self {
        let default_course = if course == "all" { "" } else { course.as_str() };
        let steps = vec![
            WizardStep {
                prompt: "目录路径（md/pdf/pptx）",
                default: String::new(),
            },
            WizardStep {
                prompt: "课程（留空 = LLM 归类）",
                default: default_course.to_owned(),
            },
        ];
        Self::build(WizardKind::Import, course, "导入笔记".to_owned(), steps)
    }

    /// 单步自由文本向导（palette Prompt 动作：/rename /load /course -new）。
    pub(crate) fn new_prompt(title: &'static str, prompt: &'static str, command: &str) -> Self {
        Self::build(
            WizardKind::Plain {
                command: command.to_owned(),
            },
            String::new(),
            title.to_owned(),
            vec![WizardStep {
                prompt,
                default: String::new(),
            }],
        )
    }

    fn build(kind: WizardKind, course: String, title: String, steps: Vec<WizardStep>) -> Self {
        Self {
            kind,
            course,
            title,
            values: vec![None; steps.len()],
            steps,
            current: 0,
        }
    }

    /// Enter：确认当前步（值由聊天框缓冲传入）。返回 true 表示已到最后一步。
    pub(crate) fn confirm(&mut self, value: String) -> bool {
        self.values[self.current] = Some(value);
        if self.current + 1 < self.steps.len() {
            self.current += 1;
            false
        } else {
            true
        }
    }

    /// Esc：回退上一步。返回 Some(该步恢复值)；None = 已在第一步（应关闭向导）。
    pub(crate) fn back(&mut self) -> Option<String> {
        if self.current == 0 {
            return None;
        }
        self.current -= 1;
        Some(
            self.values[self.current]
                .clone()
                .unwrap_or_else(|| self.steps[self.current].default.clone()),
        )
    }

    /// 合成最终命令（走 handle_command 的 D6 直连路径）。
    pub(crate) fn command(&self) -> String {
        let v: Vec<String> = (0..self.steps.len())
            .map(|i| self.values[i].clone().unwrap_or_default())
            .collect();
        match &self.kind {
            WizardKind::Review => {
                let concept = v[0].trim();
                let n = v[1].trim().parse::<usize>().unwrap_or(5);
                let scope = if concept.is_empty() {
                    self.course.clone()
                } else {
                    format!("{} {concept}", self.course)
                };
                format!("/review {scope} --n {n}")
            }
            WizardKind::Import => {
                let dir = v[0].trim().trim_matches('"');
                let course = v[1].trim();
                if course.is_empty() {
                    format!("/import {dir}")
                } else {
                    format!("/import {dir} --course {course}")
                }
            }
            WizardKind::Plain { command } => {
                let command = command.clone();
                let arg = v[0].trim();
                if arg.is_empty() {
                    command
                } else {
                    format!("{command} {arg}")
                }
            }
        }
    }
}

#[cfg(test)]
mod wizard_tests {
    use super::{Wizard, WizardKind};

    #[test]
    fn review_wizard_flow_and_command() {
        let mut w = Wizard::new_review("rust".into());
        // 概念默认空（默认值由调用方写入聊天框缓冲）
        assert!(!w.confirm("ownership".into())); // 推进到题数
        assert!(w.confirm("10".into())); // 最后一步
        assert_eq!(w.command(), "/review rust ownership --n 10");
    }

    #[test]
    fn review_wizard_empty_concept_and_default_n() {
        let mut w = Wizard::new_review("csapp".into());
        assert!(!w.confirm(String::new()));
        assert!(w.confirm(String::new())); // 数量留空 → 默认 5
        assert_eq!(w.command(), "/review csapp --n 5");
    }

    #[test]
    fn import_wizard_esc_back_restores_value() {
        let mut w = Wizard::new_import("all".into());
        assert_eq!(w.steps[1].default, "", "all 分区时课程默认应为空");
        assert!(!w.confirm("~/notes".into()));
        // Esc 回退到目录步并恢复已填值
        assert_eq!(w.back().as_deref(), Some("~/notes"));
        assert!(w.back().is_none()); // 第一步再 Esc → 关闭
    }

    #[test]
    fn import_wizard_course_omitted_when_empty() {
        let mut w = Wizard::new_import("rust".into());
        assert_eq!(w.steps[1].default, "rust");
        assert!(!w.confirm("~/CSAPP".into()));
        assert!(w.confirm(String::new())); // 课程留空 → LLM 归类
        assert_eq!(w.command(), "/import ~/CSAPP");
    }

    #[test]
    fn review_rejects_non_numeric_n_gracefully() {
        let mut w = Wizard::new_review("rust".into());
        assert!(!w.confirm(String::new()));
        assert!(w.confirm("abc".into()));
        assert!(w.command().ends_with("--n 5"), "{}", w.command());
    }

    #[test]
    fn kind_discrimination() {
        let a = Wizard::new_review("rust".into());
        let b = Wizard::new_import("rust".into());
        assert_eq!(a.title, "复习出题");
        assert_eq!(b.title, "导入笔记");
        assert_ne!(a.kind, b.kind);
        let _ = WizardKind::Review;
    }

    #[test]
    fn plain_prompt_wizard_completes_command() {
        let mut w = Wizard::new_prompt("重命名会话", "新标题", "/rename");
        assert!(w.confirm("讲所有权".into()));
        assert_eq!(w.command(), "/rename 讲所有权");
    }

    #[test]
    fn plain_prompt_empty_input_keeps_bare_command() {
        let mut w = Wizard::new_prompt("加载会话", "JSON 文件路径", "/load");
        assert!(w.confirm(String::new()));
        assert_eq!(w.command(), "/load");
    }
}
