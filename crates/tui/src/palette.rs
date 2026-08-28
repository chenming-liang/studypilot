//! 命令面板与模型弹窗的数据结构。

use agent_providers::ProviderConfig;
/// 模型选择弹窗状态。
pub struct ModelPicker {
    pub options: Vec<ProviderConfig>,
    pub selected: usize,
}

/// 命令面板条目：`needs_arg` 决定 Enter 行为——无参命令直接执行，
/// 带参命令填入输入框等待用户补全。
pub struct PaletteItem {
    pub command: &'static str,
    pub desc: &'static str,
    pub needs_arg: bool,
}

/// 命令面板（Ctrl+K）：可发现性入口——把"项目有什么能力"直接摆在眼前，
/// 交互与 /model 弹窗同范式（覆盖层 + 按键路由优先级）。
pub struct CommandPalette {
    pub items: Vec<PaletteItem>,
    /// 过滤后命中的 items 下标
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub filter: String,
}

impl CommandPalette {
    /// 全量命令表（与 /help 能力文案一一对应）。
    fn all_items() -> Vec<PaletteItem> {
        use PaletteItem as P;
        vec![
            P {
                command: "/help",
                desc: "我能做什么（能力总览）",
                needs_arg: false,
            },
            P {
                command: "/outline",
                desc: "生成当前课程知识大纲",
                needs_arg: false,
            },
            P {
                command: "/outline <课程> --export",
                desc: "生成大纲并导出 Markdown",
                needs_arg: true,
            },
            P {
                command: "/review <课程> [概念] [--n 数量]",
                desc: "出题复习（掌握度低优先）",
                needs_arg: true,
            },
            P {
                command: "/import <目录> [--course 名]",
                desc: "批量导入 md/pdf/pptx",
                needs_arg: true,
            },
            P {
                command: "/notes",
                desc: "列出当前分区的笔记",
                needs_arg: false,
            },
            P {
                command: "/course",
                desc: "查看课程分区",
                needs_arg: false,
            },
            P {
                command: "/model",
                desc: "切换模型（弹窗）",
                needs_arg: false,
            },
            P {
                command: "/budget",
                desc: "查看预算花费",
                needs_arg: false,
            },
            P {
                command: "/sessions",
                desc: "历史会话列表",
                needs_arg: false,
            },
            P {
                command: "/new",
                desc: "开启新会话",
                needs_arg: false,
            },
            P {
                command: "/export",
                desc: "导出当前会话为 JSON",
                needs_arg: false,
            },
            P {
                command: "/load <文件>",
                desc: "加载导出的会话 JSON",
                needs_arg: true,
            },
            P {
                command: "/open <id>",
                desc: "恢复指定历史会话",
                needs_arg: true,
            },
            P {
                command: "/rename <标题>",
                desc: "重命名当前会话",
                needs_arg: true,
            },
            P {
                command: "/delete <id>",
                desc: "删除笔记（不碰磁盘原文件）",
                needs_arg: true,
            },
            P {
                command: "/move <id> <课程>",
                desc: "迁移笔记到另一课程",
                needs_arg: true,
            },
            P {
                command: "/course -new <名>",
                desc: "新建课程分区",
                needs_arg: true,
            },
            P {
                command: "/course -delete <名>",
                desc: "删除课程（笔记回落 all 区）",
                needs_arg: true,
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
            filter: String::new(),
        }
    }

    pub(crate) fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
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
