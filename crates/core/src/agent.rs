//! Agent loop：解析 tool_calls → 执行 → 回填 → 循环，直到最终答案。
//!
//! R4 的事件通道（进度上报/CancellationToken 中断）属 M4，届时在
//! `run` 的循环体里挂钩子即可；M2 先用 tracing 日志暴露完整往返。

use std::sync::Arc;

use serde_json::Value;

use crate::message::{Message, ToolCall};
use crate::provider::{Error, Provider, Result, Usage};
use crate::tool::{ToolBox, tool_error_text, tool_output_to_string};

pub const DEFAULT_MAX_ROUNDS: usize = 12;

/// agent loop 运行结果：最终答案 + 累计 usage + 循环中新增的消息 + 工具调用轨迹。
#[derive(Debug, Clone)]
pub struct AgentResult {
    pub content: String,
    pub reasoning_chars: Option<usize>,
    pub total_usage: Usage,
    /// 循环期间新增的消息（assistant tool_calls / tool results / 最终 assistant）
    pub new_messages: Vec<Message>,
    /// 工具调用轨迹（供 TUI 展示 Tool trace）
    pub tool_trace: Vec<ToolTraceEntry>,
}

/// 单次工具调用记录。
#[derive(Debug, Clone)]
pub struct ToolTraceEntry {
    pub tool_name: String,
    pub query: String,
    pub hit_count: usize,
    pub ok: bool,
}

#[derive(Clone)]
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: Arc<ToolBox>,
    system_prompt: Option<Arc<str>>,
    max_rounds: usize,
    /// Context 预算（字符）；None = 不裁剪。默认启用 DEFAULT_CONTEXT_BUDGET_CHARS。
    context_budget: Option<usize>,
}

