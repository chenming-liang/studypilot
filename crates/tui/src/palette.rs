//! 命令面板、列表选择器与模型弹窗的数据结构。

use agent_providers::ProviderConfig;

/// 模型选择弹窗状态。
pub struct ModelPicker {
    pub options: Vec<ProviderConfig>,
    pub selected: usize,
}

/// 面板条目的 Enter 行为（Tab 一律填入文本模式，power-user 兜底）。
#[derive(Clone, Copy)]
pub enum PaletteAction {
    /// 无参命令，直接执行
    Run,
    /// /review 参数向导
    WizardReview,
    /// /import 参数向导
    WizardImport,
    /// 单步文本向导：(弹窗标题, 输入提示, 命令前缀)
    /// 前缀必须不含占位符——向导完成时用 `prefix + " " + 输入` 合成真实命令
    Prompt(&'static str, &'static str, &'static str),
    /// 列表选择器
    Pick(PickKind),
    /// 填入输入框（复合参数，如 /delete /move）
    Fill,
}

#[derive(Clone, Copy)]
pub enum PickKind {
    /// 切换课程分区（all + 全部课程）
    CourseSwitch,
    /// 删除课程（仅现有课程）
    CourseDelete,
    /// 恢复历史会话
    Session,
}

/// 命令面板条目：行为直接绑在条目上（不再是命令字符串前缀分流）。
pub struct PaletteItem {
    pub command: &'static str,
    pub desc: &'static str,
    pub action: PaletteAction,
}

/// 通用列表选择器：单列 label，Enter 提交绑定的完整命令。
pub struct ListPicker {
    pub title: String,
    pub items: Vec<ListChoice>,
    pub selected: usize,
}

pub struct ListChoice {
    /// 展示文本
    pub label: String,
    /// 选中后提交的完整命令
    pub command: String,
}

/// 命令面板（Ctrl+K）：可发现性入口——把"项目有什么能力"直接摆在眼前，
/// 交互与 /model 弹窗同范式（覆盖层 + 按键路由优先级）。
pub struct CommandPalette {
    pub items: Vec<PaletteItem>,
    /// 过滤后命中的 items 下标
    pub filtered: Vec<usize>,
    pub selected: usize,
    // 过滤串 = App.input（覆盖层与聊天框共享同一缓冲，无独立状态）
}

impl CommandPalette {
    /// 全量命令表（与 /help 能力文案一一对应）。
    fn all_items() -> Vec<PaletteItem> {
        use PaletteAction as A;
        use PaletteItem as P;
        vec![
            P {
                command: "/help",
                desc: "我能做什么（能力总览）",
                action: A::Run,
            },
            P {
                command: "/outline",
                desc: "生成当前课程知识大纲",
                action: A::Run,
            },
            P {
                command: "/outline --export",
                desc: "生成大纲并导出 Markdown",
                action: A::Run,
            },
            P {
                command: "/review <课程> [概念] [--n 数量]",
                desc: "出题复习（掌握度低优先）",
                action: A::WizardReview,
            },
            P {
                command: "/import <目录> [--course 名]",
                desc: "批量导入 md/pdf/pptx",
                action: A::WizardImport,
            },
            P {
                command: "/notes",
                desc: "列出当前分区的笔记",
                action: A::Run,
            },
            P {
                command: "/course",
                desc: "切换课程分区（选择列表）",
                action: A::Pick(PickKind::CourseSwitch),
            },
            P {
                command: "/course -new <名>",
                desc: "新建课程分区",
                action: A::Prompt("新建课程", "课程名", "/course -new"),
            },
            P {
                command: "/course -delete <名>",
                desc: "删除课程（笔记回落 all 区）",
                action: A::Pick(PickKind::CourseDelete),
            },
            P {
                command: "/model",
                desc: "切换模型（弹窗）",
                action: A::Run,
            },
            P {
                command: "/budget",
                desc: "查看预算花费",
                action: A::Run,
            },
            P {
                command: "/sessions",
                desc: "历史会话列表",
                action: A::Run,
            },
            P {
                command: "/new",
                desc: "开启新会话",
                action: A::Run,
            },
            P {
                command: "/export",
                desc: "导出当前会话为 JSON",
                action: A::Run,
            },
            P {
                command: "/open <id>",
                desc: "恢复指定历史会话（选择列表）",
                action: A::Pick(PickKind::Session),
            },
            P {
                command: "/rename <标题>",
                desc: "重命名当前会话",
                action: A::Prompt("重命名会话", "新标题", "/rename"),
            },
            P {
                command: "/load <文件>",
                desc: "加载导出的会话 JSON",
                action: A::Prompt("加载会话", "JSON 文件路径", "/load"),
            },
            P {
                command: "/delete <id>",
                desc: "删除笔记（不碰磁盘原文件）",
                action: A::Fill,
            },
            P {
                command: "/move <id> <课程>",
                desc: "迁移笔记到另一课程",
                action: A::Fill,
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
    }
}
