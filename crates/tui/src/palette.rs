//! 命令面板、列表选择器与模型弹窗的数据结构。

use crate::outline_render::ReviewMap;
use agent_providers::ProviderConfig;

/// 复习地图（Learning Map）树形选择器：section 头行 + 缩进概念行（键盘 ↑↓/Enter/Esc）。
/// 选中概念 → 复习该概念；选中 section → 复习整节（scope = 节内全部概念）。
pub struct ReviewMapPicker {
    pub title: String,
    pub rows: Vec<MapRow>,
    pub selected: usize,
}

/// 地图中的一行（扁平化；渲染时 section 作分组头，概念行缩进）。
#[derive(Debug, Clone, PartialEq)]
pub enum MapRow {
    Section {
        title: String,
        /// 聚合状态 (已掌握, 需巩固, 未复习)
        counts: (usize, usize, usize),
        /// 节内概念名（Enter = 复习整节 scope）
        concept_names: Vec<String>,
    },
    Concept {
        name: String,
        /// 状态标记 ✓/△/○
        mark: char,
        attempts: usize,
        correct: usize,
    },
}

impl ReviewMapPicker {
    pub fn from_map(map: &ReviewMap) -> Self {
        let mut rows: Vec<MapRow> = Vec::new();
        for (s, counts) in map.sections.iter().zip(map.section_counts()) {
            rows.push(MapRow::Section {
                title: s.title.clone(),
                counts,
                concept_names: s.nodes.iter().map(|n| n.name.clone()).collect(),
            });
            for n in &s.nodes {
                rows.push(MapRow::Concept {
                    name: n.name.clone(),
                    mark: n.status().mark().chars().next().unwrap_or('○'),
                    attempts: n.attempts,
                    correct: n.correct,
                });
            }
        }
        Self {
            title: format!("{} · Learning Map", map.course),
            rows,
            selected: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// 模型选择弹窗中的一行：一个 (provider, model) 可选项。
pub struct ModelOption {
    /// provider 名（切换目标）
    pub provider: String,
    /// provider 展示名（分组头）
    pub provider_display: String,
    /// 模型 id
    pub model: String,
    /// 模型展示名
    pub model_display: String,
    /// 是否 known pricing（Cost tracking available）
    pub known_pricing: bool,
    /// 思考模式
    pub thinking: bool,
    /// 角色配置行（fast/balanced/reasoning 绑定模型），非真实模型
    pub is_role: bool,
}

/// 模型选择弹窗状态（全量分组：provider 分组头 + 模型行）。
pub struct ModelPicker {
    pub options: Vec<ModelOption>,
    pub selected: usize,
    /// Some(role_key) = 正在为该角色选择模型；None = 切换当前模型
    pub role: Option<String>,
    /// config.toml `[roles]` 当前绑定（角色行渲染 + 角色模式初始定位）
    pub roles: std::collections::BTreeMap<String, String>,
}

impl ModelPicker {
    /// 从 providers 展开成「角色配置行 + provider 分组 + 模型行」扁平行。
    /// 展示当前选中模型（provider/model 匹配的行）。
    pub fn from_providers(
        providers: &[ProviderConfig],
        current_provider: &str,
        current_model: &str,
        roles: &std::collections::BTreeMap<String, String>,
    ) -> Self {
        let mut options = Vec::new();
        let mut selected = 0usize;
        // 顶部角色配置行（fast/balanced/reasoning）：Enter 进入该角色的模型选择。
        // 初始定位：优先当前选中模型；无模型时落在 fast 角色行。
        for role in ["fast", "balanced", "reasoning"] {
            options.push(ModelOption {
                provider: String::new(),
                provider_display: String::new(),
                model: String::new(),
                model_display: role.to_string(),
                known_pricing: false,
                thinking: false,
                is_role: true,
            });
        }
        for pc in providers {
            for m in pc.models_or_legacy() {
                let is_current = pc.name == current_provider && m.id == current_model;
                if is_current {
                    selected = options.len();
                }
                options.push(ModelOption {
                    provider: pc.name.clone(),
                    provider_display: pc.name.clone(),
                    model: m.id.clone(),
                    model_display: m.display_name().to_string(),
                    known_pricing: pc.known_pricing(),
                    thinking: m.thinking,
                    is_role: false,
                });
            }
        }
        let n = options.len();
        Self {
            options,
            selected: selected.min(n.saturating_sub(1)),
            role: None,
            roles: roles.clone(),
        }
    }

    /// 进入角色选择模式：选项只剩真实模型，初始定位到该角色当前绑定（若已配置）。
    pub fn enter_role(&mut self, role: &str) {
        let bound = self.roles.get(role).cloned().unwrap_or_default();
        let mut selected = 0usize;
        let mut models = Vec::new();
        for o in &self.options {
            if o.is_role {
                continue;
            }
            if !bound.is_empty() && format!("{}/{}", o.provider, o.model) == bound {
                selected = models.len();
            }
            models.push(ModelOption {
                provider: o.provider.clone(),
                provider_display: o.provider_display.clone(),
                model: o.model.clone(),
                model_display: o.model_display.clone(),
                known_pricing: o.known_pricing,
                thinking: o.thinking,
                is_role: false,
            });
        }
        self.role = Some(role.to_string());
        self.options = models;
        self.selected = selected.min(self.options.len().saturating_sub(1));
    }

    /// 当前选中行是否角色配置行。
    pub fn selected_is_role(&self) -> bool {
        self.options
            .get(self.selected)
            .map(|o| o.is_role)
            .unwrap_or(false)
    }
}

/// 面板分组（渲染为组头分隔行；过滤时隐藏）。顺序即显示顺序。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteGroup {
    Study,
    Course,
    Session,
    Settings,
}

impl PaletteGroup {
    pub fn label(&self) -> &'static str {
        match self {
            PaletteGroup::Study => "学习",
            PaletteGroup::Course => "课程",
            PaletteGroup::Session => "会话",
            PaletteGroup::Settings => "设置",
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
    /// 直接打开模型选择弹窗（不填输入框）
    OpenModel,
    /// 直接打开会话浏览器（不填输入框）
    OpenSessions,
    /// 直接查看预算（不填输入框）
    BudgetInfo,
    /// 直接重置累计花费（不填输入框）
    BudgetReset,
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
    /// 切换到课程（id）
    SwitchCourse(i64),
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
                action: A::OpenSessions,
                group: G::Session,
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
                group: G::Session,
                scope: S::CurrentSession,
            },
            P {
                label: "Export Conversation",
                command: "/export",
                desc: "导出当前会话为 JSON",
                action: A::Run,
                group: G::Session,
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
                group: G::Session,
                scope: S::Global,
            },
            // ---- 设置 ----
            P {
                label: "Model",
                command: "/model",
                desc: "切换模型 / 配置角色（fast/balanced/reasoning）",
                action: A::OpenModel,
                group: G::Settings,
                scope: S::Global,
            },
            P {
                label: "Budget",
                command: "/budget",
                desc: "查看/设置预算（累计/上限/剩余）",
                action: A::BudgetInfo,
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
                action: A::BudgetReset,
                group: G::Settings,
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
                    // 保留 New Course / Switch Course / Import / 设置
                    match i.group {
                        PaletteGroup::Course => {
                            i.label == "New Course"
                                || i.label == "Switch Course"
                                || i.label == "Import Materials"
                        }
                        PaletteGroup::Settings | PaletteGroup::Session => true,
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

    /// 问题 2/4/5：Model / Budget / Budget Reset / Recent Sessions 必须直开（不填输入框）。
    #[test]
    fn settings_and_sessions_actions_are_direct() {
        let p = CommandPalette::new();
        let get = |label: &str| p.items.iter().find(|i| i.label == label).unwrap();
        assert!(
            matches!(get("Model").action, PaletteAction::OpenModel),
            "Model 应直开弹窗"
        );
        assert!(
            matches!(get("Budget").action, PaletteAction::BudgetInfo),
            "Budget 应直开信息"
        );
        assert!(
            matches!(get("Budget → Reset").action, PaletteAction::BudgetReset),
            "Budget reset 应直开"
        );
        assert!(
            matches!(get("Recent Sessions").action, PaletteAction::OpenSessions),
            "Recent Sessions 应直开会话浏览器"
        );
        // 这些都不应走 Run（Run = 填输入框再 submit）
        for label in ["Model", "Budget", "Budget → Reset", "Recent Sessions"] {
            let i = get(label);
            assert!(
                !matches!(i.action, PaletteAction::Run),
                "{label} 不应走 Run（会污染输入框）"
            );
        }
    }

    /// 问题 6：Load Conversation 与 Recent Sessions 归入"会话"分组。
    #[test]
    fn conversation_actions_in_session_group() {
        let p = CommandPalette::new();
        let load = p
            .items
            .iter()
            .find(|i| i.label == "Load Conversation")
            .unwrap();
        let recent = p
            .items
            .iter()
            .find(|i| i.label == "Recent Sessions")
            .unwrap();
        assert_eq!(
            load.group,
            PaletteGroup::Session,
            "Load Conversation 应在会话组"
        );
        assert_eq!(
            recent.group,
            PaletteGroup::Session,
            "Recent Sessions 应在会话组"
        );
        // 问题 3：Help 能力总览已移除
        assert!(
            p.items.iter().all(|i| i.label != "Help"),
            "能力总览条目应移除"
        );
    }

    /// Learning Map 树形选择器：section 头行 + 缩进概念行，聚合状态与状态标记正确。
    #[test]
    fn review_map_picker_flattens_sections_and_concepts() {
        use importer::review_map::{ConceptNode, OutlineSection};
        let map = ReviewMap {
            course: "rust".into(),
            sections: vec![
                OutlineSection {
                    title: "所有权与借用".into(),
                    nodes: vec![
                        ConceptNode {
                            concept_id: Some(1),
                            name: "所有权".into(),
                            attempts: 2,
                            correct: 2,
                        },
                        ConceptNode {
                            concept_id: Some(2),
                            name: "借用".into(),
                            attempts: 2,
                            correct: 1,
                        },
                        ConceptNode {
                            concept_id: Some(3),
                            name: "移动语义".into(),
                            attempts: 0,
                            correct: 0,
                        },
                    ],
                    refs: vec![],
                },
                OutlineSection {
                    title: "类型系统".into(),
                    nodes: vec![ConceptNode {
                        concept_id: Some(4),
                        name: "Trait".into(),
                        attempts: 1,
                        correct: 1,
                    }],
                    refs: vec![],
                },
            ],
            unresolved: 0,
            titles: vec![],
            today_attempts: 0,
            today_reviewed: vec![],
        };
        let p = ReviewMapPicker::from_map(&map);
        assert_eq!(p.title, "rust · Learning Map");
        // 行 = section1 头 + 3 概念 + section2 头 + 1 概念
        assert_eq!(p.len(), 6);
        match &p.rows[0] {
            MapRow::Section {
                title,
                counts,
                concept_names,
            } => {
                assert_eq!(title, "所有权与借用");
                assert_eq!(*counts, (1, 1, 1), "聚合 ✓1·△1·○1");
                assert_eq!(concept_names.len(), 3);
            }
            _ => panic!("首行应为 section 头"),
        }
        match &p.rows[1] {
            MapRow::Concept { name, mark, .. } => {
                assert_eq!(name, "所有权");
                assert_eq!(*mark, '✓');
            }
            _ => panic!("概念行缩进"),
        }
        match &p.rows[2] {
            MapRow::Concept { name, mark, .. } => {
                assert_eq!(name, "借用");
                assert_eq!(*mark, '△');
            }
            _ => panic!("概念行缩进"),
        }
        match &p.rows[4] {
            MapRow::Section { title, counts, .. } => {
                assert_eq!(title, "类型系统");
                assert_eq!(*counts, (1, 0, 0));
            }
            _ => panic!("第二个 section 头"),
        }
    }

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
