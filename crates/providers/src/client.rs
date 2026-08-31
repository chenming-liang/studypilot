//! OpenAI-compatible 客户端（非流式：本项目交互以 agent loop 的整段回复为主，
//! SSE 流式渲染不在当前范围；中断经 CancellationToken 由调用方 select 竞速实现）。

use std::time::Duration;

use agent_core::{Error, Function, Message, Provider, Response, Result, ToolCall, Usage};
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
    pub fn new(cfg: ProviderConfig) -> Result<Self> {
        let api_key = cfg.resolve_api_key()?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self { http, cfg, api_key })
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.cfg
    }

    /// D4：JSON mode 调用。不支持的 endpoint 会返回 Error::Api，
    /// 调用方据此降级为普通 chat + prompt 约束。
    pub async fn chat_json(&self, msgs: &[Message]) -> Result<Response> {
        self.chat_request(msgs, &[], true).await
    }

    async fn chat_request(
        &self,
        msgs: &[Message],
        tools: &[Value],
        json_mode: bool,
    ) -> Result<Response> {
        let url = format!(
            "{}/chat/completions",
            self.cfg.endpoint.trim_end_matches('/')
        );

        let mut body = json!({
            "model": self.cfg.model,
            "messages": msgs,
        });
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
