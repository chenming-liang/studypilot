//! M1 验收：`cargo run -p providers --example once -- "你好"`
//! 打印回复、prompt/completion tokens 与按配置价格折算的 cost。

use agent_core::{Message, Provider};
use agent_providers::{Config, OpenAiClient, estimate_cost};
use anyhow::{Context, bail};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let prompt = args.next().unwrap_or_else(|| "你好".into());
    let config_path = args.next().unwrap_or_else(|| "config.toml".into());

    let cfg = Config::load(&config_path).context("加载配置失败")?;
    if !cfg.providers.is_empty() && cfg.max_cost <= 0.0 {
        bail!("max_cost 必须为正数");
    }
    let pc = cfg
        .default_provider()
        .context("定位默认 provider 失败")?
        .clone();
    println!(
        "provider: {} | model: {} | endpoint: {}",
        pc.name, pc.model, pc.endpoint
    );

    let client = OpenAiClient::new(pc.clone())?;
    let resp = client.chat(&[Message::user(&prompt)], &[]).await?;

    if let Some(reasoning) = &resp.reasoning {
        println!("--- 思考 ({} 字符) ---", reasoning.chars().count());
        println!("{}", preview(reasoning, 300));
    }
    println!("--- 回复 ---");
    println!("{}", resp.content);
    println!(
        "--- tokens: prompt={} completion={} | cost: ${:.6} ---",
        resp.usage.prompt_tokens,
        resp.usage.completion_tokens,
        estimate_cost(&pc, &resp.usage)
    );
    Ok(())
}

fn preview(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let head: String = s.chars().take(max_chars).collect();
        format!("{head}…")
    }
}
