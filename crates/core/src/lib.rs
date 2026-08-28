//! core：Agent Loop、Tool trait、Provider trait、事件总线的家。
//!
//! M2 范围：Tool trait + ToolBox + agent loop（tool call 往返）。
//! 事件通道与中断属 M4；具体工具实现属 crates/tools（M7）。

pub mod agent;
pub mod context;
pub mod message;
pub mod mock;
pub mod parse;
pub mod provider;
pub mod tool;

pub use agent::{Agent, AgentResult, DEFAULT_MAX_ROUNDS, LoopEvent, LoopEventFn, ToolTraceEntry};
pub use context::{DEFAULT_CONTEXT_BUDGET_CHARS, trim_history};
pub use message::{Function, Message, Role, ToolCall};
pub use mock::MockProvider;
pub use parse::{first_json_block, trim_code_fence};
pub use provider::{Error, Provider, Response, Result, Usage};
pub use tool::{Tool, ToolBox};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let m = Message::user("你好");
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, r#"{"role":"user","content":"你好"}"#);
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn usage_total() {
        let u = Usage {
            prompt_tokens: 98,
            completion_tokens: 188,
        };
        assert_eq!(u.total(), 286);
    }

    /// wire 格式保真：assistant 带 tool_calls 与 tool 结果消息的序列化形态
    /// 必须能被 API 原样接受（DB 存原样 JSON 的前提）。
    #[test]
    fn tool_message_wire_format() {
        let assistant = Message::assistant_tool_calls(vec![ToolCall::function(
            "call_1",
            "search_notes",
            r#"{"query":"内存"}"#,
        )]);
        let json = serde_json::to_string(&assistant).unwrap();
        assert_eq!(
            json,
            r#"{"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"search_notes","arguments":"{\"query\":\"内存\"}"}}]}"#
        );

        let result = Message::tool_result("call_1", "命中 3 条");
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(
            json,
            r#"{"role":"tool","content":"命中 3 条","tool_call_id":"call_1"}"#
        );

        // 反序列化回同样结构
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
    }
}
