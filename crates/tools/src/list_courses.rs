//! list_courses：列出课程及笔记/概念统计（只读，决策 D6）。

use std::sync::Arc;

use agent_core::{Error, Tool};
use async_trait::async_trait;
use serde_json::{Value, json};
use storage::Store;

pub struct ListCoursesTool {
    store: Arc<Store>,
}

impl ListCoursesTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ListCoursesTool {
    fn name(&self) -> &str {
        "list_courses"
    }

    fn description(&self) -> &str {
        "列出知识库中的全部课程及其笔记数量。"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: Value) -> agent_core::Result<Value> {
        let store = Arc::clone(&self.store);
        let courses = tokio::task::spawn_blocking(move || {
            store
                .list_courses()
                .map_err(|e| Error::Storage(e.to_string()))
        })
        .await
        .map_err(|e| Error::Transport(e.to_string()))??;

        let list: Vec<Value> = courses
            .iter()
            .map(|(id, name)| json!({"id": id, "name": name}))
            .collect();

        Ok(json!({"courses": list}))
    }
}
