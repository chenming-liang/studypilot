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

    let concepts = store.list_concepts_with_mastery(Some(course_id))?;
    let signature =
        importer::review_map::signature_of(concepts.iter().map(|c| c.name.clone()).collect());
    let cached =
        importer::review_map::load_outline_cache(course_id).map_err(|e| anyhow::anyhow!(e))?;
    let map = match cached {
        Some(c) if c.signature == signature => {
            println!("（缓存命中，零 LLM）\n");
            c.map
        }
        _ => {
            println!(
                "（缓存未命中/概念变化，调 LLM：{} / {}，组织 {} 个概念）\n",
                pc.name,
                pc.model,
                concepts.len()
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
            importer::review_map::save_outline_cache(course_id, signature, &map)
                .map_err(|e| anyhow::anyhow!(e))?;
            map
        }
    };

    println!("{}", map.markdown());
    println!("══ 选择器条目（前 10）══");
    for (label, cmd) in map.picker_items(0).into_iter().take(10) {
        println!("  {label}\n      → {cmd}");
    }
    Ok(())
}
