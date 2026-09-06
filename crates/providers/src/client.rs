//! OpenAI-compatible 客户端（非流式：本项目交互以 agent loop 的整段回复为主，
//! SSE 流式渲染不在当前范围；中断经 CancellationToken 由调用方 select 竞速实现）。

use std::time::Duration;

use agent_core::{Error, Function, Message, Provider, Response, Result, Role, ToolCall, Usage};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::config::ProviderConfig;

/// TCP 连接超时。连接建立是纯网络问题，与 LLM 生成无关，短超时快速失败。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 单次请求整体超时。LLM 长生成 + 思考模型可能需要数分钟，
/// 无超时会让 async 任务永久挂起（取消令牌也只能放弃等待）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// 取消感知的 provider 调用包装：等待期间 `cancel` 触发立即返回 `None`（不等 LLM 返回）。
/// 返回 `None` 时调用方用 `cancel.is_cancelled()` 区分「用户取消」与「调用失败」。
/// 所有 LLM 调用点（聊天/出题/大纲/导入抽取）都应经此包装，否则 Ctrl+C 期间
/// HTTP await 不响应取消，界面保持"思考中"直到请求自然返回。
pub async fn with_cancel<F>(f: F, cancel: &CancellationToken) -> Option<F::Output>
where
    F: std::future::Future,
{
    tokio::select! {
        out = f => Some(out),
        _ = cancel.cancelled() => None,
    }
}

pub struct OpenAiClient {
    http: reqwest::Client,
    cfg: ProviderConfig,
    api_key: String,
}

impl OpenAiClient {
    /// 构造客户端。**容忍缺 API key**（文档：provider incomplete ≠ config invalid）——
    /// key 为空时仍可启动，请求时返回可操作的配置错误；startup 不 crash。
    pub fn new(cfg: ProviderConfig) -> Result<Self> {
        let api_key = cfg.resolve_api_key().unwrap_or_default();
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self { http, cfg, api_key })
    }

    /// 是否配置了 API key（明文或 env 均可）。
    pub fn configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.cfg
    }

    /// D4：JSON mode 调用。不支持的 endpoint 会返回 Error::Api，
    /// 调用方据此降级为普通 chat + prompt 约束。
    pub async fn chat_json(&self, msgs: &[Message]) -> Result<Response> {
        self.chat_request(msgs, &[], true).await
    }

    /// 是否 Anthropic Messages API（端点含 anthropic.com）。
    fn is_anthropic(&self) -> bool {
        self.cfg.endpoint.contains("anthropic.com")
    }

    /// 是否本地免 key 端点（Ollama 等 localhost）。
    fn is_keyless_local(&self) -> bool {
        self.cfg.endpoint.contains("localhost") || self.cfg.endpoint.contains("127.0.0.1")
    }

    /// 是否支持 thinking 开关参数（DeepSeek V4 OpenAI 兼容格式 `{"type":"enabled|disabled"}`），
    /// 仅据此在请求体传 thinking —— 其余 OpenAI-compatible provider 不硬塞，保持默认行为。
    fn is_thinking_capable(&self) -> bool {
        self.cfg.endpoint.contains("deepseek.com")
    }

    async fn chat_request(
        &self,
        msgs: &[Message],
        tools: &[Value],
        json_mode: bool,
    ) -> Result<Response> {
        if !self.configured() && !self.is_keyless_local() {
            return Err(Error::Config(
                "AI 尚未配置 API key：Ctrl+K → Model 选择 provider 并设置 key".into(),
            ));
        }
        if self.is_anthropic() {
            return self.anthropic_request(msgs, tools).await;
        }
        let url = format!(
            "{}/chat/completions",
            self.cfg.endpoint.trim_end_matches('/')
        );

        let mut body = json!({
            "model": self.cfg.model,
            "messages": msgs,
        });
        // thinking 由 Model Role 决定（fast/balanced 关、reasoning 开）。
        // 仅为支持 thinking 参数的 provider 传参（DeepSeek V4 的 OpenAI 兼容格式），
        // 其他 OpenAI-compatible provider 不硬塞，保持其默认行为。
        if self.is_thinking_capable() {
            body["thinking"] =
                json!({"type": if self.cfg.thinking { "enabled" } else { "disabled" }});
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
        if json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }

        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(http_error(status.as_u16(), &text));
        }

        let v: Value = serde_json::from_str(&text).map_err(|e| Error::Parse(e.to_string()))?;
        parse_response(&v)
    }

    /// Anthropic Messages API（`POST /messages`）：
    /// 请求/响应结构与 OpenAI chat/completions 不同，这里做 wire 转换。
    /// 非流式（与 OpenAiClient 一致）；token 计数走 input_tokens/output_tokens。
    async fn anthropic_request(&self, msgs: &[Message], tools: &[Value]) -> Result<Response> {
        if !self.configured() {
            return Err(Error::Config(
                "AI 尚未配置 API key：Ctrl+K → Model 选择 provider 并设置 key".into(),
            ));
        }
        let url = format!("{}/messages", self.cfg.endpoint.trim_end_matches('/'));
        let body = anthropic_body(&self.cfg, msgs, tools);

        let resp = self
            .http
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(http_error(status.as_u16(), &text));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| Error::Parse(e.to_string()))?;
        parse_anthropic_response(&v)
    }
}

