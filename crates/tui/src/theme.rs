//! 语义色主题（收口会话⑨）：颜色是语义 token，不是装饰。
//!
//! 规则（70/20/10）：大多数正文用低亮度灰白；少数高饱和色只给
//! 真正需要用户注意的语义（状态/引用/错误）。换主题只改本文件。

use ratatui::style::Color;

/// 主正文（浅灰白，不用纯白降亮度）
pub const FG: Color = Color::Rgb(0xD7, 0xD7, 0xDD);
/// 次级文字：时间、metadata、快捷键提示
pub const MUTED: Color = Color::Rgb(0x72, 0x72, 0x7A);

/// Primary：应用名、当前课程、选中状态、结构
pub const PRIMARY: Color = Color::Rgb(0x61, 0xAF, 0xEF);
/// Secondary：Markdown 标题、代码关键字、AI 前缀
pub const SECONDARY: Color = Color::Rgb(0xC6, 0x78, 0xDD);
/// 二级标题（淡紫）
pub const SECONDARY_DIM: Color = Color::Rgb(0xA8, 0x93, 0xC9);

/// 用户输入、Review 题目（暖黄）
pub const USER: Color = Color::Rgb(0xE5, 0xC0, 0x7B);
/// 概念、成功、✓（绿）
pub const SUCCESS: Color = Color::Rgb(0x98, 0xC3, 0x79);
/// 引用 [n]、来源、RAG 资料（青）——产品品牌色
pub const REFERENCE: Color = Color::Rgb(0x56, 0xB6, 0xC2);
/// 错误（红）
pub const ERROR: Color = Color::Rgb(0xE0, 0x6C, 0x75);
/// 代码块前景（暖白）
pub const CODE_FG: Color = Color::Rgb(0xE5, 0xC0, 0x7B);
