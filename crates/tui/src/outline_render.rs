//! 复习地图：核心（数据/状态/渲染/构建）在 `importer::review_map`，
//! 此处仅 re-export TUI 侧用到的类型，保持 tui 内路径稳定。

pub use importer::review_map::{OutlinePayload, ReviewMap};
