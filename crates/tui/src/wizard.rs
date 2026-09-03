//! 参数向导：多步文本输入（预填默认值），合成标准 slash 命令。

/// 参数向导：每步一个文本输入（预填默认值），最后合成标准 slash 命令提交。
/// 课程自动取当前分区，LLM 零参与（D6）；Tab 填入文本模式仍可绕过向导。
pub struct WizardStep {
    pub prompt: &'static str,
    pub default: String,
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum WizardKind {
    /// 复习出题（课程 = 打开时的当前分区 id）
    Review {
        course_id: Option<i64>,
        course_name: String,
    },
    /// 导入笔记
    Import,
    /// 重命名当前会话
    RenameSession,
    /// 重命名当前课程
    RenameCourse,
    /// 加载会话 JSON
    LoadFile,
    /// 新建课程
    CreateCourse,
    /// 设置预算上限（金额元）
    BudgetLimit,
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
    pub(crate) fn new_review(course_id: Option<i64>, course_name: String) -> Self {
        Self::build(
            WizardKind::Review {
                course_id,
                course_name: course_name.clone(),
            },
            course_name,
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
    pub(crate) fn new_for(kind: WizardKind, title: &'static str, prompt: &'static str) -> Self {
        Self::build(
            kind,
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

    pub(crate) fn kind(&self) -> &WizardKind {
        &self.kind
    }

    /// 已确认的字段值（按步骤顺序）。
    pub(crate) fn values(&self) -> Vec<String> {
        (0..self.steps.len())
            .map(|i| self.values[i].clone().unwrap_or_default())
            .collect()
    }

    pub(crate) fn course_name(&self) -> &str {
        &self.course
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
}

#[cfg(test)]
mod wizard_tests {
    use super::*;

    #[test]
    fn review_wizard_flow_values() {
        let mut w = Wizard::new_review(Some(1), "rust".into());
        assert!(!w.confirm("ownership".into())); // 推进到题数
        assert!(w.confirm("10".into())); // 最后一步
        let v = w.values();
        assert_eq!(v[0], "ownership");
        assert_eq!(v[1], "10");
    }

    #[test]
    fn review_wizard_empty_concept_keeps_defaults() {
        let mut w = Wizard::new_review(Some(2), "csapp".into());
        assert!(!w.confirm(String::new()));
        assert!(w.confirm(String::new())); // 数量留空 → 上层默认 5
    }

    #[test]
    fn import_wizard_esc_back_restores_value() {
        let mut w = Wizard::new_import("all".into());
        assert_eq!(w.steps[1].default, "", "all 分区时课程默认应为空");
        assert!(!w.confirm("~/notes".into()));
        assert_eq!(w.back().as_deref(), Some("~/notes"));
        assert!(w.back().is_none()); // 第一步再 Esc → 关闭
    }

    #[test]
    fn import_wizard_course_omitted_when_empty() {
        let mut w = Wizard::new_import("rust".into());
        assert_eq!(w.steps[1].default, "rust");
        assert!(!w.confirm("~/CSAPP".into()));
        assert!(w.confirm(String::new())); // 课程留空 → LLM 归类
    }

    #[test]
    fn prompt_wizard_rename_flow() {
        let mut w = Wizard::new_for(WizardKind::RenameSession, "重命名会话", "新标题");
        assert!(w.confirm("讲所有权".into()));
    }

    #[test]
    fn prompt_wizard_empty_input_ok() {
        let mut w = Wizard::new_for(WizardKind::LoadFile, "加载会话", "JSON 文件路径");
        assert!(w.confirm(String::new()));
    }

    #[test]
    fn kinds_are_distinct() {
        assert_ne!(
            WizardKind::Review {
                course_id: Some(1),
                course_name: "rust".into()
            },
            WizardKind::Import
        );
        assert_eq!(
            WizardKind::Review {
                course_id: Some(1),
                course_name: "rust".into()
            },
            WizardKind::Review {
                course_id: Some(1),
                course_name: "rust".into()
            }
        );
    }
}
