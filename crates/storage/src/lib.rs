//! storage：rusqlite 同步 API（决策 D2，调用方经 spawn_blocking 进入线程池）
//! + FTS5(jieba 预分词) 中文检索（决策 D1）。

mod error;
mod search;
mod segment;
mod store;

pub use error::{Error, Result};
pub use search::SearchHit;
pub use segment::{match_query, match_query_or, segment};
pub use store::{
    ChunkHit, ConceptMastery, InsertOutcome, NewChunk, NewNote, Note, NoteSummary, QuestionRecord,
    SessionMeta, Store, content_hash,
};

/// 当前 schema 版本（user_version）。追加迁移时 +1 并在 migrate() 补分支。
pub const SCHEMA_VERSION: i32 = 2;

/// DDL 单点存放（与规划.md 第四章定稿基线一致）。
pub fn schema_sql() -> &'static str {
    include_str!("schema.sql")
}
