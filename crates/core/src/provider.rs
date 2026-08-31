use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::Message;
use crate::message::ToolCall;

/// Provider 层统一错误。具体客户端（reqwest 等）自行转换到此类型，
/// 上层（agent loop / TUI）只认这一个错误类型。
#[derive(Debug, Error)]
pub enum Error {
    #[error("网络/传输错误: {0}")]
    Transport(String),

    #[error("API 返回错误 (HTTP {status}): {message}")]
    Api { status: u16, message: String },

    #[error("存储错误: {0}")]
    Storage(String),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("响应解析失败: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// D4 降级判定：只有 endpoint 明确不支持 `response_format: json_object`
    /// 一类的请求级错误（4xx 中除认证 401/403、限流 429 外的常见取值），
    /// 才值得降级为 prompt 约束重调。
    ///
    /// 认证失败/限流/服务端错误不应触发第二次付费调用——那是整条链路的问题，
    /// 换普通 chat 也一样会失败。
    #[must_use]
    pub fn is_json_mode_unsupported(&self) -> bool {
        matches!(
            self,
            Error::Api {
                status: 400 | 404 | 405 | 422,
                ..
            }
        )
    }
}

/// token 用量（R6 成本统计的数据源）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 提供方上下文缓存命中的 prompt tokens（DeepSeek prompt_cache_hit_tokens /
    /// OpenAI prompt_tokens_details.cached_tokens；未返回 = 0）。
    /// 费用折算时按缓存单价计——逐题生成的稳定素材前缀命中率极高，漏记会虚高数倍。
    #[serde(default)]
    pub cached_tokens: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// 一次 chat 调用的完整结果。
///
/// `reasoning` 为思考文本（决策 D7：兼容 reasoning_content / reasoning 双变体，
/// 单独存放，不计入 content；非思考模型为 None）。
/// `tool_calls` 非空表示模型请求调用工具——agent loop 执行后回填继续循环。
#[derive(Debug, Clone)]
pub struct Response {
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub model: String,
    pub finish_reason: Option<String>,
    pub usage: Usage,
}

/// LLM 提供方抽象。测试一律用 `MockProvider` + 录制 fixture，绝不真调外部 API。
#[async_trait]
pub trait Provider: Send + Sync {
    /// 发起一次 chat 补全。`tools` 为 JSON Schema 数组（M2 接入 tool call；
    /// 传空切片即普通对话）。含 usage 的完整响应随 Result 返回。
    async fn chat(&self, msgs: &[Message], tools: &[Value]) -> Result<Response>;
}

impl Response {
    /// 纯文本最终答案（测试/fixture 常用）。
    pub fn simple(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            reasoning: None,
            tool_calls: Vec::new(),
            model: "mock".into(),
            finish_reason: Some("stop".into()),
            usage: Usage::default(),
        }
    }

    /// 携带工具调用请求的响应（finish_reason=tool_calls）。
    pub fn with_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            content: String::new(),
            reasoning: None,
            tool_calls,
            model: "mock".into(),
            finish_reason: Some("tool_calls".into()),
            usage: Usage::default(),
        }
    }
}
