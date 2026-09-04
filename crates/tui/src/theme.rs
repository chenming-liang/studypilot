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
