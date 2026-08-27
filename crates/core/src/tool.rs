//! Tool 抽象与工具集注册表。
//!
//! 决策 D6：只有只读检索型工具暴露给 agent loop；批量/费用敏感操作
//! 由命令直连流水线函数，不走这里。

use async_trait::async_trait;
use serde_json::Value;

use crate::provider::{Error, Result};

/// agent 可调用的工具。schema() 返回 JSON Schema 的 `parameters` 部分，
/// wire 格式由 [`ToolBox::schemas`] 统一拼装。
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    /// 执行工具。参数为模型给出的 JSON 对象；返回值会序列化为
    /// tool 消息内容回传给模型。
    async fn execute(&self, args: Value) -> Result<Value>;
}

/// 工具集：按名查找 + 生成 wire 格式 schemas。
#[derive(Default)]
pub struct ToolBox {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolBox {
    pub fn new() -> Self {
        Self::default()
    }

    /// 链式注册工具（重名后者覆盖前者，便于测试注入替身）。
    /// 链式注册工具（重名后者覆盖前者，便于测试注入替身）。
    pub fn register(mut self, tool: Box<dyn Tool>) -> Self {
        if let Some(slot) = self.tools.iter_mut().find(|t| t.name() == tool.name()) {
            *slot = tool;
        } else {
            self.tools.push(tool);
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// OpenAI tools 参数的 wire 格式。
    pub fn schemas(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.schema(),
                    }
                })
            })
            .collect()
    }
}

/// 把工具执行结果/错误规约为回传给模型的文本。
pub fn tool_output_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 统一把执行期错误转成文本（喂回模型而非中断循环）。
pub fn tool_error_text(err: &Error) -> String {
    format!("TOOL_ERROR: {err}")
}
