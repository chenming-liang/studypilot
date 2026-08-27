//! M2 agent loop 行为测试：全部用 MockProvider 离线驱动。

use std::sync::Arc;

use agent_core::{Agent, Error, MockProvider, Response, Role, Tool, ToolBox, ToolCall};
use async_trait::async_trait;
use serde_json::{Value, json};

/// 加法工具。
struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> &str {
        "两数相加"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"}
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, args: Value) -> agent_core::Result<Value> {
        let a = args
            .get("a")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Config("缺 a".into()))?;
        let b = args
            .get("b")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Config("缺 b".into()))?;
        Ok(json!({ "sum": a + b }))
    }
}

fn tools_with_add() -> ToolBox {
    ToolBox::new().register(Box::new(AddTool))
}

#[tokio::test]
async fn executes_tool_then_final_answer() {
    let provider = Arc::new(MockProvider::new([
        Response::with_tool_calls(vec![ToolCall::function(
            "call_1",
            "add",
            r#"{"a": 40, "b": 2}"#,
        )]),
        Response::simple("40 + 2 = 42"),
    ]));

    let agent = Agent::new(provider.clone()).with_tools(tools_with_add());
    let answer = agent.run("1+1=? 不对，请算 40+2").await.unwrap();
    assert_eq!(answer, "40 + 2 = 42");
    assert_eq!(provider.remaining(), 0);

    // 第二次请求应包含 assistant(tool_calls) + tool 结果回填
    let second = provider.received(1).unwrap();
    assert_eq!(second.len(), 3); // user + assistant(tool_call) + tool
    assert_eq!(second[0].role, Role::User);
    assert_eq!(second[1].role, Role::Assistant);
    assert_eq!(second[1].tool_calls[0].id, "call_1");
    assert_eq!(second[2].role, Role::Tool);
    assert_eq!(second[2].tool_call_id.as_deref(), Some("call_1"));
    let tool_content = second[2].content.as_deref().unwrap_or_default();
    assert!(tool_content.contains("\"sum\":42"), "{tool_content}");
}

#[tokio::test]
async fn no_tool_call_answers_immediately() {
    let provider = Arc::new(MockProvider::one(Response::simple("直接答案")));
    let agent = Agent::new(provider);
    let answer = agent.run("你好").await.unwrap();
    assert_eq!(answer, "直接答案");
}

#[tokio::test]
async fn system_prompt_is_prepended() {
    let provider = Arc::new(MockProvider::one(Response::simple("ok")));
    Agent::new(provider.clone())
        .with_system_prompt("你是知识库助手")
        .run("hi")
        .await
        .unwrap();
    let first = provider.received(0).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].role, Role::System);
    assert_eq!(first[0].content.as_deref(), Some("你是知识库助手"));
}

#[tokio::test]
async fn unknown_tool_feeds_error_back_and_recovers() {
    let tools = ToolBox::new(); // 没有 add 工具
    let provider = Arc::new(MockProvider::new([
        Response::with_tool_calls(vec![ToolCall::function("c1", "nonexistent", "{}")]),
        Response::simple("已处理"),
    ]));
    let agent = Agent::new(provider.clone()).with_tools(tools);
    let answer = agent.run("x").await.unwrap();

    assert_eq!(answer, "已处理");
    let second = provider.received(1).unwrap();
    assert!(
        second[2]
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("未知工具"),
        "错误文本应喂回模型: {}",
        second[2].content.as_deref().unwrap_or_default()
    );
}

#[tokio::test]
async fn malformed_arguments_feed_error_back() {
    let tools = ToolBox::new().register(Box::new(AddTool));
    let provider = Arc::new(MockProvider::new([
        Response::with_tool_calls(vec![ToolCall::function("c1", "add", "{不是json")]),
        Response::simple("好"),
    ]));
    let agent = Agent::new(provider.clone()).with_tools(tools);
    agent.run("x").await.unwrap();

    let second = provider.received(1).unwrap();
    assert!(
        second[2]
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("TOOL_ERROR")
            && second[2]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("JSON"),
        "{}",
        second[2].content.as_deref().unwrap_or_default()
    );
}

#[tokio::test]
async fn tool_execution_error_does_not_abort_loop() {
    struct Boom;
    #[async_trait]
    impl Tool for Boom {
        fn name(&self) -> &str {
            "boom"
        }
        fn description(&self) -> &str {
            ""
        }
        fn schema(&self) -> Value {
            json!({})
        }
        async fn execute(&self, _args: Value) -> agent_core::Result<Value> {
            Err(Error::Config("爆炸了".into()))
        }
    }

    let provider = Arc::new(MockProvider::new([
        Response::with_tool_calls(vec![ToolCall::function("c1", "boom", "{}")]),
        Response::simple("收到错误并恢复"),
    ]));
    let agent = Agent::new(provider.clone()).with_tools(ToolBox::new().register(Box::new(Boom)));
    let answer = agent.run("x").await.unwrap();
    assert_eq!(answer, "收到错误并恢复");
    assert!(
        provider.received(1).unwrap()[2]
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("爆炸了")
    );
}

#[tokio::test]
async fn max_rounds_guard() {
    // 永远请求工具 → 必须在 max_rounds 轮后报错而非死循环
    let tc = || Response::with_tool_calls(vec![ToolCall::function("c", "add", r#"{"a":1,"b":2}"#)]);
    let responses: Vec<Response> = (0..100).map(|_| tc()).collect();
    let provider = Arc::new(MockProvider::new(responses));
    let agent = Agent::new(provider)
        .with_tools(ToolBox::new().register(Box::new(AddTool)))
        .with_max_rounds(3);

    let err = agent.run("x").await.unwrap_err();
    assert!(err.to_string().contains("超过最大轮数"), "{err}");
}

#[tokio::test]
async fn schemas_passed_to_provider() {
    let provider = Arc::new(MockProvider::one(Response::simple("ok")));
    let agent = Agent::new(provider).with_tools(ToolBox::new().register(Box::new(AddTool)));
    // MockProvider 不校验 tools 参数；这里只验证 run 能正常走通。
    // wire 格式正确性由 toolbox_schemas_wire_format 单测覆盖。
    agent.run("x").await.unwrap();
}

#[test]
fn toolbox_schemas_wire_format() {
    let tools = ToolBox::new().register(Box::new(AddTool));
    let schemas = tools.schemas();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0]["type"], "function");
    assert_eq!(schemas[0]["function"]["name"], "add");
    assert_eq!(schemas[0]["function"]["parameters"]["type"], "object");
    assert_eq!(schemas[0]["function"]["parameters"]["required"][0], "a");
}

#[test]
fn duplicate_tool_name_replaced() {
    #[derive(Default)]
    struct FakeAdd;
    #[async_trait]
    impl Tool for FakeAdd {
        fn name(&self) -> &str {
            "add"
        }
        fn description(&self) -> &str {
            "假的"
        }
        fn schema(&self) -> Value {
            json!({})
        }
        async fn execute(&self, _args: Value) -> agent_core::Result<Value> {
            Ok(json!("fake"))
        }
    }

    let tools = ToolBox::new()
        .register(Box::new(AddTool))
        .register(Box::new(FakeAdd));
    assert_eq!(tools.schemas().len(), 1);
    assert_eq!(tools.get("add").unwrap().description(), "假的");
}
