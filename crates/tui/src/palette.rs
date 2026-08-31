//! 命令面板、列表选择器与模型弹窗的数据结构。

use agent_providers::ProviderConfig;

/// 模型选择弹窗状态。
pub struct ModelPicker {
    pub options: Vec<ProviderConfig>,
    pub selected: usize,
}

/// 面板分组（渲染为组头分隔行；过滤时隐藏）。顺序即显示顺序。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteGroup {
    Learn,
    Knowledge,
    Course,
    Session,
    System,
}

impl PaletteGroup {
    pub fn label(&self) -> &'static str {
        match self {
            PaletteGroup::Learn => "学习",
            PaletteGroup::Knowledge => "知识库",
            PaletteGroup::Course => "课程",
            PaletteGroup::Session => "会话",
            PaletteGroup::System => "系统",
        }
    }
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
    pub command: &'static str,
    pub desc: &'static str,
    pub action: PaletteAction,
    pub group: PaletteGroup,
}

/// 通用列表选择器：单列 label，Enter 提交绑定的完整命令。
/// 支持输入过滤（问题10：65+ 概念选择器可搜）。
pub struct ListPicker {
    pub title: String,
    pub items: Vec<ListChoice>,
    pub selected: usize,
    /// 过滤串（label 包含匹配，大小写不敏感）；空 = 显示全部
    pub filter: String,
}

impl ListPicker {
    /// 过滤后的可见条目（filter 为空 = 全部）。
    pub fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.items.len()).collect();
        }
        let f = self.filter.to_lowercase();
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
    /// 选中后提交的完整命令
    pub command: String,
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
    /// 全量命令表（书写顺序 = 分组顺序 = 显示顺序；与 /help 能力文案对应）。
    fn all_items() -> Vec<PaletteItem> {
        use PaletteAction as A;
        use PaletteGroup as G;
        use PaletteItem as P;
        vec![
            // ---- 学习 ----
            P {
                command: "/outline",
                desc: "生成当前课程知识大纲",
                action: A::Run,
                group: G::Learn,
            },
            P {
                command: "/outline --export",
                desc: "生成大纲并导出 Markdown",
                action: A::Run,
                group: G::Learn,
            },
            P {
                command: "/review --course <名>",
                desc: "出题复习（掌握度低优先）",
                action: A::WizardReview,
                group: G::Learn,
            },
            P {
                command: "/review-map",
                desc: "复习地图：选知识点开复习（△ 巩固优先）",
                action: A::Run,
                group: G::Learn,
            },
            // ---- 知识库 ----
            P {
                command: "/notes",
                desc: "浏览与管理笔记（搜索·多选·移动·删除）",
                action: A::Run,
                group: G::Knowledge,
            },
            P {
                command: "/import --dir <目录>",
                desc: "批量导入 md/pdf/pptx",
                action: A::WizardImport,
                group: G::Knowledge,
            },
            // ---- 课程 ----
            P {
                command: "/course",
                desc: "切换课程分区（选择列表）",
                action: A::Pick(PickKind::CourseSwitch),
                group: G::Course,
            },
            P {
                command: "/course -new <名>",
                desc: "新建课程分区",
                action: A::Prompt {
                    title: "新建课程",
                    prompt: "课程名",
                    kind: crate::wizard::WizardKind::CreateCourse,
                },
                group: G::Course,
            },
            P {
                command: "/course -delete <名>",
                desc: "删除课程（笔记回落 all 区）",
                action: A::Pick(PickKind::CourseDelete),
                group: G::Course,
            },
            // ---- 会话 ----
            P {
                command: "/new",
                desc: "开启新会话",
                action: A::Run,
                group: G::Session,
            },
            P {
                command: "/sessions",
                desc: "历史会话列表",
                action: A::Run,
                group: G::Session,
            },
            P {
                command: "/rename <标题>",
                desc: "重命名当前会话",
                action: A::Prompt {
                    title: "重命名会话",
                    prompt: "新标题",
                    kind: crate::wizard::WizardKind::RenameSession,
                },
                group: G::Session,
            },
            P {
                command: "/export",
                desc: "导出当前会话为 JSON",
                action: A::Run,
                group: G::Session,
            },
            P {
                command: "/load <文件>",
                desc: "加载导出的会话 JSON",
                action: A::Prompt {
                    title: "加载会话",
                    prompt: "JSON 文件路径",
                    kind: crate::wizard::WizardKind::LoadFile,
                },
                group: G::Session,
            },
            // ---- 系统 ----
            P {
                command: "/model",
                desc: "切换模型（弹窗）",
                action: A::Run,
                group: G::System,
            },
            P {
                command: "/budget",
                desc: "查看预算（累计/上限/剩余）",
                action: A::Run,
                group: G::System,
            },
            P {
                command: "/budget <金额>",
                desc: "设置花费上限（元）",
                action: A::Prompt {
                    title: "设置预算上限",
                    prompt: "金额（元）",
                    kind: crate::wizard::WizardKind::BudgetLimit,
                },
                group: G::System,
            },
            P {
                command: "/budget reset",
                desc: "重置累计花费（清空 usage_log）",
                action: A::Run,
                group: G::System,
            },
            P {
                command: "/help",
                desc: "我能做什么（能力总览）",
                action: A::Run,
                group: G::System,
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

    /// 按聊天框当前内容过滤（共享缓冲）。
    pub(crate) fn refilter(&mut self, input: &str) {
        let f = input.to_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                f.is_empty()
                    || i.command.to_lowercase().contains(&f)
                    || i.desc.to_lowercase().contains(&f)
            })
            .map(|(idx, _)| idx)
            .collect();
        self.selected = 0;
        self.scroll = 0;
    }
}
