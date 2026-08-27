//! config.toml 解析（R3 定稿格式，见 规划.md 第四章示例）。

use std::path::Path;

use serde::Deserialize;

use agent_core::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub default_provider: String,
    #[serde(default = "default_max_cost")]
    pub max_cost: f64,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

fn default_max_cost() -> f64 {
    5.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub endpoint: String,
    /// 明文 key（本地 config 文件允许；该文件已被 .gitignore 忽略）。
    pub api_key: Option<String>,
    /// 环境变量名引用——优先级低于明文 api_key，避免明文入库的推荐方式。
    pub api_key_env: Option<String>,
    pub model: String,
    /// 元 / 百万 token
    pub price_prompt: f64,
    pub price_completion: f64,
    pub context_length: u64,
    #[serde(default)]
    pub thinking: bool,
}

impl ProviderConfig {
    /// 解析真实 API key：非空明文 `api_key` 优先，否则读 `api_key_env` 指向的环境变量。
    pub fn resolve_api_key(&self) -> Result<String> {
        if let Some(key) = self.api_key.as_deref().filter(|k| !k.is_empty()) {
            return Ok(key.to_owned());
        }
        match &self.api_key_env {
            Some(var) => std::env::var(var)
                .ok()
                .filter(|v| !v.is_empty())
                .map_or_else(
                    || Err(Error::Config(format!("环境变量 {var} 未设置或为空"))),
                    Ok,
                ),
            None => Err(Error::Config(format!(
                "provider `{}` 既无 api_key 也未配置 api_key_env",
                self.name
            ))),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("读取 {}: {e}", path.display())))?;
        text.parse()
    }

    fn validate(&self) -> Result<()> {
        if self.default_provider.is_empty() {
            return Err(Error::Config("default_provider 不能为空".into()));
        }
        match self.provider(&self.default_provider) {
            Ok(_) => Ok(()),
            Err(_) => Err(Error::Config(format!(
                "default_provider `{}` 不在 providers 列表中",
                self.default_provider
            ))),
        }
    }

    pub fn provider(&self, name: &str) -> Result<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name).map_or_else(
            || Err(Error::Config(format!("未找到 provider `{name}`"))),
            Ok,
        )
    }

    /// 当前默认 provider 的克隆（客户端持有所有权）。
    pub fn default_provider(&self) -> Result<&ProviderConfig> {
        self.provider(&self.default_provider)
    }
}

impl std::str::FromStr for Config {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let cfg: Config =
            toml::from_str(text).map_err(|e| Error::Config(format!("解析配置失败: {e}")))?;
        cfg.validate()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
default_provider = "deepseek"
max_cost = 5.0

[[providers]]
name = "deepseek"
endpoint = "https://api.deepseek.com/v1"
api_key_env = "TEST_M1_KEY"
model = "deepseek-reasoner"
price_prompt = 4.0
price_completion = 16.0
context_length = 65536
thinking = true

[[providers]]
name = "plain"
endpoint = "https://example.com/v1"
api_key = "sk-plain"
api_key_env = "SHOULD_NOT_BE_READ"
model = "gpt-4o-mini"
price_prompt = 1.0
price_completion = 4.0
context_length = 128000
thinking = false
"#;

    #[test]
    fn parse_full_config() {
        let cfg = SAMPLE.parse::<Config>().unwrap();
        assert_eq!(cfg.default_provider, "deepseek");
        assert_eq!(cfg.max_cost, 5.0);
        assert_eq!(cfg.providers.len(), 2);
        let ds = cfg.default_provider().unwrap();
        assert!(ds.thinking);
        assert_eq!(ds.context_length, 65536);
    }

    #[test]
    fn default_provider_missing_in_list_fails() {
        let err = "default_provider = \"nope\"".parse::<Config>().unwrap_err();
        assert!(err.to_string().contains("不在 providers 列表中"), "{err}");
    }

    #[test]
    fn api_key_from_plaintext_wins_over_env() {
        // 明文存在时绝不读环境变量（即使指向的变量不存在也不报错）
        let cfg = SAMPLE.parse::<Config>().unwrap();
        let plain = cfg.provider("plain").unwrap();
        assert_eq!(plain.resolve_api_key().unwrap(), "sk-plain");
    }

    #[test]
    fn api_key_from_env() {
        // edition 2024 中 set_var/remove_var 为 unsafe；用互斥锁串行化，
        // 避免与其它读环境变量的测试并发竞争
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();

        let cfg = SAMPLE.parse::<Config>().unwrap();
        let ds = cfg.provider("deepseek").unwrap();
        unsafe {
            std::env::set_var("TEST_M1_KEY", "sk-from-env");
        }
        let got = ds.resolve_api_key().unwrap();
        unsafe {
            std::env::remove_var("TEST_M1_KEY");
        }
        assert_eq!(got, "sk-from-env");

        let err = ds.resolve_api_key().unwrap_err();
        assert!(err.to_string().contains("未设置或为空"), "{err}");
    }

    #[test]
    fn empty_api_key_treated_as_absent() {
        // 空字符串 key 视同未配置：回退 env 分支并报错
        let cfg = SAMPLE
            .replace("api_key = \"sk-plain\"", "api_key = \"\"")
            .parse::<Config>()
            .unwrap();
        let plain = cfg.provider("plain").unwrap();
        assert!(plain.resolve_api_key().is_err());
    }
}