/// 构造 Anthropic Messages API 请求体（纯函数，可单测）。
/// - system 消息抽作顶层字段；role=tool → user 消息 + tool_result block
/// - tools：OpenAI wire（type:function+function.parameters）→ Anthropic（name+input_schema）
fn anthropic_body(cfg: &ProviderConfig, msgs: &[Message], tools: &[Value]) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for m in msgs {
        match m.role {
            Role::System => {
                if let Some(c) = &m.content {
                    system_parts.push(c.clone());
                }
            }
            Role::User => {
                messages.push(json!({"role": "user", "content": m.content}));
            }
            Role::Assistant => {
                messages.push(anthropic_assistant_message(m));
            }
            Role::Tool => {
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": m.tool_call_id.as_deref().unwrap_or(""),
                        "content": m.content.as_deref().unwrap_or(""),
                    }]
                }));
            }
        }
    }
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": cfg.context_length.clamp(64, 4096),
        "messages": messages,
    });
    if !system_parts.is_empty() {
        body["system"] = Value::String(system_parts.join("\n\n"));
    }
    if !tools.is_empty() {
        let an_tools: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let f = t.get("function")?;
                Some(json!({
                    "name": f.get("name")?.as_str()?,
                    "description": f.get("description").and_then(Value::as_str).unwrap_or(""),
                    "input_schema": f.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
                }))
            })
            .collect();
        if !an_tools.is_empty() {
            body["tools"] = Value::Array(an_tools);
        }
    }
    body
}

/// Anthropic assistant 消息转换：content 文本 + tool_use blocks。
fn anthropic_assistant_message(m: &Message) -> Value {
    let mut content: Vec<Value> = Vec::new();
    if let Some(text) = &m.content
        && !text.is_empty()
    {
        content.push(json!({"type": "text", "text": text}));
    }
    for tc in &m.tool_calls {
        let input: Value = serde_json::from_str(&tc.function.arguments).unwrap_or(Value::Null);
        content.push(json!({
            "type": "tool_use",
            "id": tc.id,
            "name": tc.function.name,
            "input": input,
        }));
    }
    json!({"role": "assistant", "content": content})
}

/// 纯函数：从 Anthropic Messages 非流式 JSON 提取 `Response`。
pub fn parse_anthropic_response(v: &Value) -> Result<Response> {
    let content = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    // tool_use blocks → ToolCall（arguments 由 input 对象序列化）
    let tool_calls: Vec<ToolCall> = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| {
                    if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                        return None;
                    }
                    Some(ToolCall {
                        id: b.get("id")?.as_str()?.to_owned(),
                        kind: "function".into(),
                        function: Function {
                            name: b.get("name")?.as_str()?.to_owned(),
                            arguments: b.get("input").cloned().unwrap_or(Value::Null).to_string(),
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let reasoning = v
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b.get("type").and_then(Value::as_str) == Some("thinking"))
        })
        .and_then(|b| b.get("thinking").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(String::from);
    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let finish_reason = v
        .get("stop_reason")
        .and_then(Value::as_str)
        .map(String::from);
    let usage = v
        .get("usage")
        .filter(|u| !u.is_null())
        .map(|u| Usage {
            prompt_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            completion_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            cached_tokens: 0,
        })
        .unwrap_or_default();
    Ok(Response {
        content,
        reasoning,
        tool_calls,
        model,
        finish_reason,
        usage,
    })
}

#[async_trait]
impl Provider for OpenAiClient {
    async fn chat(&self, msgs: &[Message], tools: &[Value]) -> Result<Response> {
        self.chat_request(msgs, tools, false).await
    }
}

/// 把带 error 字段的响应体转成尽量可读的错误信息。
fn http_error(status: u16, body: &str) -> Error {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| body.chars().take(300).collect());
    Error::Api { status, message }
}

