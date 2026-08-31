//! 复习地图（Review Map）生成验证工具：真调 LLM，打印带状态的复习地图 markdown
//! 与选择器条目。用法：cargo run -p tui --example outline_map -- <课程名>

use std::sync::Arc;

use agent_providers::{Config, OpenAiClient};
use storage::Store;
use tokio_util::sync::CancellationToken;

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

    println!(
        "生成复习地图（{} / {}，组织 {} 个概念）…\n",
        pc.name,
        pc.model,
        store.list_concepts_with_mastery(Some(course_id))?.len()
    );
    let map = importer::review_map::build_review_map(
        Arc::clone(&store),
        provider,
        pc,
        course_id,
        course_name,
        &CancellationToken::new(),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e))?;

    println!("{}", map.markdown());
    println!("══ 选择器条目（前 10）══");
    for (label, cmd) in map.picker_items(0).into_iter().take(10) {
        println!("  {label}\n      → {cmd}");
    }
    Ok(())
}
