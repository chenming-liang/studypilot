//! 命令面板、列表选择器与模型弹窗的数据结构。

use agent_providers::ProviderConfig;

/// 模型选择弹窗状态。
pub struct ModelPicker {
    pub options: Vec<ProviderConfig>,
    pub selected: usize,
}

/// 面板分组（渲染为组头分隔行；过滤时隐藏）。顺序即显示顺序。
/// 产品化分组：STUDY / COURSE / SETTINGS / HELP（文档 §4）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteGroup {
    Study,
    Course,
    Settings,
    Help,
}

impl PaletteGroup {
    pub fn label(&self) -> &'static str {
        match self {
            PaletteGroup::Study => "学习",
            PaletteGroup::Course => "课程",
            PaletteGroup::Settings => "设置",
            PaletteGroup::Help => "帮助",
        }
    }
}

/// 命令作用域（文档 §6）：决定 palette 中 scope 标签的展示。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteScope {
    /// /model /budget /help——任何页面可见
    Global,
    /// 绑定当前课程（Review/Outline/Review Map/Materials/Import…）
    CurrentCourse,
    /// 绑定当前会话（Rename Session / Export Conversation）
    CurrentSession,
}

/// 面板条目的 Enter 行为（Tab 一律填入文本模式，power-user 兜底）。
#[derive(Clone)]
pub enum PaletteAction {
    /// 无参命令，直接执行
    Run,
    /// /review 参数向导
    WizardReview,
    /// /import 参数向导
    WizardImport,
    /// 单步文本向导：(弹窗标题, 输入提示, 向导种类)——完成时直连内部函数
    Prompt {
        title: &'static str,
        prompt: &'static str,
        kind: crate::wizard::WizardKind,
    },
    /// 列表选择器
    Pick(PickKind),
}

#[derive(Clone, Copy)]
pub enum PickKind {
    /// 切换课程分区（all + 全部课程）
    CourseSwitch,
    /// 删除课程（仅现有课程）
    CourseDelete,
    /// 复习地图：选知识点开复习
    ReviewMap,
}

/// 命令面板条目：行为直接绑在条目上（不再是命令字符串前缀分流）。
pub struct PaletteItem {
    /// 人类可读动作名（主 UI 展示，如 "Review"；不用 slash 语法）
    pub label: &'static str,
    /// CLI 命令文本（Tab 填入文本模式的 power-user 兜底；无 = 不可 Tab 填充）
    pub command: &'static str,
    pub desc: &'static str,
    pub action: PaletteAction,
    pub group: PaletteGroup,
    pub scope: PaletteScope,
}

/// 通用列表选择器：单列 label，Enter 提交绑定的完整命令。
/// 过滤 = 共享输入缓冲（App.input，复用 /notes /sessions 的搜索交互——
/// 底部输入框即搜索栏，可见可编辑），selected 指向过滤后列表的位置。
pub struct ListPicker {
    pub title: String,
    pub items: Vec<ListChoice>,
    pub selected: usize,
}

impl ListPicker {
    /// 过滤后的可见条目（input 为空 = 全部；label 大小写不敏感包含匹配）。
    pub fn visible(&self, input: &str) -> Vec<usize> {
        if input.is_empty() {
            return (0..self.items.len()).collect();
        }
        let f = input.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, c)| c.label.to_lowercase().contains(&f))
            .map(|(i, _)| i)
            .collect()
    }
}

pub struct ListChoice {
    /// 展示文本
    pub label: String,
    /// 选中后提交的完整命令（power-user 兼容；动作优先于命令）
    pub command: String,
    /// 直接执行的动作（不走命令字符串→parser→handler 的 command-centric 路径）
    pub action: Option<ListChoiceAction>,
}

/// 列表选择器选中项的直接动作（canonical application action，文档 §22/23）。
/// UI → Action，不再"拼命令字符串→解析→handler"。
#[derive(Clone)]
pub enum ListChoiceAction {
    /// 切换到课程（id；None = all 区）
    SwitchCourse(Option<i64>),
    /// 删除课程
    DeleteCourse(i64),
}

/// 命令面板（Ctrl+K）：可发现性入口——按功能分组展示。
/// 过滤串 = App.input（覆盖层与聊天框共享同一缓冲，无独立状态）。
pub struct CommandPalette {
    pub items: Vec<PaletteItem>,
    /// 过滤后命中的 items 下标（按分组顺序）
    pub filtered: Vec<usize>,
    pub selected: usize,
    /// 列表滚动起点（条目数超过视口时）
    pub scroll: usize,
}

