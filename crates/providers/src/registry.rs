//! Model Registry（文档 §七）：内置 OpenAI-compatible provider 预设 + 自定义 provider。
//!
//! 目标：普通用户不需要手填模型价格——内置预设带 pricing 元数据，
//! 未知/自定义模型 pricing optional（显示 Cost tracking unavailable 但可正常使用）。
//!
//! 协议说明：除 Anthropic 走独立 Messages API（client.rs 内部分支）外，其余预设全部
//! OpenAI-compatible（`{endpoint}/chat/completions` + Bearer）；Gemini 用 Google 官方
//! OpenAI 兼容端点；Ollama 本地免 key。

use crate::config::ProviderConfig;

/// 内置 provider 预设（display name 用友好名，不暴露内部 id）。
pub struct ProviderPreset {
    /// 机器 id（写入 config 的 provider.name）
    pub id: &'static str,
    /// 用户可见名称
    pub display_name: &'static str,
    /// OpenAI-compatible 端点（`{endpoint}/chat/completions`）；Anthropic 用其 Messages 端点
    pub endpoint: &'static str,
    /// 可选模型
    pub models: &'static [ModelPreset],
}

/// 一个模型预设：内置模型带 pricing 元数据（可选）。
pub struct ModelPreset {
    pub id: &'static str,
    pub display_name: &'static str,
    /// 元 / 百万 token（人民币；近似官方定价，以厂商官网为准）
    pub price_prompt: Option<f64>,
    pub price_completion: Option<f64>,
    /// 上下文缓存命中单价（可选；未配置按全价保守估算）
    pub price_prompt_cached: Option<f64>,
    pub context_length: u64,
    pub thinking: bool,
}

