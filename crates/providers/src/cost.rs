//! 成本折算（R6 的计算核心；统计落库属后续里程碑）。

use agent_core::Usage;

use crate::config::ProviderConfig;

/// 按配置单价（元/百万 token）把一次调用的 usage 折算为费用（元）。
pub fn estimate_cost(cfg: &ProviderConfig, usage: &Usage) -> f64 {
    cfg.price_prompt * usage.prompt_tokens as f64 / 1_000_000.0
        + cfg.price_completion * usage.completion_tokens as f64 / 1_000_000.0
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
}
