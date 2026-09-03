//! config.toml 解析（provider 定义格式见本文件 ProviderConfig 字段）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use agent_core::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for ProviderConfig {
    /// 空占位 provider（无有效配置时启动用；Setup/Settings 引导填写）。
    fn default() -> Self {
        Self {
            name: "custom".into(),
            endpoint: "https://api.openai.com/v1".into(),
            api_key: None,
            api_key_env: None,
            model: "".into(),
            price_prompt: None,
            price_completion: None,
            price_prompt_cached: None,
            context_length: 8192,
            thinking: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_provider: "custom".into(),
            max_cost: default_max_cost(),
            providers: vec![ProviderConfig::default()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub endpoint: String,
    /// 明文 key（本地 config 文件允许；该文件已被 .gitignore 忽略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// 环境变量名引用——优先级低于明文 api_key，避免明文入库的推荐方式。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    pub model: String,
    /// 元 / 百万 token（可选：未知模型可运行，Cost tracking unavailable）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_prompt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_completion: Option<f64>,
    /// 元 / 百万 token（上下文缓存命中部分）；未配置 = 与 price_prompt 同价（保守）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_prompt_cached: Option<f64>,
    #[serde(default)]
    pub context_length: u64,
    #[serde(default)]
    pub thinking: bool,
}

impl ProviderConfig {
    /// 是否具备已知 pricing（未知模型显示 Cost tracking unavailable，但可正常使用）。
    pub fn known_pricing(&self) -> bool {
        self.price_prompt.is_some() && self.price_completion.is_some()
    }
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

    /// 序列化回 TOML（供 /model 写盘 runtime config）。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let toml = toml::to_string(self).map_err(|e| Error::Config(format!("序列化失败: {e}")))?;
        std::fs::write(path, toml)
            .map_err(|e| Error::Config(format!("写入 {}: {e}", path.display())))
    }

    /// 解析运行时配置路径：优先 `~/.studypilot/config.toml`（产品形态），
    /// 不存在则回退 cwd `config.toml`（开发者兼容）。确保目录存在。
    pub fn runtime_path() -> Result<std::path::PathBuf> {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let dir = std::path::PathBuf::from(home).join(".studypilot");
            let _ = std::fs::create_dir_all(&dir);
            return Ok(dir.join("config.toml"));
        }
        Ok(std::path::PathBuf::from("config.toml"))
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

    #[test]
    fn pricing_optional_parses_without_prices() {
        // 用户无需填写价格（文档 §十一）：未知模型也能配置/运行
        let cfg = r#"
default_provider = "custom"
[[providers]]
name = "custom"
endpoint = "http://localhost:8000/v1"
model = "my-model"
"#
        .parse::<Config>()
        .unwrap();
        let p = cfg.default_provider().unwrap();
        assert!(!p.known_pricing(), "未配价格 = Cost tracking unavailable");
    }

    #[test]
    fn known_pricing_detects_present_prices() {
        let cfg = SAMPLE.parse::<Config>().unwrap();
        assert!(cfg.provider("deepseek").unwrap().known_pricing());
    }

    #[test]
    fn save_roundtrip_preserves_config() {
        let cfg = SAMPLE.parse::<Config>().unwrap();
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("sp-cfg-{}-{ns}.toml", std::process::id()));
        cfg.save(&path).unwrap();
        let reloaded = Config::load(&path).unwrap();
        assert_eq!(reloaded.default_provider, cfg.default_provider);
        assert_eq!(reloaded.providers.len(), cfg.providers.len());
        let _ = std::fs::remove_file(&path);
    }

    // ── 文档 §十二：incomplete provider ≠ config invalid ──

    #[test]
    fn missing_config_does_not_crash() {
        // runtime_path 目录不存在 → load 返回 Err（由 main 兜底为 default），不 panic
        let none_path = std::path::PathBuf::from("/tmp/definitely-not-exists-sp/config.toml");
        assert!(Config::load(&none_path).is_err());
    }

    #[test]
    fn incomplete_custom_provider_is_valid_config() {
        // custom 存在但无 key：Config 仍然 valid（validate 只查 default_provider 存在）
        let cfg = r#"
default_provider = "custom"
[[providers]]
name = "custom"
endpoint = "http://localhost:8000/v1"
model = "my-model"
"#
        .parse::<Config>()
        .unwrap();
        let p = cfg.default_provider().unwrap();
        assert!(p.resolve_api_key().is_err(), "缺 key 但 resolve 报错");
        assert!(!p.known_pricing());
        assert!(!p.api_key.is_some());
        assert!(!p.api_key_env.is_some());
    }

    #[test]
    fn malformed_toml_still_returns_config_error() {
        let bad = "default_provider = ".parse::<Config>().unwrap_err();
        assert!(bad.to_string().contains("解析配置失败"), "{bad}");
    }

    #[test]
    fn configured_provider_starts_normally() {
        // 明文 key 存在 → resolve 成功
        let cfg = SAMPLE.parse::<Config>().unwrap();
        assert_eq!(
            cfg.provider("plain").unwrap().resolve_api_key().unwrap(),
            "sk-plain"
        );
    }
}