/// 内置 provider 预设列表（2026-09：各厂商当前主流常用模型）。
pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "deepseek",
        display_name: "DeepSeek",
        endpoint: "https://api.deepseek.com/v1",
        models: &[
            ModelPreset {
                id: "deepseek-reasoner",
                display_name: "DeepSeek Reasoner (R1)",
                price_prompt: Some(4.0),
                price_completion: Some(16.0),
                price_prompt_cached: Some(1.0),
                context_length: 128000,
                thinking: true,
            },
            ModelPreset {
                id: "deepseek-chat",
                display_name: "DeepSeek Chat (V3)",
                price_prompt: Some(0.27),
                price_completion: Some(1.1),
                price_prompt_cached: Some(0.0675),
                context_length: 128000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "qwen",
        display_name: "Qwen (通义千问)",
        endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &[
            ModelPreset {
                id: "qwen3-max",
                display_name: "Qwen3 Max",
                price_prompt: Some(4.0),
                price_completion: Some(16.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen3-plus",
                display_name: "Qwen3 Plus",
                price_prompt: Some(1.2),
                price_completion: Some(2.4),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen3-turbo",
                display_name: "Qwen3 Turbo",
                price_prompt: Some(0.3),
                price_completion: Some(0.6),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen-max",
                display_name: "Qwen Max",
                price_prompt: Some(2.4),
                price_completion: Some(9.6),
                price_prompt_cached: None,
                context_length: 32000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen-long",
                display_name: "Qwen Long",
                price_prompt: Some(0.3),
                price_completion: Some(0.3),
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "glm",
        display_name: "GLM (Zhipu)",
        endpoint: "https://open.bigmodel.cn/api/paas/v4",
        models: &[
            ModelPreset {
                id: "glm-4-plus",
                display_name: "GLM-4-Plus",
                price_prompt: Some(4.0),
                price_completion: Some(4.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "glm-4-air",
                display_name: "GLM-4-Air",
                price_prompt: Some(0.5),
                price_completion: Some(0.5),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "glm-4-flash",
                display_name: "GLM-4-Flash",
                price_prompt: Some(0.0),
                price_completion: Some(0.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "glm-4-long",
                display_name: "GLM-4-Long",
                price_prompt: Some(0.05),
                price_completion: Some(0.1),
                price_prompt_cached: None,
                context_length: 320000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "kimi",
        display_name: "Kimi (Moonshot)",
        endpoint: "https://api.moonshot.cn/v1",
        models: &[
            ModelPreset {
                id: "kimi-latest",
                display_name: "Kimi Latest",
                price_prompt: Some(4.0),
                price_completion: Some(12.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "kimi-k2",
                display_name: "Kimi K2",
                price_prompt: Some(4.0),
                price_completion: Some(16.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "moonshot-v1-128k",
                display_name: "Moonshot V1 128K",
                price_prompt: Some(12.0),
                price_completion: Some(12.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "moonshot-v1-32k",
                display_name: "Moonshot V1 32K",
                price_prompt: Some(8.0),
                price_completion: Some(8.0),
                price_prompt_cached: None,
                context_length: 32000,
                thinking: false,
            },
            ModelPreset {
                id: "moonshot-v1-8k",
                display_name: "Moonshot V1 8K",
                price_prompt: Some(4.0),
                price_completion: Some(4.0),
                price_prompt_cached: None,
                context_length: 8000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "minimax",
        display_name: "MiniMax",
        endpoint: "https://api.minimaxi.com/v1",
        models: &[
            ModelPreset {
                id: "MiniMax-Text-01",
                display_name: "MiniMax Text-01",
                price_prompt: Some(1.0),
                price_completion: Some(8.0),
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
            ModelPreset {
                id: "MiniMax-M1",
                display_name: "MiniMax M1",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 128000,
                thinking: true,
            },
            ModelPreset {
                id: "abab6.5s-chat",
                display_name: "ABAB 6.5s",
                price_prompt: Some(1.0),
                price_completion: Some(8.0),
                price_prompt_cached: None,
                context_length: 245000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "doubao",
        display_name: "Doubao (火山方舟)",
        endpoint: "https://ark.cn-beijing.volces.com/api/v3",
        models: &[
            ModelPreset {
                id: "doubao-seed-1-6-flash",
                display_name: "Doubao Seed 1.6 Flash",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "doubao-1.5-pro-32k",
                display_name: "Doubao 1.5 Pro 32K",
                price_prompt: Some(0.8),
                price_completion: Some(2.0),
                price_prompt_cached: None,
                context_length: 32000,
                thinking: false,
            },
            ModelPreset {
                id: "doubao-1.5-pro-256k",
                display_name: "Doubao 1.5 Pro 256K",
                price_prompt: Some(0.8),
                price_completion: Some(2.0),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "doubao-1.5-lite-32k",
                display_name: "Doubao 1.5 Lite 32K",
                price_prompt: Some(0.3),
                price_completion: Some(0.6),
                price_prompt_cached: None,
                context_length: 32000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "hunyuan",
        display_name: "Hunyuan (腾讯混元)",
        endpoint: "https://api.hunyuan.cloud.tencent.com/v1",
        models: &[
            ModelPreset {
                id: "hunyuan-turbos",
                display_name: "Hunyuan TurboS",
                price_prompt: Some(1.0),
                price_completion: Some(3.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "hunyuan-pro",
                display_name: "Hunyuan Pro",
                price_prompt: Some(4.0),
                price_completion: Some(12.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "hunyuan-standard",
                display_name: "Hunyuan Standard",
                price_prompt: Some(0.5),
                price_completion: Some(1.5),
                price_prompt_cached: None,
                context_length: 32000,
                thinking: false,
            },
            ModelPreset {
                id: "hunyuan-lite",
                display_name: "Hunyuan Lite",
                price_prompt: Some(0.0),
                price_completion: Some(0.0),
                price_prompt_cached: None,
                context_length: 8000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "ernie",
        display_name: "ERNIE (百度千帆)",
        endpoint: "https://qianfan.baidubce.com/v2",
        models: &[
            ModelPreset {
                id: "ernie-4.0-8k",
                display_name: "ERNIE 4.0 8K",
                price_prompt: Some(20.0),
                price_completion: Some(20.0),
                price_prompt_cached: None,
                context_length: 8000,
                thinking: false,
            },
            ModelPreset {
                id: "ernie-3.5-8k",
                display_name: "ERNIE 3.5 8K",
                price_prompt: Some(0.8),
                price_completion: Some(2.0),
                price_prompt_cached: None,
                context_length: 8000,
                thinking: false,
            },
            ModelPreset {
                id: "ernie-speed-8k",
                display_name: "ERNIE Speed 8K",
                price_prompt: Some(0.2),
                price_completion: Some(0.4),
                price_prompt_cached: None,
                context_length: 8000,
                thinking: false,
            },
            ModelPreset {
                id: "ernie-lite-8k",
                display_name: "ERNIE Lite 8K",
                price_prompt: Some(0.03),
                price_completion: Some(0.03),
                price_prompt_cached: None,
                context_length: 8000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "openai",
        display_name: "OpenAI",
        endpoint: "https://api.openai.com/v1",
        models: &[
            ModelPreset {
                id: "gpt-4o",
                display_name: "GPT-4o",
                price_prompt: Some(11.0),
                price_completion: Some(44.0),
                price_prompt_cached: Some(2.75),
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "gpt-4o-mini",
                display_name: "GPT-4o mini",
                price_prompt: Some(1.1),
                price_completion: Some(4.4),
                price_prompt_cached: Some(0.275),
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "gpt-4.1",
                display_name: "GPT-4.1",
                price_prompt: Some(10.0),
                price_completion: Some(40.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "gpt-4.1-mini",
                display_name: "GPT-4.1 mini",
                price_prompt: Some(1.0),
                price_completion: Some(4.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "o3",
                display_name: "o3",
                price_prompt: Some(30.0),
                price_completion: Some(60.0),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: true,
            },
            ModelPreset {
                id: "o4-mini",
                display_name: "o4-mini",
                price_prompt: Some(6.0),
                price_completion: Some(24.0),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: true,
            },
        ],
    },
    ProviderPreset {
        id: "anthropic",
        display_name: "Anthropic (Claude)",
        endpoint: "https://api.anthropic.com/v1",
        models: &[
            ModelPreset {
                id: "claude-sonnet-4",
                display_name: "Claude Sonnet 4",
                price_prompt: Some(20.0),
                price_completion: Some(100.0),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
            ModelPreset {
                id: "claude-opus-4",
                display_name: "Claude Opus 4",
                price_prompt: Some(100.0),
                price_completion: Some(500.0),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
            ModelPreset {
                id: "claude-haiku-4",
                display_name: "Claude Haiku 4",
                price_prompt: Some(7.0),
                price_completion: Some(35.0),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
            ModelPreset {
                id: "claude-3-7-sonnet",
                display_name: "Claude 3.7 Sonnet",
                price_prompt: Some(20.0),
                price_completion: Some(100.0),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "gemini",
        display_name: "Gemini (Google)",
        endpoint: "https://generativelanguage.googleapis.com/v1beta/openai",
        models: &[
            ModelPreset {
                id: "gemini-2.5-pro",
                display_name: "Gemini 2.5 Pro",
                price_prompt: Some(9.0),
                price_completion: Some(70.0),
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
            ModelPreset {
                id: "gemini-2.5-flash",
                display_name: "Gemini 2.5 Flash",
                price_prompt: Some(2.0),
                price_completion: Some(18.0),
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
            ModelPreset {
                id: "gemini-2.5-flash-lite",
                display_name: "Gemini 2.5 Flash-Lite",
                price_prompt: Some(0.7),
                price_completion: Some(2.8),
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
            ModelPreset {
                id: "gemini-2.0-flash",
                display_name: "Gemini 2.0 Flash",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "grok",
        display_name: "Grok (xAI)",
        endpoint: "https://api.x.ai/v1",
        models: &[
            ModelPreset {
                id: "grok-3",
                display_name: "Grok 3",
                price_prompt: Some(20.0),
                price_completion: Some(100.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "grok-3-mini",
                display_name: "Grok 3 mini",
                price_prompt: Some(2.0),
                price_completion: Some(7.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "grok-2",
                display_name: "Grok 2",
                price_prompt: Some(14.0),
                price_completion: Some(70.0),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "openrouter",
        display_name: "OpenRouter",
        endpoint: "https://openrouter.ai/api/v1",
        // 模型名带 `/`（`provider/model` 路由）；与 canonical `provider/model` 分隔天然兼容
        // （split_canonical 用 split_once('/')，只拆第一个 `/`，rest 即完整模型名）。
        models: &[
            ModelPreset {
                id: "anthropic/claude-3.7-sonnet",
                display_name: "Claude 3.7 Sonnet (routed)",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
            ModelPreset {
                id: "openai/gpt-4o",
                display_name: "GPT-4o (routed)",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "deepseek/deepseek-chat",
                display_name: "DeepSeek Chat (routed)",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "google/gemini-2.5-flash",
                display_name: "Gemini 2.5 Flash (routed)",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "ollama",
        display_name: "Ollama (本地)",
        endpoint: "http://localhost:11434/v1",
        // 本地免 key；模型需先 `ollama pull`。OpenAI 兼容端点无需 Authorization。
        models: &[
            ModelPreset {
                id: "llama3.2",
                display_name: "Llama 3.2",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen3:8b",
                display_name: "Qwen3 8B",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 32000,
                thinking: false,
            },
            ModelPreset {
                id: "deepseek-r1:7b",
                display_name: "DeepSeek R1 7B",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 32000,
                thinking: true,
            },
            ModelPreset {
                id: "phi4",
                display_name: "Phi-4",
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
        ],
    },
];

/// 按 id 查内置预设。
pub fn preset(id: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// 从内置预设构造一个 ProviderConfig（不含 api key——key 由用户填/环境变量）。
/// pricing 自动带上内置元数据；`models` 填全预设模型列表（目标：一个 key 多个 model）。
pub fn provider_from_preset(preset_id: &str, model_id: &str) -> ProviderConfig {
    let p = preset(preset_id).expect("内置预设存在");
    let m = p
        .models
        .iter()
        .find(|m| m.id == model_id)
        .expect("模型在预设中");
    let models = p
        .models
        .iter()
        .map(|pm| crate::config::ModelConfig {
            id: pm.id.to_string(),
            name: Some(pm.display_name.to_string()),
            price_prompt: pm.price_prompt,
            price_completion: pm.price_completion,
            price_prompt_cached: pm.price_prompt_cached,
            context_length: pm.context_length,
            thinking: pm.thinking,
        })
        .collect();
    ProviderConfig {
        name: p.id.to_string(),
        endpoint: p.endpoint.to_string(),
        api_key: None,
        api_key_env: None,
        model: m.id.to_string(),
        models,
        price_prompt: m.price_prompt,
        price_completion: m.price_completion,
        price_prompt_cached: m.price_prompt_cached,
        context_length: m.context_length,
        thinking: m.thinking,
    }
}

/// 构造一个自定义 OpenAI-compatible provider（pricing 未知 → Cost tracking unavailable）。
/// 单 model 创建；保存前 `ensure_models` 会把该 model 补进 models 列表。
pub fn custom_provider(name: &str, endpoint: &str, model: &str) -> ProviderConfig {
    let mut cfg = ProviderConfig {
        name: name.to_string(),
        endpoint: endpoint.to_string(),
        api_key: None,
        api_key_env: None,
        model: model.to_string(),
        models: Vec::new(),
        price_prompt: None,
        price_completion: None,
        price_prompt_cached: None,
        context_length: 8192,
        thinking: false,
    };
    cfg.ensure_models();
    cfg
}

/// 构造自定义 provider 的多模型配置（Setup 的 Custom 输入逗号分隔多个 model）。
/// 第一个 model 为当前活动模型；pricing 未知。
pub fn custom_provider_multi(name: &str, endpoint: &str, models: &[&str]) -> ProviderConfig {
    let models: Vec<crate::config::ModelConfig> = models
        .iter()
        .map(|m| crate::config::ModelConfig {
            id: m.to_string(),
            name: None,
            price_prompt: None,
            price_completion: None,
            price_prompt_cached: None,
            context_length: 8192,
            thinking: false,
        })
        .collect();
    let model = models.first().map(|m| m.id.clone()).unwrap_or_default();
    ProviderConfig {
        name: name.to_string(),
        endpoint: endpoint.to_string(),
        api_key: None,
        api_key_env: None,
        model,
        models,
        price_prompt: None,
        price_completion: None,
        price_prompt_cached: None,
        context_length: 8192,
        thinking: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_cover_all_target_providers() {
        let ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        for want in [
            "deepseek",
            "qwen",
            "glm",
            "kimi",
            "minimax",
            "doubao",
            "hunyuan",
            "ernie",
            "openai",
            "anthropic",
            "gemini",
            "grok",
            "openrouter",
            "ollama",
        ] {
            assert!(ids.contains(&want), "缺少预设 {want}");
        }
        assert_eq!(ids.len(), 14);
    }

    #[test]
    fn every_provider_has_multiple_models_and_endpoint() {
        for p in PRESETS {
            assert!(!p.endpoint.is_empty(), "{} 缺 endpoint", p.id);
            assert!(
                p.models.len() >= 2,
                "{} 应支持多个模型（实际 {}）",
                p.id,
                p.models.len()
            );
            for m in p.models {
                assert!(!m.id.is_empty() && !m.display_name.is_empty());
            }
            // 内置模型带定价（未知/本地模型允许 None；openrouter 聚合路由、ollama 本地免费豁免）
            assert!(
                p.id == "openrouter"
                    || p.id == "ollama"
                    || p.models.iter().any(|m| m.price_prompt.is_some()),
                "{} 应至少一个模型带定价",
                p.id
            );
        }
    }

    #[test]
    fn openrouter_model_ids_keep_slash() {
        // OpenRouter 模型名含 `/`，canonical `openrouter/anthropic/claude-3.7-sonnet`
        // 经 split_once('/') 拆为 provider=openrouter, model=anthropic/claude-3.7-sonnet
        let cfg = provider_from_preset("openrouter", "anthropic/claude-3.7-sonnet");
        assert_eq!(cfg.name, "openrouter");
        assert_eq!(cfg.model, "anthropic/claude-3.7-sonnet");
        assert_eq!(cfg.endpoint, "https://openrouter.ai/api/v1");
        let canonical = format!("{}/{}", cfg.name, cfg.model);
        let (p, m) = crate::config::split_canonical(&canonical).unwrap();
        assert_eq!(p, "openrouter");
        assert_eq!(m, "anthropic/claude-3.7-sonnet");
    }

    #[test]
    fn provider_from_preset_carries_pricing() {
        let cfg = provider_from_preset("deepseek", "deepseek-reasoner");
        assert_eq!(cfg.name, "deepseek");
        assert_eq!(cfg.model, "deepseek-reasoner");
        assert!(cfg.known_pricing(), "内置预设应 known_pricing");
        assert!(cfg.thinking);
        assert_eq!(cfg.endpoint, "https://api.deepseek.com/v1");
    }

    #[test]
    fn qwen_and_anthropic_presets_are_well_formed() {
        let q = preset("qwen").unwrap();
        assert_eq!(q.models.len(), 5);
        assert!(q.models.iter().any(|m| m.id == "qwen3-max"));
        let a = preset("anthropic").unwrap();
        assert!(a.models.iter().any(|m| m.id == "claude-sonnet-4"));
        assert_eq!(a.endpoint, "https://api.anthropic.com/v1");
        let g = preset("gemini").unwrap();
        assert!(g.endpoint.contains("generativelanguage.googleapis.com"));
        let o = preset("ollama").unwrap();
        assert!(o.endpoint.contains("localhost"));
    }

    #[test]
    fn unknown_preset_panics_in_test() {
        assert!(preset("nonexistent").is_none());
    }

    #[test]
    fn custom_provider_pricing_unknown_but_usable() {
        let c = custom_provider("my-llm", "http://localhost:8000/v1", "model-x");
        assert!(!c.known_pricing(), "自定义 provider 不应假设价格");
        assert_eq!(c.endpoint, "http://localhost:8000/v1");
        assert_eq!(c.model, "model-x");
    }
}
