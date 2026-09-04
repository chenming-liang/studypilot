//! First-run AI Setup Wizard（文档：普通用户无需理解 Ctrl+K / /test / config.toml）。
//!
//! 状态机：Provider → Model → Credentials（API key）→ Test → Done。
//! 覆盖层模式（类似 palette），不污染 Session；成功才保存 runtime config。
//!
//! 复用：`providers::registry`（preset/custom）、`Config::save`、canonical 连接测试。

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::Arc;

/// Setup 步骤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupStep {
    Provider,
    Model,
    /// Custom provider 输入（name/base_url/model 三步，普通文本非遮罩）
    CustomName,
    CustomBaseUrl,
    CustomModel,
    Credentials,
    Test,
    Done,
}

/// Setup 状态（pending 配置，Test 成功才 Apply）。
#[derive(Debug)]
pub(crate) struct SetupState {
    pub(crate) step: SetupStep,
    /// 用户当前光标（Provider/Model 选择列表）
    pub(crate) cursor: usize,
    /// 选中的 provider（registry preset id 或 "custom"）
    pub(crate) provider: Option<String>,
    /// 选中的模型（多选：一次配置多个 models，文档 §9）
    pub(crate) selected_models: Vec<String>,
    /// 当前列表光标处模型（保留旧语义，切换/Test 用首个）
    pub(crate) model: Option<String>,
    /// 自定义 provider 输入（name/base_url/model）
    pub(crate) custom_name: String,
    pub(crate) custom_base_url: String,
    pub(crate) custom_model: String,
    /// Custom 输入缓冲（普通文本；逐字段确认后写入对应字段）
    pub(crate) custom_buf: String,
    /// API key（遮罩显示）
    pub(crate) api_key: String,
    /// 测试进行中 / 最近测试结果文本 / 是否已通过
    pub(crate) testing: bool,
    pub(crate) test_result: Option<String>,
    pub(crate) test_passed: bool,
    /// 输入缓冲（密钥输入时不落主 input）
    pub(crate) secret_buf: String,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            step: SetupStep::Provider,
            cursor: 0,
            provider: None,
            model: None,
            selected_models: Vec::new(),
            custom_name: String::new(),
            custom_base_url: String::new(),
            custom_model: String::new(),
            custom_buf: String::new(),
            api_key: String::new(),
            testing: false,
            test_result: None,
            test_passed: false,
            secret_buf: String::new(),
        }
    }
}

