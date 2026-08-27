//! M2 验收：`cargo run -p agent-core --example loop-demo`
//! MockProvider 驱动 agent loop，日志可见完整 tool call 往返（RUST_LOG=info）。

use agent_core::{Agent, Error, MockProvider, Response, Result, Tool, ToolBox, ToolCall};
use async_trait::async_trait;
use serde_json::{Value, json};

/// 演示用模拟检索工具（真实 search_notes 属 crates/tools，M7 接入）。
struct FakeSearchNotes;

#[async_trait]
impl Tool for FakeSearchNotes {
    fn name(&self) -> &str {
        "search_notes"
    }

    fn description(&self) -> &str {
        "检索个人知识库笔记，返回 top-k 片段（演示桩）"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "检索关键词"}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Config("缺少 query 参数".into()))?;
        Ok(json!({
            "hits": [
                {"note": "02-所有权与结构化数据", "snippet": format!("……与『{query}』相关的演示片段……")}
            ]
        }))
    }
}

fn resp_tool_call() -> Response {
    Response::with_tool_calls(vec![ToolCall::function(
        "call_demo_1",
        "search_notes",
        r#"{"query": "什么是所有权"}"#,
    )])
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let provider = MockProvider::new([
        // 第 1 轮：模型决定调用工具
        resp_tool_call(),
        // 第 2 轮：拿到工具结果后给出最终答案
        Response::simple("所有权是 Rust 的核心概念：值有唯一所有者，赋值即转移（move）。[1]"),
    ]);

    let agent = Agent::new(std::sync::Arc::new(provider))
        .with_tools(ToolBox::new().register(Box::new(FakeSearchNotes)))
        .with_system_prompt("你是个人知识库助手，仅依据资料回答并标注引用");

    let answer = agent.run("什么是所有权？").await?;

    println!("\n=== 最终答案 ===\n{answer}");
    Ok(())
}
