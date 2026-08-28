//! providers：OpenAI-compatible LLM 客户端 + config.toml 解析 + 成本折算。

pub mod client;
pub mod config;
pub mod cost;

pub use client::{OpenAiClient, parse_response, with_cancel};
pub use config::{Config, ProviderConfig};
pub use cost::estimate_cost;
