//! 概念质量验收工具（2026-08-30 Concept Extraction 修复的 before/after 验证）。
//!
//! 用法：cargo run -p tui --example refresh_concepts -- <课程名>
//!
//! 流程：打印 before 概念清单 → 逐篇从 DB 全文重抽（真调 LLM）→ 替换关联 +
//! 清理零关联零历史概念 → 打印 after 清单。验收标准：无形容词/空泛词/同义重复，
//! 每个概念能独立出一道合理的复习题。

use std::sync::Arc;

use agent_providers::{Config, OpenAiClient};
use storage::Store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let course_name = std::env::args().nth(1).unwrap_or_else(|| "rust".to_owned());

    let store = Arc::new(Store::open("data/mynotes.db")?);
    let cfg = Config::load(std::path::Path::new("config.toml"))?;
    let pc = cfg.default_provider()?.clone();
    let provider = Arc::new(OpenAiClient::new(pc.clone())?);

    let Some(course_id) = store
        .list_courses()?
        .into_iter()
        .find(|(_, n)| n == &course_name)
        .map(|(id, _)| id)
    else {
        anyhow::bail!("课程 `{course_name}` 不存在");
    };

    let before = store.list_concept_names_by_course(course_id)?;
    println!("══ Before（{} 个）══", before.len());
    for c in &before {
        println!("  {c}");
    }

    println!("\n开始刷新（{} / {}，逐篇全文重抽）…\n", pc.name, pc.model);
    let max_cost = 50.0;
    let cancel = tokio_util::sync::CancellationToken::new();
    let msg = importer::refresh_course_concepts(
        Arc::clone(&store),
        provider,
        pc,
        course_id,
        max_cost,
        &cancel,
    )
    .await
    .map_err(|e| anyhow::anyhow!(e))?;
    println!("{msg}\n");

    let after = store.list_concept_names_by_course(course_id)?;
    println!("══ After（{} 个）══", after.len());
    for c in &after {
        println!("  {c}");
    }

    // 掌握度关联完整性：刷新不应破坏已有学习记录
    let linked = store.list_concepts_with_mastery(Some(course_id))?;
    println!("\n关联笔记的概念 {} 个（掌握度记录保留）", linked.len());
    Ok(())
}