/// Custom 模型输入按逗号拆分并 trim 空白（`A, B,C` → ["A","B","C"]；空项丢弃）。
pub(crate) fn split_custom_models(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl SetupState {
    /// Custom provider 的用户可见名称（内部 id "custom" 不作为显示名）。
    pub(crate) fn custom_display_name(&self) -> String {
        if self.custom_name.trim().is_empty() {
            "custom".to_string()
        } else {
            self.custom_name.trim().to_string()
        }
    }

    /// Custom provider 的模型列表（逗号拆分后 trim；空则返回空列表）。
    pub(crate) fn custom_model_list(&self) -> Vec<String> {
        split_custom_models(&self.custom_model)
    }

    /// Provider 列表（registry 展示名 + Custom）。
    pub(crate) fn provider_options(&self) -> Vec<(&'static str, &'static str)> {
        let mut v: Vec<_> = agent_providers::PRESETS
            .iter()
            .map(|p| (p.id, p.display_name))
            .collect();
        v.push(("custom", "Custom OpenAI-compatible"));
        v
    }

    /// 当前 provider 的模型列表。
    pub(crate) fn model_options(&self) -> Vec<String> {
        match self.provider.as_deref() {
            Some("custom") => Vec::new(),
            Some(id) => agent_providers::preset(id)
                .map(|p| p.models.iter().map(|m| m.id.to_string()).collect())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

impl App {
    /// 启动 Setup（从 Home 的 Set up AI / palette / 首次自动）。
    pub(crate) fn start_setup(&mut self) {
        self.setup = Some(SetupState::default());
    }

    /// Setup 进入下一步。
    pub(crate) fn setup_next(&mut self) {
        let Some(s) = &mut self.setup else { return };
        s.step = match s.step {
            SetupStep::Provider => SetupStep::Model,
            SetupStep::Model => SetupStep::Credentials,
            SetupStep::CustomName => SetupStep::CustomBaseUrl,
            SetupStep::CustomBaseUrl => SetupStep::CustomModel,
            SetupStep::CustomModel => SetupStep::Credentials,
            SetupStep::Credentials => {
                // 新 key 已提交：进入 Test，强制重测
                s.test_passed = false;
                SetupStep::Test
            }
            SetupStep::Test => SetupStep::Done,
            SetupStep::Done => SetupStep::Done,
        };
    }

    /// Setup 返回上一步（Provider 再 Esc = 关闭 Setup 回 Home）。
    pub(crate) fn setup_back(&mut self) {
        let Some(s) = &mut self.setup else { return };
        s.step = match s.step {
            SetupStep::Provider => {
                self.setup = None;
                return;
            }
            SetupStep::Model => SetupStep::Provider,
            SetupStep::CustomName => SetupStep::Provider,
            SetupStep::CustomBaseUrl => SetupStep::CustomName,
            SetupStep::CustomModel => SetupStep::CustomBaseUrl,
            SetupStep::Credentials => {
                // 回到 Model/CustomModel：清空输入缓冲，避免带回旧 key
                s.secret_buf.clear();
                if s.provider.as_deref() == Some("custom") {
                    SetupStep::CustomModel
                } else {
                    SetupStep::Model
                }
            }
            SetupStep::Test => {
                // 回 Credentials：保留旧 key（渲染提示），新输入直接替换；标记重测
                s.secret_buf.clear();
                s.test_passed = false;
                SetupStep::Credentials
            }
            SetupStep::Done => SetupStep::Test,
        };
    }

    /// Setup 按键路由（键盘第一，文档 §二十/§二十一）。
    /// 返回 true = 已消费。
    pub(crate) fn handle_setup_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        let Some(step) = self.setup.as_ref().map(|s| s.step) else {
            return false;
        };
        match step {
            SetupStep::Provider => {
                // ↑↓ 选择 / Enter 确认 / Esc 返回
                match key.code {
                    KeyCode::Up => {
                        if let Some(s) = &mut self.setup {
                            s.cursor = s.cursor.saturating_sub(1);
                        }
                        true
                    }
                    KeyCode::Down => {
                        {
                            let n = self.setup_options().len();
                            if let Some(s) = &mut self.setup
                                && n > 0
                            {
                                s.cursor = (s.cursor + 1).min(n - 1);
                            }
                        }
                        true
                    }
                    KeyCode::Enter => {
                        self.setup_select();
                        true
                    }
                    KeyCode::Esc => {
                        self.setup_back();
                        true
                    }
                    _ => false,
                }
            }
            SetupStep::Model => {
                // ↑↓ 移动 / 空格 多选 / Enter 确认 / Esc 返回（一次可加多个 models）
                match key.code {
                    KeyCode::Up => {
                        if let Some(s) = &mut self.setup {
                            s.cursor = s.cursor.saturating_sub(1);
                        }
                        true
                    }
                    KeyCode::Down => {
                        {
                            let n = self.setup_options().len();
                            if let Some(s) = &mut self.setup
                                && n > 0
                            {
                                s.cursor = (s.cursor + 1).min(n - 1);
                            }
                        }
                        true
                    }
                    KeyCode::Char(' ') => {
                        self.setup_toggle_model();
                        true
                    }
                    KeyCode::Enter => {
                        self.setup_select();
                        true
                    }
                    KeyCode::Esc => {
                        self.setup_back();
                        true
                    }
                    _ => false,
                }
            }
            SetupStep::CustomName | SetupStep::CustomBaseUrl | SetupStep::CustomModel => {
                // 普通文本输入（name/base_url/model；Enter 确认写入字段并进下一步）
                match key.code {
                    KeyCode::Esc => {
                        self.setup_back();
                        true
                    }
                    KeyCode::Backspace => {
                        if let Some(s) = &mut self.setup {
                            s.custom_buf.pop();
                        }
                        true
                    }
                    KeyCode::Delete => {
                        if let Some(s) = &mut self.setup {
                            s.custom_buf.clear();
                        }
                        true
                    }
                    KeyCode::Enter => {
                        let (value, next) = {
                            let s = self.setup.as_mut().unwrap();
                            let value = std::mem::take(&mut s.custom_buf);
                            let next = match s.step {
                                SetupStep::CustomName => {
                                    s.custom_name = value.clone();
                                    SetupStep::CustomBaseUrl
                                }
                                SetupStep::CustomBaseUrl => {
                                    s.custom_base_url = value.clone();
                                    SetupStep::CustomModel
                                }
                                SetupStep::CustomModel => {
                                    s.custom_model = value.clone();
                                    SetupStep::Credentials
                                }
                                _ => unreachable!(),
                            };
                            (value, next)
                        };
                        let _ = value;
                        self.setup.as_mut().unwrap().step = next;
                        true
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(s) = &mut self.setup {
                            s.custom_buf.push(c);
                        }
                        true
                    }
                    _ => false,
                }
            }
            SetupStep::Credentials => {
                // secret input：API key（不明文显示，文档 §七/§二十二）
                match key.code {
                    KeyCode::Esc => {
                        self.setup_back();
                        true
                    }
                    KeyCode::Backspace => {
                        if let Some(s) = &mut self.setup {
                            s.secret_buf.pop();
                        }
                        true
                    }
                    KeyCode::Delete => {
                        if let Some(s) = &mut self.setup {
                            s.secret_buf.clear();
                        }
                        true
                    }
                    KeyCode::Enter => {
                        if let Some(s) = &mut self.setup {
                            s.api_key = std::mem::take(&mut s.secret_buf);
                        }
                        self.setup_next();
                        true
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(s) = &mut self.setup {
                            s.secret_buf.push(c);
                        }
                        true
                    }
                    _ => false,
                }
            }
            SetupStep::Test => {
                // 已通过 → Enter 前进到 Done；否则重跑测试（文档：失败保留 pending 可重试）
                match key.code {
                    KeyCode::Enter => {
                        let passed = self.setup.as_ref().map(|s| s.test_passed).unwrap_or(false);
                        if passed {
                            self.setup_next();
                        } else {
                            self.setup_test();
                        }
                        true
                    }
                    KeyCode::Esc => {
                        self.setup_back();
                        true
                    }
                    _ => false,
                }
            }
            SetupStep::Done => {
                match key.code {
                    KeyCode::Enter => {
                        self.setup = None; // 回 Home
                        true
                    }
                    KeyCode::Esc => {
                        self.setup_back();
                        true
                    }
                    _ => false,
                }
            }
        }
    }

    /// 当前 Setup 步骤的可用选项（供渲染/按键共用）。
    pub(crate) fn setup_options(&self) -> Vec<String> {
        let Some(s) = &self.setup else {
            return Vec::new();
        };
        match s.step {
            SetupStep::Provider => s
                .provider_options()
                .into_iter()
                .map(|(_, name)| name.to_string())
                .collect(),
            SetupStep::Model => {
                let models = s.model_options();
                if models.is_empty() {
                    vec!["Custom model (manual)".to_string()]
                } else {
                    models
                }
            }
            _ => Vec::new(),
        }
    }

    /// 空格：切换当前光标模型的选中状态（Model 多选）。
    pub(crate) fn setup_toggle_model(&mut self) {
        let Some(s) = &mut self.setup else { return };
        if s.step != SetupStep::Model {
            return;
        }
        let models = s.model_options();
        let Some(m) = models.get(s.cursor).cloned() else {
            return;
        };
        match s.selected_models.iter().position(|x| *x == m) {
            Some(i) => {
                s.selected_models.remove(i);
            }
            None => s.selected_models.push(m),
        }
    }

    /// Setup 选择当前光标项（Provider/Model 列表）。
    pub(crate) fn setup_select(&mut self) {
        let Some(s) = &mut self.setup else { return };
        match s.step {
            SetupStep::Provider => {
                let opts = s.provider_options();
                let id = opts.get(s.cursor).map(|(id, _)| id.to_string());
                if let Some(id) = id {
                    s.provider = Some(id.clone());
                    s.selected_models.clear();
                    if id == "custom" {
                        // Custom：进入 name/base_url/model 文本输入（文档：一等能力）
                        s.custom_buf.clear();
                        s.step = SetupStep::CustomName;
                    } else {
                        s.model = s.model_options().first().cloned();
                        s.step = SetupStep::Model;
                    }
                }
            }
            SetupStep::Model => {
                let models = s.model_options();
                if !models.is_empty() && s.selected_models.is_empty() {
                    // 没主动空格选：默认选中光标处模型（单模型快捷流不变）
                    s.selected_models.push(models[s.cursor].clone());
                }
                s.model = s.selected_models.first().cloned();
                s.step = SetupStep::Credentials;
            }
            _ => {}
        }
    }

    /// Setup 连接测试（复用 canonical `run_connection_test`）。
    pub(crate) fn setup_test(&mut self) {
        if self.setup.is_none() {
            return;
        }
        let cfg = self.build_pending_config();
        if let Some(s) = &mut self.setup {
            s.testing = true;
            s.test_result = None;
            s.test_passed = false;
        }
        self.run_connection_test(cfg, crate::app::AppEvent::SetupTestDone);
    }

    /// 构建待保存的 ProviderConfig（pending，不立即 Apply）。
    fn build_pending_config(&self) -> agent_providers::ProviderConfig {
        let Some(s) = &self.setup else {
            return self.provider_cfg.clone();
        };
        let selected_models = s.selected_models.clone();
        match s.provider.as_deref() {
            Some("custom") => {
                let name = s.custom_display_name();
                let endpoint = if s.custom_base_url.trim().is_empty() {
                    "https://api.openai.com/v1".to_string()
                } else {
                    s.custom_base_url.trim().to_string()
                };
                let models = s.custom_model_list();
                let models_refs: Vec<&str> = models.iter().map(String::as_str).collect();
                // 逗号分隔的多个 model → 多个独立 ModelConfig（文档 §二：一个 key 多个 model）
                let mut cfg = if models_refs.is_empty() {
                    agent_providers::custom_provider(
                        &name,
                        &endpoint,
                        s.model.as_deref().unwrap_or(""),
                    )
                } else {
                    agent_providers::custom_provider_multi(&name, &endpoint, &models_refs)
                };
                cfg.api_key = Some(s.api_key.clone());
                cfg
            }
            Some(id) => {
                let model = selected_models
                    .first()
                    .cloned()
                    .or_else(|| s.model.clone())
                    .unwrap_or_default();
                let mut cfg = agent_providers::provider_from_preset(id, &model);
                cfg.api_key = Some(s.api_key.clone());
                // models 只保留用户选中的（一次配置可加多个，文档 §9）
                cfg.models.retain(|m| selected_models.contains(&m.id));
                cfg
            }
            None => self.provider_cfg.clone(),
        }
    }

    /// Setup 测试结果回填 + 成功时保存配置并进入 Done。
    pub(crate) fn on_setup_test_done(&mut self, text: String) {
        let Some(s) = &mut self.setup else { return };
        s.testing = false;
        s.test_result = Some(text.clone());
        if !text.starts_with('✓') {
            return; // 失败：保留 pending，允许修改后重试（文档 §十一）
        }
        s.test_passed = true;
        s.step = SetupStep::Done; // 成功直接进 Done，避免停死在 Test 步（回归）
        // Apply 到当前 provider + 持久化 runtime config
        let cfg = self.build_pending_config();
        let name = cfg.name.clone();
        if let Ok(client) = agent_providers::OpenAiClient::new(cfg.clone()) {
            self.provider = Arc::new(client);
        }
        // 更新 all_providers 中的该 provider（追加而非覆盖，保留已配置的其它 provider）
        if let Some(p) = self.all_providers.iter_mut().find(|p| p.name == name) {
            *p = cfg.clone();
        } else {
            self.all_providers.push(cfg.clone());
        }
        self.provider_cfg = cfg.clone();
        // 持久化凭证到 auth.toml（合并已有 provider 的 key，不覆盖）
        let key = cfg.api_key.clone().unwrap_or_default();
        let auth_path = agent_providers::AuthConfig::auth_path()
            .unwrap_or_else(|_| std::path::PathBuf::from("auth.toml"));
        let mut auth = agent_providers::AuthConfig::load(&auth_path).unwrap_or_default();
        auth.providers.insert(
            name.clone(),
            agent_providers::ProviderAuth {
                api_key: if key.is_empty() { None } else { Some(key) },
                api_key_env: None,
            },
        );
        if let Err(e) = auth.save(&auth_path) {
            tracing::warn!("保存凭证失败: {e}");
        }
        // 持久化 Provider/Model 定义（api_key 已由 serde skip_serializing 剥离，不落盘）
        let persist = agent_providers::Config {
            default_provider: name.clone(),
            default_model: format!("{}/{}", name, cfg.model),
            max_cost: self.max_cost,
            providers: self.all_providers.clone(),
        };
        let path = self.config_file.clone();
        if let Err(e) = persist.save(&path) {
            tracing::warn!("保存 runtime config 失败: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::commands::course_delete_tests::test_app;

    #[test]
    fn setup_state_transitions() {
        let mut app = test_app();
        app.start_setup();
        let s = app.setup.as_ref().unwrap();
        assert_eq!(s.step, SetupStep::Provider);
        // Provider 选择 deepseek → Model
        app.setup_select();
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Model);
        assert_eq!(
            app.setup.as_ref().unwrap().provider.as_deref(),
            Some("deepseek")
        );
        // Model 选择 → Credentials
        app.setup_select();
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Credentials);
    }

    #[test]
    fn provider_selection_lists_registry_and_custom() {
        let mut app = test_app();
        app.start_setup();
        let opts = app.setup.as_ref().unwrap().provider_options();
        let names: Vec<&str> = opts.iter().map(|(_, n)| *n).collect();
        assert!(names.contains(&"DeepSeek"));
        assert!(names.contains(&"GLM (Zhipu)"));
        assert!(names.contains(&"OpenAI"));
        assert!(names.contains(&"Custom OpenAI-compatible"));
    }

    #[test]
    fn model_selection_from_registry() {
        let mut app = test_app();
        app.start_setup();
        {
            let s = app.setup.as_mut().unwrap();
            s.provider = Some("deepseek".into());
            s.cursor = 0;
        }
        let models = app.setup.as_ref().unwrap().model_options();
        assert!(models.contains(&"deepseek-reasoner".to_string()));
    }

    #[test]
    fn secret_input_masking_preserves_typed_value() {
        let mut app = test_app();
        app.start_setup();
        // 手动进入 Credentials 态
        app.setup.as_mut().unwrap().step = SetupStep::Credentials;
        // 模拟输入密钥
        let ev = |c: char| {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            )
        };
        app.handle_setup_key(ev('s'));
        app.handle_setup_key(ev('k'));
        assert_eq!(app.setup.as_ref().unwrap().secret_buf, "sk");
        // Enter → api_key 写入 + 进入 Test
        let enter = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        app.handle_setup_key(enter);
        assert_eq!(app.setup.as_ref().unwrap().api_key, "sk");
    }

    #[test]
    fn esc_back_navigation_walks_steps() {
        let mut app = test_app();
        app.start_setup();
        app.setup.as_mut().unwrap().step = SetupStep::Done;
        app.setup_back();
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Test);
        app.setup_back();
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Credentials);
        app.setup_back();
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Model);
        app.setup_back();
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Provider);
        app.setup_back();
        assert!(app.setup.is_none(), "Provider 再 Esc 回 Home");
    }

    #[test]
    fn custom_provider_builds_pending_config() {
        let mut app = test_app();
        app.start_setup();
        {
            let s = app.setup.as_mut().unwrap();
            s.provider = Some("custom".into());
            s.custom_name = "my-llm".into();
            s.custom_base_url = "http://localhost:8000/v1".into();
            s.custom_model = "model-x".into();
            s.api_key = "sk-test".into();
        }
        let cfg = app.build_pending_config();
        assert_eq!(cfg.name, "my-llm");
        assert_eq!(cfg.endpoint, "http://localhost:8000/v1");
        assert_eq!(cfg.model, "model-x");
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));
        assert!(!cfg.known_pricing(), "自定义 provider pricing 可选");
    }

    /// Custom provider 四步 UI 输入（Provider → Name → Base URL → Model → Credentials）。
    fn type_chars(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_setup_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
    }
    fn enter(app: &mut App) {
        app.handle_setup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
    }

    #[test]
    fn custom_provider_ui_walkthrough() {
        let mut app = test_app();
        app.start_setup();
        // Provider 步：光标移到 Custom OpenAI-compatible（最后一个）→ Enter
        {
            let opts = app.setup.as_ref().unwrap().provider_options();
            let custom_idx = opts.len() - 1;
            app.setup.as_mut().unwrap().cursor = custom_idx;
        }
        enter(&mut app);
        assert_eq!(
            app.setup.as_ref().unwrap().step,
            SetupStep::CustomName,
            "选 Custom 进 Name 输入"
        );
        // Name
        type_chars(&mut app, "my-llm");
        enter(&mut app);
        assert_eq!(app.setup.as_ref().unwrap().custom_name, "my-llm");
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::CustomBaseUrl);
        // Base URL
        type_chars(&mut app, "http://localhost:8000/v1");
        enter(&mut app);
        assert_eq!(
            app.setup.as_ref().unwrap().custom_base_url,
            "http://localhost:8000/v1"
        );
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::CustomModel);
        // Model
        type_chars(&mut app, "model-x");
        enter(&mut app);
        assert_eq!(app.setup.as_ref().unwrap().custom_model, "model-x");
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Credentials);
        // Key → Test
        type_chars(&mut app, "sk-custom");
        enter(&mut app);
        assert_eq!(app.setup.as_ref().unwrap().api_key, "sk-custom");
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Test);
        // pending config 已带完整 custom 字段
        let cfg = app.build_pending_config();
        assert_eq!(cfg.name, "my-llm");
        assert_eq!(cfg.endpoint, "http://localhost:8000/v1");
        assert_eq!(cfg.model, "model-x");
        assert_eq!(cfg.api_key.as_deref(), Some("sk-custom"));
    }

    /// 问题 1：用户输入的 Custom provider 名（如 paratera）必须保留，不能回退为 "custom"。
    #[test]
    fn custom_provider_name_paratera_is_preserved() {
        let mut app = test_app();
        app.start_setup();
        {
            let s = app.setup.as_mut().unwrap();
            s.provider = Some("custom".into());
            s.custom_name = "paratera".into();
            s.custom_base_url = "https://api.paratera.cn/v1".into();
            s.custom_model = "Qwen3.7-Plus, Deepseek-V4-Flash-0731, GLM-5.3-Flash".into();
            s.api_key = "sk-paratera".into();
        }
        let cfg = app.build_pending_config();
        assert_eq!(cfg.name, "paratera", "provider 名必须为用户输入值");
        assert_eq!(cfg.endpoint, "https://api.paratera.cn/v1");
        assert!(!cfg.name.contains("custom"), "不得残留内部 id");
        // Test Connection 用的 cfg：name=paratera、model=第一个实际模型
        assert_eq!(cfg.model, "Qwen3.7-Plus", "活动模型应为第一个");
        assert_eq!(cfg.api_key.as_deref(), Some("sk-paratera"));
    }

    /// 问题 2：逗号分隔的多个模型 → 拆成独立 ModelConfig（trim 空白），而非含逗号单串。
    #[test]
    fn custom_provider_multi_models_split_and_trim() {
        let mut app = test_app();
        app.start_setup();
        let raw_model = {
            let s = app.setup.as_mut().unwrap();
            s.provider = Some("custom".into());
            s.custom_name = "paratera".into();
            s.custom_base_url = "https://api.paratera.cn/v1".into();
            s.custom_model = " Qwen3.7-Plus ,Deepseek-V4-Flash-0731,  GLM-5.3-Flash ,".into();
            s.api_key = "sk-paratera".into();
            s.custom_model.clone()
        };
        let list = split_custom_models(&raw_model);
        assert_eq!(
            list,
            vec!["Qwen3.7-Plus", "Deepseek-V4-Flash-0731", "GLM-5.3-Flash"],
            "逗号拆分 + trim + 空项丢弃"
        );
        let cfg = app.build_pending_config();
        assert_eq!(cfg.models.len(), 3, "保存 3 个独立 models");
        let ids: Vec<&str> = cfg.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["Qwen3.7-Plus", "Deepseek-V4-Flash-0731", "GLM-5.3-Flash"],
            "每个 model id 独立且已 trim"
        );
        assert_eq!(cfg.model, "Qwen3.7-Plus", "活动模型 = 第一个");
        assert_eq!(
            cfg.models.iter().filter(|m| m.id.contains(',')).count(),
            0,
            "不得有含逗号的 model id"
        );
    }

    /// 问题 3 配置落盘回归：config.toml 写入后重读，name 与 models 保持。
    /// （模拟 on_setup_test_done 的 persist 结构；api_key 剥离到 auth.toml 不落 config）
    #[test]
    fn custom_config_persist_and_reload_keeps_name_and_models() {
        let mut app = test_app();
        app.start_setup();
        {
            let s = app.setup.as_mut().unwrap();
            s.provider = Some("custom".into());
            s.custom_name = "paratera".into();
            s.custom_base_url = "https://api.paratera.cn/v1".into();
            s.custom_model = "Qwen3.7-Plus, Deepseek-V4-Flash-0731, GLM-5.3-Flash".into();
            s.api_key = "sk-paratera".into();
        }
        let cfg = app.build_pending_config();
        let name = cfg.name.clone();
        let persist = agent_providers::Config {
            default_provider: name.clone(),
            default_model: format!("{}/{}", name, cfg.model),
            max_cost: app.max_cost,
            providers: vec![cfg.clone()],
        };
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("sp-setup-custom-{ns}.toml"));
        persist.save(&path).unwrap();
        let reloaded = agent_providers::Config::load(&path).unwrap();
        assert_eq!(reloaded.default_provider, "paratera");
        let p = reloaded.default_provider().unwrap();
        assert_eq!(p.name, "paratera", "重读后 provider 名仍为 paratera");
        assert_eq!(p.models.len(), 3);
        let ids: Vec<&str> = p.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["Qwen3.7-Plus", "Deepseek-V4-Flash-0731", "GLM-5.3-Flash"]
        );
        assert_eq!(p.model, "Qwen3.7-Plus");
        // canonical id：provider/model
        assert_eq!(
            reloaded.resolve_default_model().unwrap(),
            "paratera/Qwen3.7-Plus"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Esc 逐级回退 Custom 输入步。
    #[test]
    fn custom_esc_back_walks_steps() {
        let mut app = test_app();
        app.start_setup();
        app.setup.as_mut().unwrap().step = SetupStep::CustomModel;
        let esc = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        app.handle_setup_key(esc);
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::CustomBaseUrl);
        app.handle_setup_key(esc);
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::CustomName);
        app.handle_setup_key(esc);
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Provider);
        app.handle_setup_key(esc);
        assert!(app.setup.is_none(), "Provider 再 Esc 回 Home");
    }

    /// 回归：坏 key 测试失败 → 改好 key → 成功必须能离开 Test 步。
    /// 旧 bug：Test 成功不推进 step，Test 步 Enter 又恒重跑测试 → 永远停在 Test（"不能加入模型"）。
    #[test]
    fn test_step_advances_to_done_after_success_only() {
        let mut app = test_app();
        app.start_setup();
        let ev = |c: char| {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            )
        };
        let enter = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let esc = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        // 进入 Credentials，输入坏 key → Enter 进 Test
        app.setup.as_mut().unwrap().step = SetupStep::Credentials;
        for c in "bad-key".chars() {
            app.handle_setup_key(ev(c));
        }
        app.handle_setup_key(enter);
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Test);
        assert!(!app.setup.as_ref().unwrap().test_passed);
        // 失败事件：留在 Test，test_passed=false（Enter 走重测分支，不会前进）
        app.on_setup_test_done("✗ 连接失败（HTTP 401）: invalid key".into());
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Test);
        assert!(!app.setup.as_ref().unwrap().test_passed);
        // Esc 回 Credentials：输入框为空（旧 key 保留在 api_key 供渲染提示）
        app.handle_setup_key(esc);
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Credentials);
        assert!(
            app.setup.as_ref().unwrap().secret_buf.is_empty(),
            "回 Credentials 后输入框应为空，重新输入直接替换旧 key"
        );
        // 输入好 key → Enter（无需 Delete）
        for c in "good-key".chars() {
            app.handle_setup_key(ev(c));
        }
        app.handle_setup_key(enter);
        assert_eq!(
            app.setup.as_ref().unwrap().api_key,
            "good-key",
            "新 key 必须替换旧 key"
        );
        assert_eq!(app.setup.as_ref().unwrap().step, SetupStep::Test);
        // 成功事件：自动进 Done
        app.on_setup_test_done("✓ API reachable · Authentication valid".into());
        assert!(app.setup.as_ref().unwrap().test_passed);
        assert_eq!(
            app.setup.as_ref().unwrap().step,
            SetupStep::Done,
            "成功必须离开 Test 步"
        );
        // Done Enter → 关闭 Setup 回 Home
        app.handle_setup_key(enter);
        assert!(app.setup.is_none(), "Done 后 Enter 回 Home");
    }

    /// 回归：Test 失败回 Credentials 时旧 key 不可残留——再输入新 key Enter 必须替换。
    #[test]
    fn reentering_credentials_overwrites_old_key() {
        let mut app = test_app();
        app.start_setup();
        let ev = |c: char| {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            )
        };
        let enter = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let esc = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        app.setup.as_mut().unwrap().step = SetupStep::Credentials;
        for c in "old-key".chars() {
            app.handle_setup_key(ev(c));
        }
        app.handle_setup_key(enter);
        assert_eq!(app.setup.as_ref().unwrap().api_key, "old-key");
        // Test 失败 → Esc 回 Credentials → 输入新 key → Enter
        app.handle_setup_key(esc);
        for c in "new-key".chars() {
            app.handle_setup_key(ev(c));
        }
        app.handle_setup_key(enter);
        assert_eq!(
            app.setup.as_ref().unwrap().api_key,
            "new-key",
            "第二次输入必须覆盖旧 key"
        );
    }
}