impl CommandPalette {
    /// 全量命令表（书写顺序 = 分组顺序 = 显示顺序）。
    /// 产品化：label 人类可读、scope 标注、分组 CONTINUE/STUDY/COURSE/SETTINGS/HELP。
    fn all_items() -> Vec<PaletteItem> {
        use PaletteAction as A;
        use PaletteGroup as G;
        use PaletteItem as P;
        use PaletteScope as S;
        vec![
            // ---- 学习 ----
            P {
                label: "New Conversation",
                command: "/new",
                desc: "当前课程开新会话",
                action: A::Run,
                group: G::Study,
                scope: S::CurrentCourse,
            },
            P {
                label: "Review",
                command: "/review",
                desc: "出题复习（掌握度低优先）",
                action: A::WizardReview,
                group: G::Study,
                scope: S::CurrentCourse,
            },
            P {
                label: "Review Map",
                command: "/review-map",
                desc: "复习地图：选知识点开复习",
                action: A::Run,
                group: G::Study,
                scope: S::CurrentCourse,
            },
            P {
                label: "Outline",
                command: "/outline",
                desc: "生成课程知识大纲",
                action: A::Run,
                group: G::Study,
                scope: S::CurrentCourse,
            },
            P {
                label: "Outline → Export",
                command: "/outline --export",
                desc: "生成大纲并导出 Markdown",
                action: A::Run,
                group: G::Study,
                scope: S::CurrentCourse,
            },
            P {
                label: "Materials",
                command: "/notes",
                desc: "浏览与管理当前课程笔记",
                action: A::Run,
                group: G::Study,
                scope: S::CurrentCourse,
            },
            // ---- 课程 ----
            P {
                label: "Switch Course",
                command: "/course",
                desc: "切换课程",
                action: A::Pick(PickKind::CourseSwitch),
                group: G::Course,
                scope: S::Global,
            },
            P {
                label: "New Course",
                command: "/course -new",
                desc: "新建课程",
                action: A::Prompt {
                    title: "新建课程",
                    prompt: "课程名",
                    kind: crate::wizard::WizardKind::CreateCourse,
                },
                group: G::Course,
                scope: S::Global,
            },
            P {
                label: "Rename Course",
                command: "/rename-course",
                desc: "重命名当前课程",
                action: A::Prompt {
                    title: "重命名课程",
                    prompt: "新课程名",
                    kind: crate::wizard::WizardKind::RenameCourse,
                },
                group: G::Course,
                scope: S::CurrentCourse,
            },
            P {
                label: "Delete Course",
                command: "/course -delete",
                desc: "删除课程（笔记回落 all 区）",
                action: A::Pick(PickKind::CourseDelete),
                group: G::Course,
                scope: S::CurrentCourse,
            },
            P {
                label: "Import Materials",
                command: "/import",
                desc: "批量导入 md/pdf/pptx 到当前课程",
                action: A::WizardImport,
                group: G::Course,
                scope: S::CurrentCourse,
            },
            P {
                label: "Recent Sessions",
                command: "/sessions",
                desc: "查看当前课程学习历史",
                action: A::Run,
                group: G::Course,
                scope: S::CurrentCourse,
            },
            // ---- 会话 ----
            P {
                label: "Rename Session",
                command: "/rename",
                desc: "重命名当前会话",
                action: A::Prompt {
                    title: "重命名会话",
                    prompt: "新标题",
                    kind: crate::wizard::WizardKind::RenameSession,
                },
                group: G::Study,
                scope: S::CurrentSession,
            },
            P {
                label: "Export Conversation",
                command: "/export",
                desc: "导出当前会话为 JSON",
                action: A::Run,
                group: G::Study,
                scope: S::CurrentSession,
            },
            P {
                label: "Load Conversation",
                command: "/load",
                desc: "加载导出的会话 JSON",
                action: A::Prompt {
                    title: "加载会话",
                    prompt: "JSON 文件路径",
                    kind: crate::wizard::WizardKind::LoadFile,
                },
                group: G::Study,
                scope: S::Global,
            },
            // ---- 设置 ----
            P {
                label: "Model",
                command: "/model",
                desc: "切换模型",
                action: A::Run,
                group: G::Settings,
                scope: S::Global,
            },
            P {
                label: "Budget",
                command: "/budget",
                desc: "查看/设置预算（累计/上限/剩余）",
                action: A::Run,
                group: G::Settings,
                scope: S::Global,
            },
            P {
                label: "Budget → Set",
                command: "/budget",
                desc: "设置花费上限（元）",
                action: A::Prompt {
                    title: "设置预算上限",
                    prompt: "金额（元）",
                    kind: crate::wizard::WizardKind::BudgetLimit,
                },
                group: G::Settings,
                scope: S::Global,
            },
            P {
                label: "Budget → Reset",
                command: "/budget reset",
                desc: "重置累计花费",
                action: A::Run,
                group: G::Settings,
                scope: S::Global,
            },
            // ---- 帮助 ----
            P {
                label: "Help",
                command: "/help",
                desc: "能力总览",
                action: A::Run,
                group: G::Help,
                scope: S::Global,
            },
        ]
    }

