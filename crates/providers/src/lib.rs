//! providers：OpenAI-compatible LLM 客户端 + config.toml 解析 + 成本折算 + Model Registry。

pub mod client;
pub mod config;
pub mod cost;
pub mod registry;

pub use client::{OpenAiClient, parse_response, with_cancel};
pub use config::{AuthConfig, Config, ModelConfig, ProviderAuth, ProviderConfig};
pub use cost::estimate_cost;
pub use registry::{
    ModelPreset, PRESETS, ProviderPreset, custom_provider, custom_provider_multi, preset,
    provider_from_preset,
};