/// 纯函数：从非流式 chat.completion JSON 提取 `Response`。
/// 不碰网络——单测直接喂录制的 fixture。
pub fn parse_response(v: &Value) -> Result<Response> {
    let choice = v
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| Error::Parse(format!("响应缺少 choices: {}", brief(v))))?;

    let message = choice
        .get("message")
        .ok_or_else(|| Error::Parse(format!("choices[0] 缺少 message: {}", brief(v))))?;

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    // D7：reasoning_content 优先，兼容 reasoning 变体；空串视同无思考
    let reasoning = ["reasoning_content", "reasoning"]
        .iter()
        .find_map(|k| message.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(String::from);

    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(String::from);
    // M2：tool call 提取（OpenAI wire: choices[0].message.tool_calls）
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_tool_call).collect())
        .unwrap_or_default();
    // usage 缺失/null/字段非法都回退 0——R6 少记一次总好过拒答合法响应
    let usage = v
        .get("usage")
        .filter(|u| !u.is_null())
        .and_then(|u| parse_usage(u).ok())
        .unwrap_or_default();

    Ok(Response {
        content,
        reasoning,
        tool_calls,
        model,
        finish_reason,
        usage,
    })
}

/// 单个 tool_call 的解析；字段缺失的条目跳过（不因脏数据丢整响应）。
fn parse_tool_call(v: &Value) -> Option<ToolCall> {
    let function = v.get("function")?;
    Some(ToolCall {
        id: v.get("id")?.as_str()?.to_owned(),
        kind: v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .to_owned(),
        function: Function {
            name: function.get("name")?.as_str()?.to_owned(),
            arguments: function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
    })
}

fn parse_usage(u: &Value) -> Result<Usage> {
    // 缓存命中 tokens：DeepSeek 顶层 prompt_cache_hit_tokens；
    // OpenAI 嵌套 prompt_tokens_details.cached_tokens——两家都收，无则 0
    let cached = u
        .get("prompt_cache_hit_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            u.get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    Ok(Usage {
        prompt_tokens: int_field(u, "prompt_tokens")?,
        completion_tokens: int_field(u, "completion_tokens")?,
        cached_tokens: cached,
    })
}

fn int_field(v: &Value, key: &str) -> Result<u64> {
    v.get(key)
        .and_then(Value::as_u64)
        .map_or_else(|| Err(Error::Parse(format!("usage 缺少 {key}"))), Ok)
}