impl Agent {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            tools: Arc::new(ToolBox::new()),
            system_prompt: None,
            max_rounds: DEFAULT_MAX_ROUNDS,
            context_budget: Some(crate::context::DEFAULT_CONTEXT_BUDGET_CHARS),
        }
    }

    pub fn with_tools(mut self, tools: ToolBox) -> Self {
        self.tools = Arc::new(tools);
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        let prompt: String = prompt.into();
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_max_rounds(mut self, max_rounds: usize) -> Self {
        self.max_rounds = max_rounds;
        self
    }

    /// 自定义上下文字符预算（Context Manager，超限裁掉最老的轮次）。
    #[must_use]
    pub fn with_context_budget(mut self, budget_chars: usize) -> Self {
        self.context_budget = Some(budget_chars);
        self
    }

    /// 关闭历史裁剪（全量发送，行为回到旧版）。
    #[must_use]
    pub fn without_context_trim(mut self) -> Self {
        self.context_budget = None;
        self
    }

    /// 单轮入口（M2 兼容）：从用户输入到最终文本答案。
    pub async fn run(&self, user_input: &str) -> Result<String> {
        let mut msgs: Vec<Message> = Vec::new();
        if let Some(sys) = &self.system_prompt {
            msgs.push(Message::system(sys.to_string()));
        }
        msgs.push(Message::user(user_input));
        let result = self.run_loop(&mut msgs).await?;
        Ok(result.content)
    }

    /// 多轮入口（M7）：传入已有历史（不含 system prompt），返回完整结果。
    /// 超预算时先经 Context Manager 裁剪（tool_calls 配对不会被拆散）。
    pub async fn run_messages(&self, history: &[Message]) -> Result<AgentResult> {
        let trimmed = match self.context_budget {
            Some(budget) => crate::context::trim_history(history, budget),
            None => crate::context::TrimmedHistory {
                messages: history.to_vec(),
                omitted_rounds: 0,
                omitted_chars: 0,
            },
        };
        if trimmed.omitted_rounds > 0 {
            tracing::info!(
                omitted_rounds = trimmed.omitted_rounds,
                omitted_chars = trimmed.omitted_chars,
                "context manager: 早期对话已省略"
            );
        }
        let mut msgs: Vec<Message> = Vec::new();
        if let Some(sys) = &self.system_prompt {
            msgs.push(Message::system(sys.to_string()));
        }
        msgs.extend(trimmed.messages);
        self.run_loop(&mut msgs).await
    }

    /// 核心循环：provider.chat → 有 tool_calls 则执行回填 → 无则最终答案。
    async fn run_loop(&self, msgs: &mut Vec<Message>) -> Result<AgentResult> {
        let initial_len = msgs.len();
        let mut total_usage = Usage::default();
        let mut last_reasoning: Option<usize>;
        let mut search_count: usize = 0;
        let mut tool_trace: Vec<ToolTraceEntry> = Vec::new();

        for round in 1..=self.max_rounds {
            let resp = self.provider.chat(msgs, &self.tools.schemas()).await?;
            total_usage.prompt_tokens += resp.usage.prompt_tokens;
            total_usage.completion_tokens += resp.usage.completion_tokens;
            last_reasoning = resp.reasoning.as_ref().map(|r| r.chars().count());

            if resp.tool_calls.is_empty() {
                tracing::info!(round, "agent loop: 最终答案");
                msgs.push(Message::assistant(&resp.content));
                let new_messages = msgs[initial_len..].to_vec();
                return Ok(AgentResult {
                    content: resp.content,
                    reasoning_chars: last_reasoning,
                    total_usage,
                    new_messages,
                    tool_trace,
                });
            }

            tracing::info!(
                round,
                count = resp.tool_calls.len(),
                "agent loop: 收到 tool calls"
            );
            msgs.push(Message::assistant_tool_calls(resp.tool_calls.clone()));

            for tc in &resp.tool_calls {
                // 检索型工具防重：search_notes 最多 3 次真实检索，且 query 不能重复。
                // 注意：计数/轨迹只在真正执行时登记；坏参（q 提取失败）透传 execute_tool
                // 让工具自身的参数校验回错误给模型修正，而不是谎报"重复查询"。
                let output = if tc.function.name == "search_notes" {
                    let args: Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    let q = args
                        .get("query")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from);

                    match q {
                        None => self.execute_tool(tc).await,
                        Some(q) => {
                            if tool_trace.iter().any(|t| t.query == q) {
                                tracing::info!(round, query = %q, "重复 query 被跳过");
                                format!("查询「{q}」已搜过，请换关键词或根据已有结果回答。")
                            } else if search_count >= 3 {
                                tracing::info!(round, "search_notes 已达 3 次上限");
                                "检索次数已达上限，请根据已有结果回答。".to_owned()
                            } else {
                                search_count += 1;
                                let result = self.execute_tool(tc).await;
                                // 从 tool 输出 JSON 提取 hit_count
                                let hit_count = serde_json::from_str::<Value>(&result)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("results").and_then(|r| r.as_array()).map(|a| a.len())
                                    })
                                    .unwrap_or(0);
                                tool_trace.push(ToolTraceEntry {
                                    tool_name: tc.function.name.clone(),
                                    query: q,
                                    hit_count,
                                    ok: !result.starts_with("TOOL_ERROR"),
                                });
                                result
                            }
                        }
                    }
                } else {
                    self.execute_tool(tc).await
                };
                tracing::info!(
                    round,
                    tool = %tc.function.name,
                    id = %tc.id,
                    ok = !output.starts_with("TOOL_ERROR"),
                    "agent loop: 工具执行完成"
                );
                msgs.push(Message::tool_result(&tc.id, output));
            }
        }

        Err(Error::Config(format!(
            "超过最大轮数 {max_rounds}，模型仍未给出最终答案",
            max_rounds = self.max_rounds
        )))
    }

    /// 执行单个 tool call：参数解析失败/未知工具/执行失败都不中断循环，
    /// 统一转成错误文本喂回模型自行调整。
    async fn execute_tool(&self, tc: &ToolCall) -> String {
        let Some(tool) = self.tools.get(&tc.function.name) else {
            let err = Error::Config(format!("未知工具 `{}`", tc.function.name));
            tracing::warn!(tool = %tc.function.name, "{err}");
            return tool_error_text(&err);
        };

        let args: Value = match serde_json::from_str(&tc.function.arguments) {
            Ok(v) => v,
            Err(e) => {
                let err = Error::Parse(format!(
                    "工具 `{}` 参数不是合法 JSON: {e}",
                    tc.function.name
                ));
                tracing::warn!("{err}");
                return tool_error_text(&err);
            }
        };

        match tool.execute(args).await {
            Ok(v) => tool_output_to_string(&v),
            Err(e) => {
                tracing::warn!(tool = %tc.function.name, "工具执行失败: {e}");
                tool_error_text(&e)
            }
        }
    }
}
