//! Model Registry（文档 §七）：内置 OpenAI-compatible provider 预设 + 自定义 provider。
//!
//! 目标：普通用户不需要手填模型价格——内置预设带 pricing 元数据，
//! 未知/自定义模型 pricing optional（显示 Cost tracking unavailable 但可正常使用）。

use crate::config::ProviderConfig;

/// 内置 provider 预设（display name 用友好名，不暴露内部 id）。
pub struct ProviderPreset {
    /// 机器 id（写入 config 的 provider.name）
    pub id: &'static str,
    /// 用户可见名称
    pub display_name: &'static str,
    /// 可选模型
    pub models: &'static [ModelPreset],
}

/// 一个模型预设：内置模型带 pricing 元数据（可选）。
pub struct ModelPreset {
    pub id: &'static str,
    pub display_name: &'static str,
    /// 元 / 百万 token
    pub price_prompt: Option<f64>,
    pub price_completion: Option<f64>,
    /// 上下文缓存命中单价（可选；未配置按全价保守估算）
    pub price_prompt_cached: Option<f64>,
    pub context_length: u64,
    pub thinking: bool,
}

/// 内置 provider 预设列表。
pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "deepseek",
        display_name: "DeepSeek",
        models: &[
            ModelPreset {
                id: "deepseek-reasoner",
                display_name: "DeepSeek Reasoner",
                price_prompt: Some(4.0),
                price_completion: Some(16.0),
                price_prompt_cached: Some(1.0),
                context_length: 65536,
                thinking: true,
            },
            ModelPreset {
                id: "deepseek-chat",
                display_name: "DeepSeek Chat",
                price_prompt: Some(0.27),
                price_completion: Some(1.1),
                price_prompt_cached: Some(0.0675),
                context_length: 65536,
                thinking: false,
            },
        ],
    },
    ProviderPreset {
        id: "glm",
        display_name: "GLM (Zhipu)",
        models: &[ModelPreset {
            id: "glm-4-plus",
            display_name: "GLM-4-Plus",
            price_prompt: Some(4.0),
            price_completion: Some(4.0),
            price_prompt_cached: None,
            context_length: 128000,
            thinking: false,
        }],
    },
    ProviderPreset {
        id: "openai",
        display_name: "OpenAI",
        models: &[
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
                id: "gpt-4o",
                display_name: "GPT-4o",
                price_prompt: Some(11.0),
                price_completion: Some(44.0),
                price_prompt_cached: Some(2.75),
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
    let endpoint = match p.id {
        "deepseek" => "https://api.deepseek.com/v1",
        "glm" => "https://open.bigmodel.cn/api/paas/v4",
        "openai" => "https://api.openai.com/v1",
        _ => "https://api.openai.com/v1",
    };
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
        endpoint: endpoint.to_string(),
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
    fn presets_have_display_names_and_pricing() {
        assert_eq!(PRESETS.len(), 3);
        let ds = preset("deepseek").unwrap();
        assert_eq!(ds.display_name, "DeepSeek");
        assert_eq!(ds.models.len(), 2);
        // 内置模型带 pricing
        assert!(ds.models[0].price_prompt.is_some());
        assert!(ds.models[0].price_completion.is_some());
    }

    #[test]
    fn provider_from_preset_carries_pricing() {
        let cfg = provider_from_preset("deepseek", "deepseek-reasoner");
        assert_eq!(cfg.name, "deepseek");
        assert_eq!(cfg.model, "deepseek-reasoner");
        assert!(cfg.known_pricing(), "内置预设应 known_pricing");
        assert!(cfg.thinking);
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
