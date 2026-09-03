//! 成本折算（R6 的计算核心；统计落库属后续里程碑）。

use agent_core::Usage;

use crate::config::ProviderConfig;

/// 按配置单价（元/百万 token）把一次调用的 usage 折算为费用（元）。
pub fn estimate_cost(cfg: &ProviderConfig, usage: &Usage) -> f64 {
    // 未知 pricing（Option=None）→ Cost tracking unavailable，不计费
    let (Some(price_prompt), Some(price_completion)) = (cfg.price_prompt, cfg.price_completion)
    else {
        return 0.0;
    };
    // 缓存命中部分按缓存价（deepseek-reasoner 命中价 ≈ 全价 1/4），
    // 未配置缓存单价时按全价（与旧行为一致，保守不多算）
    let hit = usage.cached_tokens.min(usage.prompt_tokens) as f64;
    let miss = usage.prompt_tokens as f64 - hit;
    let cached_price = cfg.price_prompt_cached.unwrap_or(price_prompt);
    cached_price * hit / 1_000_000.0
        + price_prompt * miss / 1_000_000.0
        + price_completion * usage.completion_tokens as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc() -> ProviderConfig {
        // 与 config.toml 中 deepseek 单价一致
        toml::from_str(
            r#"
name = "deepseek"
endpoint = "https://api.deepseek.com/v1"
model = "deepseek-reasoner"
price_prompt = 4.0
price_completion = 16.0
context_length = 65536
thinking = true
"#,
        )
        .unwrap()
    }

    #[test]
    fn cost_matches_m0_real_usage() {
        // M0 实测：prompt=98 completion=188 → 4*98/1e6 + 16*188/1e6
        let cost = estimate_cost(
            &pc(),
            &Usage {
                cached_tokens: 0,
                prompt_tokens: 98,
                completion_tokens: 188,
            },
        );
        assert!((cost - (4.0 * 98.0 + 16.0 * 188.0) / 1e6).abs() < 1e-12);
    }

    #[test]
    fn zero_usage_is_free() {
        assert_eq!(estimate_cost(&pc(), &Usage::default()), 0.0);
    }

    #[test]
    fn cached_tokens_priced_at_cached_rate() {
        // 100万 prompt（50万命中）+ 10万 completion：
        // 命中 1.0/M × 0.5 + 全价 4.0/M × 0.5 + 16.0/M × 0.1 = 0.5 + 2.0 + 1.6 = 4.1
        let mut cfg = pc();
        cfg.price_prompt_cached = Some(1.0);
        let cost = estimate_cost(
            &cfg,
            &Usage {
                prompt_tokens: 1_000_000,
                completion_tokens: 100_000,
                cached_tokens: 500_000,
            },
        );
        assert!((cost - 4.1).abs() < 1e-9, "cost = {cost}");
    }
}
