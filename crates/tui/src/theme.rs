//! 语义色主题（收口会话⑨）：颜色是语义 token，不是装饰。
//!
//! 规则（70/20/10）：大多数正文用低亮度灰白；少数高饱和色只给
//! 真正需要用户注意的语义（状态/引用/错误）。换主题只改本文件。
//!
//! 颜色重构（问题.md 追加）：正文=终端默认前景、蓝色为主视觉、
//! 绿/黄/红仅语义、**不使用紫色**——原 SECONDARY（紫）统一为主蓝。

use ratatui::style::Color;

/// 终端默认前景色（正文；不再强制染色）
pub const DEFAULT: Color = Color::Reset;
/// 主正文（= 终端默认前景，不再强制染色；颜色重构）
pub const FG: Color = DEFAULT;
/// 次级文字：时间、metadata、快捷键提示
pub const MUTED: Color = Color::Rgb(0x72, 0x72, 0x7A);

/// Primary：应用名、当前课程、选中状态、结构（主视觉色）
pub const PRIMARY: Color = Color::Rgb(0x61, 0xAF, 0xEF);
/// Secondary：Markdown 标题、代码关键字、AI 前缀——统一为主蓝（不再紫）
pub const SECONDARY: Color = PRIMARY;
/// 二级标题/次级结构（淡蓝）
pub const SECONDARY_DIM: Color = Color::Rgb(0x8F, 0xB8, 0xD9);

/// 用户输入、Review 题目（暖黄）
pub const USER: Color = Color::Rgb(0xE5, 0xC0, 0x7B);
/// 警告/需巩固（黄）——Review Map △、需巩固概念
pub const WARNING: Color = Color::Rgb(0xE5, 0xC0, 0x7B);
/// 概念、成功、✓（绿）
pub const SUCCESS: Color = Color::Rgb(0x98, 0xC3, 0x79);
/// 引用 [n]、来源、RAG 资料（青）——产品品牌色
pub const REFERENCE: Color = Color::Rgb(0x56, 0xB6, 0xC2);
/// 错误（红）
pub const ERROR: Color = Color::Rgb(0xE0, 0x6C, 0x75);
/// 代码块前景（syntect 主题接管，此值仅兜底）
pub const CODE_FG: Color = Color::Rgb(0xE5, 0xC0, 0x7B);
/// 行内代码（One Dark 橙，无背景——黑底上 DarkGray 打底刺眼）
pub const CODE_INLINE: Color = Color::Rgb(0xD1, 0x9A, 0x66);

// ── Runtime Status 语义色（问题.md：右上角状态栏独立于蓝色导航/对话层）──
// 要求：processing 用暖/紫色系、tool 用青绿、success 绿、warning 黄、error 红、idle 灰。
// 统一走 `status_color()` 映射，不散落在各字符串上。

/// processing / thinking / generating：暖橙（不占蓝色主视觉）
pub const STATUS_PROCESSING: Color = Color::Rgb(0xE8, 0xA3, 0x5C);
/// tool execution：青绿
pub const STATUS_TOOL: Color = REFERENCE;
/// warning / 需注意：黄
pub const STATUS_WARNING: Color = WARNING;
/// error / 失败：红
pub const STATUS_ERROR: Color = ERROR;
/// cancelled / idle：灰
pub const STATUS_IDLE: Color = MUTED;
/// success / ready：绿（与主题 SUCCESS 同语义）
pub const STATUS_SUCCESS: Color = SUCCESS;

/// Runtime Status 语义种类（仅用于颜色映射，不影响业务逻辑）。
/// 完整覆盖文档列出的状态类；部分变体当前 header 未用到，保留映射层供扩展。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StatusKind {
    Processing,
    Tool,
    Warning,
    Error,
    Idle,
    Success,
}

/// 状态 → 语义色 的集中映射（问题.md：同一类状态必须颜色一致，禁止逐字符串上色）。
pub fn status_color(kind: StatusKind) -> Color {
    match kind {
        StatusKind::Processing => STATUS_PROCESSING,
        StatusKind::Tool => STATUS_TOOL,
        StatusKind::Warning => STATUS_WARNING,
        StatusKind::Error => STATUS_ERROR,
        StatusKind::Idle => STATUS_IDLE,
        StatusKind::Success => STATUS_SUCCESS,
    }
}

#[cfg(test)]
mod status_color_tests {
    use super::*;

    /// 问题.md：状态 → 语义色映射，同一类状态颜色一致，processing 不用蓝色主视觉。
    #[test]
    fn status_kind_maps_to_distinct_semantic_colors() {
        let p = status_color(StatusKind::Processing);
        assert_ne!(p, PRIMARY, "processing 不得用蓝色主视觉");
        assert_eq!(p, STATUS_PROCESSING);
        assert_eq!(status_color(StatusKind::Success), SUCCESS);
        assert_eq!(status_color(StatusKind::Warning), WARNING);
        assert_eq!(status_color(StatusKind::Error), ERROR);
        assert_eq!(status_color(StatusKind::Idle), MUTED);
        assert_eq!(status_color(StatusKind::Tool), REFERENCE);
        // 同类一致：同一 Kind 永远同色
        for _ in 0..2 {
            assert_eq!(status_color(StatusKind::Processing), p);
        }
    }
}
