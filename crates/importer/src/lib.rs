//! 导入流水线：walkdir 收集 → 格式解析 → LLM 概念抽取 → 入库（幂等去重 + 容错）。
//! 决策 D6：/import 直连内部函数，不进 agent loop。
//! 决策 D3：pdf 经 pymupdf 子进程。决策 D1：FTS 入库走 jieba 预分词。
//! 决策 D4：LLM JSON 输出降级链。

mod clean;
mod events;
mod extract;
mod parser;
mod pipeline;
pub mod review_map;

pub use events::{FilePhase, ImportEvent};
pub use extract::{extract_concepts, refresh_course_concepts};
pub use pipeline::{ImportConfig, import_directory};
pub use review_map::{ReviewMap, build_review_map};
