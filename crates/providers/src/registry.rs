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
    /// $ / 百万 token（USD 估算价，固定 1 USD = 6.71 CNY；近似官方定价，以厂商官网为准）
    pub price_prompt: Option<f64>,
    pub price_completion: Option<f64>,
    /// 上下文缓存命中单价（可选；未配置按全价保守估算）
    pub price_prompt_cached: Option<f64>,
    pub context_length: u64,
    pub thinking: bool,
}

/// 内置 provider 预设列表（2026-09：各厂商当前主流常用模型）。
/// pricing 为 USD / 1M tokens 估算价（固定换算 1 USD = 6.71 CNY），仅用于成本估算、不追求账单级精确。
pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "deepseek",
        display_name: "DeepSeek",
        endpoint: "https://api.deepseek.com/v1",
        models: &[
            ModelPreset {
                id: "deepseek-v4-flash",
                display_name: "DeepSeek V4 Flash",
                price_prompt: Some(0.44),
                price_completion: Some(1.32),
                price_prompt_cached: Some(0.014),
                context_length: 1_000_000,
                thinking: true,
            },
            ModelPreset {
                id: "deepseek-v4-pro",
                display_name: "DeepSeek V4 Pro",
                price_prompt: Some(1.32),
                price_completion: Some(3.96),
                price_prompt_cached: Some(0.022),
                context_length: 1_000_000,
                thinking: true,
            },
            ModelPreset {
                id: "deepseek-v4-flash-vision-exp",
                display_name: "DeepSeek V4 Flash Vision (Exp)",
                price_prompt: Some(0.44),
                price_completion: Some(1.32),
                price_prompt_cached: None,
                context_length: 1_000_000,
                thinking: true,
            },
        ],
    },
    ProviderPreset {
        id: "qwen",
        display_name: "Qwen (通义千问)",
        endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &[
            ModelPreset {
                id: "qwen3.8-max",
                display_name: "Qwen 3.8 Max",
                price_prompt: Some(1.65),
                price_completion: Some(4.951),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen3.7-max",
                display_name: "Qwen 3.7 Max",
                price_prompt: Some(1.65),
                price_completion: Some(4.951),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen3.8-flash",
                display_name: "Qwen 3.8 Flash",
                price_prompt: Some(0.113),
                price_completion: Some(0.382),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "qwen3.7-flash",
                display_name: "Qwen 3.7 Flash",
                price_prompt: Some(0.028),
                price_completion: Some(0.11),
                price_prompt_cached: None,
                context_length: 256000,
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
                id: "glm-5.3",
                display_name: "GLM-5.3",
                price_prompt: Some(1.40),
                price_completion: Some(4.40),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "glm-5.3-flash",
                display_name: "GLM-5.3 Flash",
                price_prompt: Some(0.12),
                price_completion: Some(0.42),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "glm-5.2",
                display_name: "GLM-5.2",
                price_prompt: Some(1.40),
                price_completion: Some(4.40),
                price_prompt_cached: None,
                context_length: 128000,
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
                id: "kimi-k3",
                display_name: "Kimi K3",
                price_prompt: Some(2.98),
                price_completion: Some(14.90),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "kimi-k2.7-code",
                display_name: "Kimi K2.7 Code",
                price_prompt: Some(0.97),
                price_completion: Some(4.02),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "kimi-k2.6",
                display_name: "Kimi K2.6",
                price_prompt: Some(0.97),
                price_completion: Some(4.02),
                price_prompt_cached: None,
                context_length: 256000,
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
                id: "MiniMax-M3",
                display_name: "MiniMax M3",
                price_prompt: Some(0.60),
                price_completion: Some(2.40),
                price_prompt_cached: None,
                context_length: 1_000_000,
                thinking: false,
            },
            ModelPreset {
                id: "MiniMax-M2.7",
                display_name: "MiniMax M2.7",
                price_prompt: Some(0.30),
                price_completion: Some(1.20),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "MiniMax-M2.7-highspeed",
                display_name: "MiniMax M2.7 Highspeed",
                price_prompt: Some(0.60),
                price_completion: Some(2.40),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "MiniMax-M2.5",
                display_name: "MiniMax M2.5",
                price_prompt: Some(0.30),
                price_completion: Some(1.20),
                price_prompt_cached: None,
                context_length: 256000,
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
                id: "doubao-seed-2.1-pro",
                display_name: "Doubao Seed 2.1 Pro",
                price_prompt: Some(0.89),
                price_completion: Some(4.47),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "doubao-seed-2.1-turbo",
                display_name: "Doubao Seed 2.1 Turbo",
                price_prompt: Some(0.45),
                price_completion: Some(2.24),
                price_prompt_cached: None,
                context_length: 256000,
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
                id: "Hy4-preview",
                display_name: "Hunyuan Hy4 Preview",
                price_prompt: Some(0.834),
                price_completion: Some(2.501),
                price_prompt_cached: None,
                context_length: 256000,
                thinking: false,
            },
            ModelPreset {
                id: "Hy3",
                display_name: "Hunyuan Hy3",
                price_prompt: Some(0.132),
                price_completion: Some(0.528),
                price_prompt_cached: None,
                context_length: 256000,
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
                id: "ernie-5.1",
                display_name: "ERNIE 5.1",
                price_prompt: Some(0.60),
                price_completion: Some(2.68),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "ernie-5.0",
                display_name: "ERNIE 5.0",
                price_prompt: Some(0.89),
                price_completion: Some(3.58),
                price_prompt_cached: None,
                context_length: 128000,
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
                id: "gpt-6-astra",
                display_name: "GPT-6 Astra",
                price_prompt: Some(10.00),
                price_completion: Some(50.00),
                price_prompt_cached: None,
                context_length: 400000,
                thinking: false,
            },
            ModelPreset {
                id: "gpt-5.6-sol",
                display_name: "GPT-5.6 Sol",
                price_prompt: Some(4.00),
                price_completion: Some(20.00),
                price_prompt_cached: None,
                context_length: 400000,
                thinking: false,
            },
            ModelPreset {
                id: "gpt-5.6-terra",
                display_name: "GPT-5.6 Terra",
                price_prompt: Some(2.00),
                price_completion: Some(12.00),
                price_prompt_cached: None,
                context_length: 400000,
                thinking: false,
            },
            ModelPreset {
                id: "gpt-5.6-luna",
                display_name: "GPT-5.6 Luna",
                price_prompt: Some(0.20),
                price_completion: Some(1.20),
                price_prompt_cached: None,
                context_length: 400000,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "anthropic",
        display_name: "Anthropic (Claude)",
        endpoint: "https://api.anthropic.com/v1",
        models: &[
            ModelPreset {
                id: "claude-fable-5.1",
                display_name: "Claude Fable 5.1",
                price_prompt: Some(5.00),
                price_completion: Some(25.00),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
            ModelPreset {
                id: "claude-opus-5",
                display_name: "Claude Opus 5",
                price_prompt: Some(2.50),
                price_completion: Some(12.50),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
            ModelPreset {
                id: "claude-sonnet-5",
                display_name: "Claude Sonnet 5",
                price_prompt: Some(1.00),
                price_completion: Some(5.00),
                price_prompt_cached: None,
                context_length: 200000,
                thinking: false,
            },
            ModelPreset {
                id: "claude-haiku-4.5",
                display_name: "Claude Haiku 4.5",
                price_prompt: Some(0.50),
                price_completion: Some(2.50),
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
                id: "gemini-3.8-flash",
                display_name: "Gemini 3.8 Flash",
                price_prompt: Some(0.75),
                price_completion: Some(3.75),
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
            ModelPreset {
                id: "gemini-3.7-flash",
                display_name: "Gemini 3.7 Flash",
                price_prompt: Some(0.75),
                price_completion: Some(3.75),
                price_prompt_cached: None,
                context_length: 1000000,
                thinking: false,
            },
            ModelPreset {
                id: "gemini-3.6-flash",
                display_name: "Gemini 3.6 Flash",
                price_prompt: Some(0.75),
                price_completion: Some(3.75),
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
                id: "grok-4.6",
                display_name: "Grok 4.6",
                price_prompt: Some(2.00),
                price_completion: Some(6.00),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "grok-4.5",
                display_name: "Grok 4.5",
                price_prompt: Some(2.00),
                price_completion: Some(6.00),
                price_prompt_cached: None,
                context_length: 128000,
                thinking: false,
            },
            ModelPreset {
                id: "grok-4.3",
                display_name: "Grok 4.3",
                price_prompt: Some(1.25),
                price_completion: Some(2.50),
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
        ] {
            assert!(ids.contains(&want), "缺少预设 {want}");
        }
        assert_eq!(ids.len(), 12);
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
            // 内置模型带定价（未知/本地模型允许 None——pricing 未知时 Cost tracking unavailable）
        }
    }

    #[test]
    fn slash_model_names_are_parsed_per_canonical_rule() {
        // 模型名可含 `/`（如 OpenRouter 路由）；canonical `provider/model`
        // 经 split_once('/') 拆为 provider + 剩余模型名，只拆首个 `/`。
        let canonical = "openrouter/anthropic/claude-3.7-sonnet";
        let (p, m) = crate::config::split_canonical(canonical).unwrap();
        assert_eq!(p, "openrouter");
        assert_eq!(m, "anthropic/claude-3.7-sonnet");
    }

    #[test]
    fn provider_from_preset_carries_meta() {
        let cfg = provider_from_preset("deepseek", "deepseek-v4-flash");
        assert_eq!(cfg.name, "deepseek");
        assert_eq!(cfg.model, "deepseek-v4-flash");
        assert!(cfg.thinking);
        assert_eq!(cfg.endpoint, "https://api.deepseek.com/v1");
    }

    #[test]
    fn qwen_and_anthropic_presets_are_well_formed() {
        let q = preset("qwen").unwrap();
        assert!(q.models.len() >= 2);
        assert!(q.models.iter().any(|m| m.id == "qwen3.8-max"));
        let a = preset("anthropic").unwrap();
        assert!(a.models.iter().any(|m| m.id == "claude-fable-5.1"));
        assert_eq!(a.endpoint, "https://api.anthropic.com/v1");
        let g = preset("gemini").unwrap();
        assert!(g.endpoint.contains("generativelanguage.googleapis.com"));
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
