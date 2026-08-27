//! search_notes：chunk 级 FTS5 检索，返回精准片段（决策 D1/D6）。

use std::sync::Arc;

use agent_core::{Error, Tool};
use async_trait::async_trait;
use serde_json::{Value, json};
use storage::Store;

pub struct SearchNotesTool {
    store: Arc<Store>,
    course_id: Option<i64>,
}

impl SearchNotesTool {
    pub fn new(store: Arc<Store>, course_id: Option<i64>) -> Self {
        Self { store, course_id }
    }
}

#[async_trait]
impl Tool for SearchNotesTool {
    fn name(&self) -> &str {
        "search_notes"
    }

    fn description(&self) -> &str {
        "检索个人知识库笔记，返回最相关的片段。回答时优先依据这些资料，\
         每条结果带 [n] 标号，请在用到其内容时用 [n] 标注引用；\
         资料未覆盖的部分可用自身知识补充，但需明示「（笔记外补充）」。"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "检索关键词或问题"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> agent_core::Result<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Parse("缺少 query 参数".into()))?;

        let store = Arc::clone(&self.store);
        let cid = self.course_id;
        let query = query.to_owned();

        let hits = tokio::task::spawn_blocking(move || store.search_chunks(&query, cid, 8))
            .await
            .map_err(|e| Error::Transport(e.to_string()))?
            .map_err(|e| Error::Storage(e.to_string()))?;

        if hits.is_empty() {
            return Ok(json!({
                "results": [],
                "hint": "未检索到相关笔记，请尝试换关键词。"
            }));
        }

        // context 去重：同一笔记只保留一个 chunk（bm25 最好的），避免重复内容污染 LLM
        let mut seen_notes = std::collections::HashSet::new();
        let deduped: Vec<_> = hits
            .into_iter()
            .filter(|h| seen_notes.insert(h.note_id))
            .collect();

        let results: Vec<Value> = deduped
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let preview: String = h.content.chars().take(1500).collect();
                let role = classify_chunk(&h.heading);
                json!({
                    "id": i + 1,
                    "title": h.note_title,
                    "section": h.heading,
                    "role": role,
                    "content": preview,
                })
            })
            .collect();

        Ok(json!({
            "results": results,
            "count": results.len(),
            "hint": "请用 [n] 标号引用上述片段。如：根据[1]所述...\n\
                     每个 chunk 有 role 标签（定义/例子/规则/易混淆/总结），请据此组织回答。"
        }))
    }
}

/// 根据 chunk 标题分类角色（供 LLM 组织回答结构）。
fn classify_chunk(heading: &str) -> &'static str {
    let h = heading.to_lowercase();
    if h.contains("例") || h.contains("示例") || h.contains("demo") || h.contains("代码") {
        "例子"
    } else if h.contains("规则") || h.contains("约束") || h.contains("限制") || h.contains("要求")
    {
        "规则"
    } else if h.contains("误区")
        || h.contains("陷阱")
        || h.contains("注意")
        || h.contains("坑")
        || h.contains("错误")
    {
        "易混淆"
    } else if h.contains("总结") || h.contains("小结") || h.contains("对比") || h.contains("区别")
    {
        "总结"
    } else if h.contains("定义") || h.contains("什么是") || h.contains("概念") || h.contains("原理")
    {
        "定义"
    } else {
        "正文"
    }
}