    pub fn new() -> Self {
        let items = Self::all_items();
        let filtered = (0..items.len()).collect();
        Self {
            items,
            filtered,
            selected: 0,
            scroll: 0,
        }
    }

    /// context-aware 过滤（文档 §5）：按当前 workspace 与是否有课程裁掉不适用项。
    /// - Home：只留 Continue / 课程创建切换 / 设置 / 帮助；无课程时再去掉课程项
    /// - Course：隐藏会话级动作（Rename Session / Export Conversation）
    /// - Session：隐藏课程管理（New/Delete Course），保留学习/设置/帮助
    pub(crate) fn apply_context(&mut self, workspace: crate::app::Workspace, has_courses: bool) {
        use crate::app::Workspace as W;
        let mut keep: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| match workspace {
                W::Home => {
                    // Home 不执行课程级动作（Review/Outline/Materials…），
                    // 保留 New Course / Switch Course / Import / 设置 / 帮助
                    match i.group {
                        PaletteGroup::Course => {
                            i.label == "New Course"
                                || i.label == "Switch Course"
                                || i.label == "Import Materials"
                        }
                        PaletteGroup::Settings | PaletteGroup::Help => true,
                        PaletteGroup::Study => i.label == "New Conversation",
                    }
                }
                W::Course => i.scope != PaletteScope::CurrentSession,
                W::Session => {
                    !(i.group == PaletteGroup::Course
                        && (i.label == "New Course" || i.label == "Delete Course"))
                }
            })
            .map(|(idx, _)| idx)
            .collect();
        if !has_courses {
            // 无课程：去掉一切 CurrentCourse 作用域项（Review/Outline/Continue/Materials…）
            keep.retain(|&idx| self.items[idx].scope != PaletteScope::CurrentCourse);
        }
        self.filtered = keep;
        self.selected = 0;
        self.scroll = 0;
    }

    /// 按聊天框当前内容过滤（共享缓冲）。匹配 label / desc / CLI command（power-user 直达）。
    pub(crate) fn refilter(&mut self, input: &str) {
        let f = input.to_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                f.is_empty()
                    || i.label.to_lowercase().contains(&f)
                    || i.command.to_lowercase().contains(&f)
                    || i.desc.to_lowercase().contains(&f)
            })
            .map(|(idx, _)| idx)
            .collect();
        self.selected = 0;
        self.scroll = 0;
    }
}

#[cfg(test)]
mod picker_filter_tests {
    use super::*;

    fn picker() -> ListPicker {
        ListPicker {
            title: "t".into(),
            items: vec![
                ListChoice {
                    label: "○ 变量遮蔽".into(),
                    command: "/review".into(),
                    action: None,
                },
                ListChoice {
                    label: "△ 借用（做对 1 / 共 2 题）".into(),
                    command: "/review".into(),
                    action: None,
                },
                ListChoice {
                    label: "✓ 所有权".into(),
                    command: "/review".into(),
                    action: None,
                },
            ],
            selected: 0,
        }
    }

    #[test]
    fn empty_input_shows_all() {
        let p = picker();
        assert_eq!(p.visible("").len(), 3);
    }

    #[test]
    fn filter_matches_case_insensitive_substring() {
        let p = picker();
        let v = p.visible("借用");
        assert_eq!(v, vec![1]);
        let p2 = ListPicker {
            title: "t".into(),
            items: vec![ListChoice {
                label: "○ Vec 容器".into(),
                command: "c".into(),
                action: None,
            }],
            selected: 0,
        };
        assert_eq!(p2.visible("vec").len(), 1);
        assert_eq!(p2.visible("VEC").len(), 1);
    }

    #[test]
    fn no_match_is_empty() {
        let p = picker();
        assert!(p.visible("不存在的概念").is_empty());
    }

    #[test]
    fn selected_is_index_into_visible_list() {
        let p = picker();
        let v = p.visible("借用");
        assert_eq!(v[p.selected.min(v.len() - 1)], 1);
    }
}