fn brief(v: &Value) -> String {
    let s = v.to_string();
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 M0 实测（D7）构造的 deepseek-reasoner 非流式响应 fixture。
    #[test]
    fn parse_reasoning_content_variant() {
        let v = serde_json::json!({
            "id": "x", "object": "chat.completion",
            "model": "deepseek-v4-flash",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "9.8 大。",
                    "reasoning_content": "We need compare..."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 98, "completion_tokens": 188,
                      "completion_tokens_details": {"reasoning_tokens": 159}}
        });
        let r = parse_response(&v).unwrap();
        assert_eq!(r.content, "9.8 大。");
        assert_eq!(r.reasoning.as_deref(), Some("We need compare..."));
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
        assert_eq!(r.usage.prompt_tokens, 98);
        assert_eq!(r.usage.completion_tokens, 188);
        assert_eq!(r.model, "deepseek-v4-flash");
    }

    /// OpenAI 系变体：字段叫 `reasoning`（D7 双变体兼容）。
    #[test]
    fn parse_reasoning_variant() {
        let v = serde_json::json!({
            "model": "gpt-4o-mini",
            "choices": [{"message": {"content": "hi", "reasoning": "think"}, "finish_reason": null}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        });
        let r = parse_response(&v).unwrap();
        assert_eq!(r.reasoning.as_deref(), Some("think"));
        assert_eq!(r.finish_reason, None);
    }

    #[test]
    fn parse_non_thinking_and_missing_usage() {
        let v = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
        });
        let r = parse_response(&v).unwrap();
        assert_eq!(r.content, "ok");
        assert_eq!(r.reasoning, None);
        assert_eq!(r.usage, Usage::default());
    }

    /// Anthropic Messages 响应解析（文本 + tool_use + input/output tokens）。
    #[test]
    fn parse_anthropic_response_text_and_tool_use() {
        let v = serde_json::json!({
            "model": "claude-sonnet-4",
            "content": [
                {"type": "thinking", "thinking": "let me think"},
                {"type": "text", "text": "先查笔记。"},
                {"type": "tool_use", "id": "toolu_01", "name": "search_notes",
                 "input": {"query": "所有权"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 120, "output_tokens": 30}
        });
        let r = parse_anthropic_response(&v).unwrap();
        assert_eq!(r.content, "先查笔记。");
        assert_eq!(r.reasoning.as_deref(), Some("let me think"));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].function.name, "search_notes");
        assert_eq!(r.tool_calls[0].function.arguments, r#"{"query":"所有权"}"#);
        assert_eq!(r.finish_reason.as_deref(), Some("tool_use"));
        assert_eq!(r.usage.prompt_tokens, 120);
        assert_eq!(r.usage.completion_tokens, 30);
        assert_eq!(r.model, "claude-sonnet-4");
    }

    /// Anthropic 请求体构造：system 抽顶层、role=tool → tool_result block、
    /// OpenAI tools wire → Anthropic input_schema。
    #[test]
    fn anthropic_body_maps_messages_and_tools() {
        let cfg = crate::config::ProviderConfig {
            name: "anthropic".into(),
            endpoint: "https://api.anthropic.com/v1".into(),
            api_key: None,
            api_key_env: None,
            model: "claude-sonnet-4".into(),
            models: Vec::new(),
            price_prompt: None,
            price_completion: None,
            price_prompt_cached: None,
            context_length: 200000,
            thinking: false,
        };
        let msgs = vec![
            Message::system("你是导师。"),
            Message::user("所有权是什么"),
            Message::assistant_tool_calls(vec![ToolCall::function(
                "toolu_01",
                "search_notes",
                r#"{"query":"所有权"}"#,
            )]),
            Message::tool_result("toolu_01", "找到 3 条"),
            Message::user("继续"),
        ];
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "search_notes",
                "description": "检索",
                "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}
            }
        })];
        let body = anthropic_body(&cfg, &msgs, &tools);
        assert_eq!(body["model"], "claude-sonnet-4");
        assert_eq!(body["system"], "你是导师。");
        assert_eq!(body["max_tokens"], 4096, "context 200k 但截到 4096");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "system 已抽到顶层，不占 messages");
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "所有权是什么");
        // assistant tool_use block
        let asst = &msgs[1];
        assert_eq!(asst["role"], "assistant");
        assert_eq!(asst["content"][0]["type"], "tool_use");
        assert_eq!(asst["content"][0]["name"], "search_notes");
        // tool result → user + tool_result block
        let tr = &msgs[2];
        assert_eq!(tr["role"], "user");
        assert_eq!(tr["content"][0]["type"], "tool_result");
        assert_eq!(tr["content"][0]["tool_use_id"], "toolu_01");
        assert_eq!(tr["content"][0]["content"], "找到 3 条");
        // tools 转换
        let ts = body["tools"].as_array().unwrap();
        assert_eq!(ts[0]["name"], "search_notes");
        assert!(ts[0].get("input_schema").is_some());
        assert!(ts[0].get("function").is_none(), "OpenAI wire 不应残留");
    }

    #[test]
    fn empty_reasoning_string_is_none() {
        let v = serde_json::json!({
            "choices": [{"message": {"content": "ok", "reasoning_content": ""}}]
        });
        assert_eq!(parse_response(&v).unwrap().reasoning, None);
    }

    #[test]
    fn missing_choices_is_parse_error() {
        let v = serde_json::json!({"error": {"message": "boom"}});
        let err = parse_response(&v).unwrap_err();
        assert!(err.to_string().contains("缺少 choices"), "{err}");
    }

    /// M0 实测录制的 tool_calls 响应结构（M2）。
    #[test]
    fn parse_tool_calls() {
        let v = serde_json::json!({
            "model": "deepseek-v4-flash",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "search_notes",
                            "arguments": "{\"query\":\"所有权\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20}
        });
        let r = parse_response(&v).unwrap();
        assert_eq!(r.tool_calls.len(), 1);
        let tc = &r.tool_calls[0];
        assert_eq!(tc.id, "call_abc");
        assert_eq!(tc.kind, "function");
        assert_eq!(tc.function.name, "search_notes");
        assert_eq!(tc.function.arguments, r#"{"query":"所有权"}"#);
        assert_eq!(r.finish_reason.as_deref(), Some("tool_calls"));
    }

    /// 缺 id/name 的脏条目应被跳过而不是报错。
    #[test]
    fn malformed_tool_call_entry_skipped() {
        let v = serde_json::json!({
            "choices": [{"message": {"tool_calls": [
                {"type": "function", "function": {"name": "no_id", "arguments": "{}"}},
                {"id": "ok1", "type": "function", "function": {"name": "good", "arguments": "{}"}}
            ]}}]
        });
        let r = parse_response(&v).unwrap();
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "ok1");
    }
}

#[cfg(test)]
mod with_cancel_tests {
    use super::*;

    #[tokio::test]
    async fn cancel_triggers_immediate_none() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        // 已取消：即使 future 永不完成也立即返回 None（不等 LLM 返回）
        let r = with_cancel(std::future::pending::<()>(), &cancel).await;
        assert_eq!(r, None);
    }

    #[tokio::test]
    async fn completes_returns_output() {
        let cancel = CancellationToken::new();
        let r = with_cancel(async { 42 }, &cancel).await;
        assert_eq!(r, Some(42));
    }
}
