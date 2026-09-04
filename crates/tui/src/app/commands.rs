//! 斜杠命令分发与各命令 handler（D6：命令直连，不进 agent loop）。

use std::sync::Arc;

use agent_core::{Message, Provider};
use agent_providers::OpenAiClient;
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, CourseOpOutcome, Entry, ModelPicker};
use crate::course_cmd::{CourseAction, collect_flags, parse_course_action, parse_review_action};
use crate::review;
use storage::Store;

impl App {
    /// 用户按 Enter 提交。斜杠命令随时可执行；
    /// 普通消息请求中忽略新提交（串行，避免费用与状态混乱）。
    pub(crate) fn submit(&mut self) {
        // 命令/聊天都是 Session workspace 的行为：从 Home/Course 发起时先切入会话工作区
        self.enter_session_workspace();
        let text = self.input.trim().to_owned();
        self.input.clear();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') {
            self.handle_command(&text);
            return;
        }
        if self.is_inflight() {
            self.push_entry(Entry::Info(
                "请求进行中，请等待完成或 Ctrl+C 中断后再发送".into(),
            ));
            return;
        }
        // R6 预算熔断：发起下一个请求前检查，超限拒绝并提示
        if self.total_cost >= self.max_cost {
            self.push_entry(Entry::Error(format!(
                "已达预算上限 ¥{:.2}（累计 ¥{:.4}），拒绝发送。可在 config.toml 调高 max_cost",
                self.max_cost, self.total_cost
            )));
            return;
        }

        let msg = Message::user(text.clone());
        self.history.push(msg.clone());
        self.push_entry(Entry::User(text));
        self.persist(msg);
        self.spawn_chat();
    }

    /// 最小命令分发：已实现的执行，未实现的按里程碑提示（错误信息即帮助）。
    pub(crate) fn handle_command(&mut self, text: &str) {
        self.push_entry(Entry::User(text.to_owned()));

        let mut parts = text.split_whitespace();
        let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
        let arg = super::strip_placeholder_tokens(&parts.collect::<Vec<_>>().join(" "));

        // inflight 门控：会话切换/替换类命令在请求进行中拒绝，
        // 防止 Done 事件把上一个会话的消息写入新会话（跨会话数据混写）
        const SESSION_SWITCHERS: &[&str] = &["/open", "/load", "/new", "/rename"];
        if SESSION_SWITCHERS.contains(&cmd.as_str()) && self.is_inflight() {
            self.push_entry(Entry::Error(
                "请求进行中，请等待完成或 Ctrl+C 中断后再执行会话切换".into(),
            ));
            return;
        }

        match cmd.as_str() {
            "/help" => {
                for l in [
                    "StudyPilot — Quick Guide",
                    "",
                    "按 Ctrl+K 打开命令面板（全部动作/搜索）。",
                    "详细用法见 docs/核心代码逻辑.md 与项目 README。",
                ] {
                    self.push_entry(Entry::Info(l.into()));
                }
            }
            "/course" => self.handle_course_command(&arg),
            "/model" => self.handle_model_command(arg.trim()),
            "/test" => self.handle_test_command(),
            "/new" => self.start_new_session(),
            "/sessions" => self.open_session_browser(),
            "/open" => self.handle_open_command(&arg),
            "/export" => self.export_session(),
            "/rename" => match self.workspace {
                crate::app::Workspace::Course => {
                    if arg.trim().is_empty() {
                        self.take_input_for_overlay();
                        self.wizard = Some(crate::wizard::Wizard::new_for(
                            crate::wizard::WizardKind::RenameCourse,
                            "重命名课程",
                            "新课程名",
                        ));
                        self.enter_wizard_step();
                    } else {
                        self.rename_course(arg.trim());
                    }
                }
                crate::app::Workspace::Session => {
                    if arg.trim().is_empty() {
                        self.take_input_for_overlay();
                        self.wizard = Some(crate::wizard::Wizard::new_for(
                            crate::wizard::WizardKind::RenameSession,
                            "重命名会话",
                            "新标题",
                        ));
                        self.enter_wizard_step();
                    } else {
                        self.rename_session(arg.trim());
                    }
                }
                crate::app::Workspace::Home => {
                    self.push_entry(Entry::Error("请先进入课程或会话再重命名".into()));
                }
            },
            "/load" => {
                if arg.trim().is_empty() {
                    self.push_entry(Entry::Error("用法: /load <导出的 JSON 文件路径>".into()));
                } else {
                    self.load_session_file(arg.trim());
                }
            }
            "/import" => self.handle_import_command(arg.trim()),
            "/delete" => self.handle_delete_command(arg.trim()),
            "/move" => self.handle_move_command(arg.trim()),
            "/budget" => self.handle_budget_command(arg.trim()),
            "/notes" => self.handle_notes_command(),
            "/outline" => self.handle_outline_command(arg.trim()),
            "/search" => self.not_implemented("/search", "M7（检索能力经 agent 工具自动调度）"),
            "/refresh-concepts" => self.handle_refresh_concepts(),
            "/review-map" => self.handle_review_map_command(arg.trim()),
            "/review" => self.handle_review_command(arg.trim()),
            _ => {
                self.push_entry(Entry::Error(format!(
                    "未知命令 `{cmd}`，输入 /help 查看可用命令"
                )));
            }
        }
    }

    /// /refresh-concepts —— 对当前分区课程的全部笔记重抽概念。
    /// 唯一输入 = DB 存储全文（canonical content，不依赖源文件）；
    /// 收尾清理「零关联零历史」概念，有学习记录的绝不删。
    pub(crate) fn handle_refresh_concepts(&mut self) {
        self.enter_session_workspace();
        if self.is_inflight() || self.import_cancel.is_some() {
            self.push_entry(Entry::Error("有任务进行中，请先完成或 Ctrl+C 中断".into()));
            return;
        }
        // 预算熔断（R6）：逐篇 LLM 重抽是 refresh 的开销源
        if self.total_cost >= self.max_cost {
            self.push_entry(Entry::Error(format!(
                "已达预算上限 ¥{:.2}（累计 ¥{:.4}），拒绝刷新。可用 /budget 调高上限",
                self.max_cost, self.total_cost
            )));
            return;
        }
        let Some(course_id) = self.current_course_id() else {
            self.push_entry(Entry::Error(
                "概念刷新需要课程分区：先 /course <名> 切换".into(),
            ));
            return;
        };
        let cancel = CancellationToken::new();
        self.inflight = Some(cancel.clone());
        let store = Arc::clone(&self.store);
        let (provider, provider_cfg) = self.role_client(agent_providers::ModelRole::Fast);
        let max_cost = self.max_cost;
        let tx = self.tx.clone();
        // Agent Trace：START（完成行由 ConceptsRefreshed 汇总回投）
        self.push_entry(Entry::Tool {
            text: "[抽取] 逐篇重抽概念（全文输入，Ctrl+C 可中断）…".into(),
            ok: None,
        });
        tokio::spawn(async move {
            let result = importer::refresh_course_concepts(
                store,
                provider,
                provider_cfg,
                course_id,
                max_cost,
                &cancel,
            )
            .await;
            let _ = tx.send(AppEvent::ConceptsRefreshed(result));
        });
    }

    pub(crate) fn not_implemented(&mut self, cmd: &str, milestone: &str) {
        self.push_entry(Entry::Error(format!(
            "{cmd} 尚未实现（计划于 {milestone}）"
        )));
    }

    // ---- 预算管理 ----

    /// `/budget`：查/改/重置预算。
    /// 打开会话浏览器（搜索/恢复/重命名/删除）；默认只显示当前课程（文档 §9）。
    /// palette 直开与 /sessions /open 无参共用。
    pub(crate) fn open_session_browser(&mut self) {
        self.take_input_for_overlay();
        self.session_browser = Some(crate::session_browser::SessionBrowser::new(
            self.current_course_id(),
        ));
        self.session_browser_search();
    }

    /// `/open <id>`：按 id 打开会话；无参 = 打开会话浏览器。
    pub(crate) fn handle_open_command(&mut self, arg: &str) {
        match arg.trim().parse::<i64>() {
            Ok(id) => self.open_session(id),
            Err(_) if arg.trim().is_empty() => self.open_session_browser(),
            Err(_) => self.push_entry(Entry::Error(
                "用法: /open <会话 id>（无参数打开浏览器）".into(),
            )),
        }
    }

    /// `/budget`：无参显示预算；`reset` 清零；数字设置上限。
    pub(crate) fn handle_budget_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let remaining = (self.max_cost - self.total_cost).max(0.0);
            self.push_entry(Entry::Info(format!(
                "预算: 累计 ¥{:.4} / 上限 ¥{:.2} / 剩余 ¥{:.4}",
                self.total_cost, self.max_cost, remaining
            )));
            return;
        }

        if arg == "reset" {
            let store = Arc::clone(&self.store);
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let result =
                    spawn_blocking(move || store.clear_usage_log().map_err(|e| e.to_string()))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                let _ = tx.send(AppEvent::BudgetReset(result));
            });
            return;
        }

        // 设置新上限（持久化到 data/budget.json——重启后保留，问题16）
        match arg.parse::<f64>() {
            Ok(val) if val > 0.0 => {
                self.max_cost = val;
                let dir = std::path::Path::new("data");
                let _ = std::fs::create_dir_all(dir);
                let _ =
                    std::fs::write(dir.join("budget.json"), format!(r#"{{"max_cost": {val}}}"#));
                self.push_entry(Entry::Info(format!(
                    "预算上限已设为 ¥{val:.2}（已保存，重启保留）"
                )));
            }
            _ => {
                self.push_entry(Entry::Error(
                    "用法: /budget [金额 | reset]（金额须为正数）".into(),
                ));
            }
        }
    }

    pub(crate) fn on_budget_reset(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.total_cost = 0.0;
                self.session_cost = 0.0;
                self.push_entry(Entry::Info("累计花费已清零".into()));
            }
            Err(e) => self.push_entry(Entry::Error(format!("清零失败: {e}"))),
        }
    }

    // ---- M8：复习模式 ----

    /// `/review <课程> [概念] [--n 数量]`：启动复习出题。
    pub(crate) fn handle_review_command(&mut self, arg: &str) {
        self.enter_session_workspace();
        // 无参 = 参数向导（课程自动取当前分区），与 /help 承诺一致
        if arg.trim().is_empty() {
            self.open_review_wizard();
            return;
        }
        if self.review.is_some() {
            self.push_entry(Entry::Error("复习进行中，请先完成或 Esc 退出".into()));
            return;
        }
        // 预算熔断（R6）：出题前检查
        if self.total_cost >= self.max_cost {
            self.push_entry(Entry::Error(format!(
                "已达预算上限 ¥{:.2}（累计 ¥{:.4}），拒绝出题。可用 /budget 调高上限",
                self.max_cost, self.total_cost
            )));
            return;
        }
        let known: Vec<String> = self.courses.iter().map(|(_, n)| n.clone()).collect();
        match parse_review_action(arg, &known) {
            Ok(spec) => {
                // all = 全部笔记出题（course_id=None）
                let course_id = if spec.course == "all" {
                    None
                } else {
                    self.courses
                        .iter()
                        .find(|(_, n)| *n == spec.course.as_str())
                        .map(|(id, _)| *id)
                };
                if course_id.is_none() && spec.course != "all" {
                    self.push_entry(Entry::Error(format!("课程 `{}` 不存在", spec.course)));
                    return;
                }
                self.run_review(course_id, spec.course, spec.scope, spec.n);
            }
            Err(e) => self.push_entry(Entry::Error(e)),
        }
    }

    /// 复习执行（结构化入口：手输解析与向导直连共用）。
    pub(crate) fn run_review(
        &mut self,
        course_id: Option<i64>,
        course_name: String,
        scope: Option<String>,
        n: usize,
    ) {
        if self.review.is_some() {
            self.push_entry(Entry::Error("复习进行中，请先完成或 Esc 退出".into()));
            return;
        }
        // 预算熔断（R6）：出题前检查
        if self.total_cost >= self.max_cost {
            self.push_entry(Entry::Error(format!(
                "已达预算上限 ¥{:.2}（累计 ¥{:.4}），拒绝出题。可用 /budget 调高上限",
                self.max_cost, self.total_cost
            )));
            return;
        }
        // 正式 Review 出题（含逐题生成）→ Balanced 角色模型
        let (provider, provider_cfg) = self.role_client(agent_providers::ModelRole::Balanced);
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        // 出题期间挂 inflight：header 显示进行中，Ctrl+C 可取消
        let cancel = CancellationToken::new();
        self.inflight = Some(cancel.clone());

        let scope_disp = scope
            .as_deref()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| "随机范围（薄弱优先）".into());
        self.push_entry(Entry::Info(format!(
            "开始出题: 《{course_name}》范围「{scope_disp}」，{n} 题"
        )));

        tokio::spawn(async move {
            review::start_review(
                store,
                provider,
                provider_cfg,
                course_id,
                course_name.to_owned(),
                scope,
                n,
                tx,
                cancel,
            )
            .await;
        });
    }

    /// `/notes`：打开笔记浏览器（搜索 → 多选 → m 移动 / d 删除）。
    /// 范围固化为打开时的分区；搜索词走聊天框共享缓冲。
    pub(crate) fn handle_notes_command(&mut self) {
        let scope = self.current_course_id();
        let label = self.course.clone();
        self.take_input_for_overlay();
        self.note_browser = Some(crate::note_browser::NoteBrowser::new(scope, label));
        self.browser_search();
    }

    /// `/delete <id>` 或 `/delete --course <name>`
    pub(crate) fn handle_delete_command(&mut self, arg: &str) {
        // 全旗标：--id <笔记 id>（单删）或 --course <名>（批量删该课笔记）
        let mut id: Option<i64> = None;
        let mut course_name: Option<String> = None;
        collect_flags(arg, &mut |flag, value| match flag {
            "--id" => id = value.trim().parse::<i64>().ok(),
            "--course" => course_name = Some(value),
            _ => {}
        });
        match (id, course_name) {
            (Some(_), Some(_)) => {
                self.push_entry(Entry::Error("--id 与 --course 只能二选一".into()));
            }
            (None, None) => {
                self.push_entry(Entry::Error(
                    "用法: /delete --id <笔记 id> 或 /delete --course <课程名>".into(),
                ));
            }
            (Some(id), None) => {
                let store = Arc::clone(&self.store);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result =
                        spawn_blocking(move || store.delete_note(id).map_err(|e| e.to_string()))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r);
                    let _ = tx.send(AppEvent::NotesDeleted(result, format!("笔记 #{id}")));
                });
            }
            (None, Some(name)) => {
                let name = name.trim().to_owned();
                if name.is_empty() || name == "all" {
                    self.push_entry(Entry::Error("--course 不能为空或 all".into()));
                    return;
                }
                let Some(cid) = self
                    .courses
                    .iter()
                    .find(|(_, n)| *n == name)
                    .map(|(id, _)| *id)
                else {
                    self.push_entry(Entry::Error(format!("课程 `{name}` 不存在")));
                    return;
                };
                let store = Arc::clone(&self.store);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = spawn_blocking(move || {
                        store
                            .delete_notes_by_course(cid)
                            .map(|n| n > 0)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                    let _ = tx.send(AppEvent::NotesDeleted(result, format!("课程 {name}")));
                });
            }
        }
    }

    /// `/move <id> <课程名>`
    pub(crate) fn handle_move_command(&mut self, arg: &str) {
        let mut id: Option<i64> = None;
        let mut target: Option<String> = None;
        collect_flags(arg, &mut |flag, value| match flag {
            "--id" => id = value.trim().parse::<i64>().ok(),
            "--course" => target = Some(value),
            _ => {}
        });
        let (Some(id), Some(target)) = (id, target) else {
            self.push_entry(Entry::Error(
                "用法: /move --id <笔记 id> --course <目标课程名>".into(),
            ));
            return;
        };
        let target_course_id = if target == "all" {
            None
        } else {
            match self.courses.iter().find(|(_, n)| *n == target.as_str()) {
                Some((cid, _)) => Some(*cid),
                None => {
                    self.push_entry(Entry::Error(format!(
                        "课程 `{target}` 不存在。可用: /course -new {target} 新建"
                    )));
                    return;
                }
            }
        };
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        let target = target.to_owned();
        tokio::spawn(async move {
            let result = spawn_blocking(move || {
                store
                    .move_note(id, target_course_id)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = tx.send(AppEvent::NotesMoved(
                result,
                format!("笔记 #{id} → {target}"),
            ));
        });
    }

    /// `/model [provider/model]`：无参弹窗选择（全量分组：provider 头 + 模型行）；带参直接切换。
    /// `/model [provider[/model]]`：无参弹窗选择（角色配置行 + 全量分组：provider 头 + 模型行）；带参直接切换。
    /// `/model fast|balanced|reasoning`：打开该角色的模型选择（配到 config.toml `[roles]`）。
    pub(crate) fn handle_model_command(&mut self, arg: &str) {
        if arg.is_empty() {
            self.take_input_for_overlay(); // 聊天框缓冲即搜索栏（fzf 风格）
            self.model_picker = Some(ModelPicker::from_providers(
                &self.all_providers,
                &self.provider_cfg.name,
                &self.provider_cfg.model,
                &self.roles,
            ));
            return;
        }
        // role 配置：/model fast|balanced|reasoning → 进入该角色模型选择
        let roles = ["fast", "balanced", "reasoning"];
        if roles.contains(&arg) {
            self.take_input_for_overlay();
            let mut picker = ModelPicker::from_providers(
                &self.all_providers,
                &self.provider_cfg.name,
                &self.provider_cfg.model,
                &self.roles,
            );
            picker.enter_role(arg);
            self.model_picker = Some(picker);
            return;
        }
        // power-user：/model provider/model 或 /model model（默认当前 provider）
        let (provider, model) = match arg.split_once('/') {
            Some((p, m)) => (p.to_string(), m.to_string()),
            None => (self.provider_cfg.name.clone(), arg.to_string()),
        };
        self.apply_model_switch(&provider, &model);
    }

    /// `/test`：连接测试（文档 §十二）——最小成本 chat 调用，
    /// 返回用户可读结果（endpoint 可达 / 鉴权有效 / 模型可用）。
    pub(crate) fn handle_test_command(&mut self) {
        self.enter_session_workspace();
        self.run_connection_test(self.provider_cfg.clone(), AppEvent::TestConnection);
    }

    /// 单次连接测试（对给定 client 做一次最小 chat 调用，返回用户可读结果）。
    /// 供单模型 `/test` 与 Setup 多模型逐测共用。
    pub(crate) async fn connection_test_once(
        client: OpenAiClient,
        cancel: CancellationToken,
    ) -> String {
        // 连接测试走普通 chat（非 JSON mode）：`chat_json` 会带 response_format，
        // DeepSeek 对不含 "json" 字样的 prompt 直接 400（"Prompt must contain the word 'json'"），
        // 而连接测试只关心 endpoint/auth/model 可用，不需要结构化输出。
        let msgs = vec![Message::user("请直接回复：ping")];
        let result = agent_providers::with_cancel(client.chat(&msgs, &[]), &cancel).await;
        match result {
            Some(Ok(resp)) if !resp.content.is_empty() => format!(
                "✓ API reachable · Authentication valid · Model available\n\
                 响应: {}",
                resp.content.chars().take(60).collect::<String>()
            ),
            Some(Ok(_)) => "✓ API reachable · 但模型返回空响应（Invalid response）".to_string(),
            Some(Err(e)) => match e {
                agent_core::Error::Api { status, message } => format!(
                    "✗ 连接失败（HTTP {status}）: {message}\n如果鉴权失败，请检查 API key 是否有效"
                ),
                agent_core::Error::Transport(m) => {
                    format!("✗ Endpoint unreachable: {m}\n检查 base URL 与网络连接")
                }
                other => format!("✗ 连接失败: {other}"),
            },
            None if cancel.is_cancelled() => "连接测试已取消".into(),
            None => "连接测试失败（无响应）".into(),
        }
    }

    /// canonical 连接测试（Setup 与 /test 共用同一业务实现，文档 §十）。
    /// `emit` 决定结果回流到哪个事件（/test → TestConnection；Setup → SetupTestDone）。
    pub(crate) fn run_connection_test(
        &mut self,
        cfg: agent_providers::ProviderConfig,
        emit: fn(String) -> AppEvent,
    ) {
        if self.is_inflight() {
            self.push_entry(Entry::Info("请求进行中，请稍后再试".into()));
            return;
        }
        let client = match OpenAiClient::new(cfg.clone()) {
            Ok(c) => c,
            Err(e) => {
                let _ = self.tx.send(emit(format!("✗ 无法初始化客户端: {e}")));
                return;
            }
        };
        let provider_name = cfg.name.clone();
        let model = cfg.model.clone();
        let cancel = CancellationToken::new();
        self.inflight = Some(cancel.clone());
        let tx = self.tx.clone();
        let _ = tx.send(emit(format!("正在测试连接: {provider_name}/{model} …")));
        tokio::spawn(async move {
            let outcome = Self::connection_test_once(client, cancel).await;
            let _ = tx.send(emit(outcome));
        });
    }

    /// 执行模型切换：定位 provider → 用模型元数据重写 cfg（key/endpoint 原样）→
    /// 重建 client → 持久化 default_model（provider/model），不要求重新输入 key。
    pub(crate) fn apply_model_switch(&mut self, provider: &str, model: &str) {
        let Some(cfg) = self
            .all_providers
            .iter()
            .find(|p| p.name == provider)
            .cloned()
        else {
            let names: Vec<&str> = self.all_providers.iter().map(|p| p.name.as_str()).collect();
            self.push_entry(Entry::Error(format!(
                "未知 provider `{provider}`。可用: {}",
                names.join(", ")
            )));
            return;
        };
        let mut new_cfg = cfg.with_model(model);
        new_cfg.ensure_models();
        match OpenAiClient::new(new_cfg.clone()) {
            Ok(client) => {
                self.provider = Arc::new(client);
                let old = format!("{}/{}", self.provider_cfg.name, self.provider_cfg.model);
                self.provider_cfg = new_cfg.clone();
                // 持久化默认 provider + 默认模型（重启保留；失败不阻断切换）
                let persist = agent_providers::Config {
                    default_provider: cfg.name.clone(),
                    default_model: format!("{provider}/{model}"),
                    max_cost: self.max_cost,
                    providers: self.all_providers.clone(),
                    roles: self.roles.clone(),
                };
                let path = self.config_file.clone();
                if let Err(e) = persist.save(&path) {
                    tracing::warn!("保存 runtime config 失败: {e}");
                }
                self.push_entry(Entry::Info(format!(
                    "已切换: `{old}` → `{}` │ {}（思考模式: {}）",
                    self.provider_cfg.name,
                    self.provider_cfg.model,
                    if self.provider_cfg.thinking {
                        "开"
                    } else {
                        "关"
                    }
                )));
            }
            Err(e) => self.push_entry(Entry::Error(format!("切换失败: {e}"))),
        }
    }

    /// 把模型绑定到角色（UI /model 面板 → fast/balanced/reasoning）：
    /// 只写 `App.roles` + 持久化 `[roles]` 表，不切换当前对话模型。
    pub(crate) fn apply_role_assign(&mut self, role: &str, provider: &str, model: &str) {
        let canonical = format!("{provider}/{model}");
        let old = self.roles.insert(role.to_string(), canonical.clone());
        let persist = agent_providers::Config {
            default_provider: self.provider_cfg.name.clone(),
            default_model: format!("{}/{}", self.provider_cfg.name, self.provider_cfg.model),
            max_cost: self.max_cost,
            providers: self.all_providers.clone(),
            roles: self.roles.clone(),
        };
        let path = self.config_file.clone();
        if let Err(e) = persist.save(&path) {
            tracing::warn!("保存 runtime config 失败: {e}");
        }
        let note = match old {
            Some(prev) if prev != canonical => format!("（原 `{prev}`）"),
            Some(_) | None => String::new(),
        };
        self.push_entry(Entry::Info(format!(
            "已设置角色 `{role}` = `{canonical}` {note}│ 该角色的任务将用此模型，未配置的角色用当前模型"
        )));
    }

    /// `/course`：`-list` 列出全部；`-new <名>` 新建并切换；`-delete <名>` 删除；
    /// `<课程|all>` 切换到已有分区。
    pub(crate) fn handle_course_command(&mut self, arg: &str) {
        let known: Vec<String> = self.courses.iter().map(|(_, n)| n.clone()).collect();
        match parse_course_action(arg, &known) {
            CourseAction::List => {
                let names: Vec<&str> = std::iter::once("all")
                    .chain(self.courses.iter().map(|(_, n)| n.as_str()))
                    .collect();
                self.push_entry(Entry::Info(format!(
                    "当前课程: {} │ 可用: {} │ 用法: /course <课程名|all> │ \
                     -new <名> 新建 │ -delete <名> 删除",
                    self.course,
                    names.join(", ")
                )));
            }
            CourseAction::Switch(name) => self.switch_course(&name),
            CourseAction::Create(name) => self.create_course_flow(name),
            CourseAction::Delete(name) => self.delete_course(&name),
            CourseAction::SwitchById(id) => self.switch_course_by_id(id),
            CourseAction::DeleteById(id) => self.delete_course_by_id(id),
            CourseAction::Invalid(msg) => self.push_entry(Entry::Error(msg)),
        }
    }

    /// canonical：切换到课程（by 名称；"all" = 全库检索范围）。
    /// CLI / palette CourseSwitch / Home 选课共用同一入口。
    /// 切换时换出/换入 per-course 分区（文档 §32）。
    pub(crate) fn switch_course(&mut self, name: &str) {
        if name != self.course {
            self.swap_course_partition(name);
        }
        self.sync_home_cursor_to_course();
        self.sync_session_course();
        self.push_entry(Entry::Info(format!("已切换到课程: {name}")));
        persist_last_course(name);
        self.spawn_course_summary(name.to_owned());
    }

    /// canonical：按 id 切换（对课程名任何字符免疫）。
    pub(crate) fn switch_course_by_id(&mut self, id: i64) {
        let Some(name) = self
            .courses
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.clone())
        else {
            self.push_entry(Entry::Error(format!("课程 #{id} 不存在（可能已删除）")));
            return;
        };
        self.switch_course(&name);
    }

    /// canonical：删除课程（by 名称）。当前课删除后回落 all 区；数据（笔记/概念）
    /// 由外键 ON DELETE SET NULL 保留，不丢。
    pub(crate) fn delete_course(&mut self, name: &str) {
        let is_current = self.course == name;
        let name2 = name.to_owned();
        self.run_course_op(move |store| {
            let deleted = store.delete_course(&name2).map_err(|e| e.to_string())?;
            if !deleted {
                return Err(format!("课程 `{name2}` 不存在"));
            }
            let list = store.list_courses().map_err(|e| e.to_string())?;
            let msg = format!("已删除课程: {name2}（其笔记已回落 all 区）");
            let switch_to = is_current.then(|| "all".to_owned());
            Ok((msg, list, switch_to))
        });
    }

    /// canonical：按 id 删除课程。
    pub(crate) fn delete_course_by_id(&mut self, id: i64) {
        let Some((_, name)) = self.courses.iter().find(|(i, _)| *i == id).cloned() else {
            self.push_entry(Entry::Error(format!("课程 #{id} 不存在（可能已删除）")));
            return;
        };
        self.delete_course(&name);
    }

    /// canonical：重命名当前课程（区分于 rename_session，文档 §15）。
    /// D2 异步写库 → CourseRenamed 事件回填列表 + 同步当前课程。
    pub(crate) fn rename_course(&mut self, new_name: &str) {
        if new_name.is_empty() {
            self.push_entry(Entry::Error("课程名不能为空".into()));
            return;
        }
        let Some(course_id) = self.current_course_id() else {
            self.push_entry(Entry::Error(
                "当前没有可重命名的课程（all 区不可重命名）".into(),
            ));
            return;
        };
        let new_name = new_name.to_owned();
        let name_for_db = new_name.clone();
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ok = spawn_blocking({
                let store = Arc::clone(&store);
                move || {
                    store
                        .rename_course(course_id, &name_for_db)
                        .unwrap_or(false)
                }
            })
            .await
            .unwrap_or(false);
            let _ = tx.send(AppEvent::CourseRenamed(ok, course_id, new_name));
        });
    }

    /// 课程重命名回填：更新内存列表 + 同步当前课程 + Home 光标。
    pub(crate) fn on_course_renamed(&mut self, ok: bool, id: i64, new_name: String) {
        if !ok {
            self.push_entry(Entry::Error("重命名失败：课程不存在或名称重复".into()));
            return;
        }
        let was_current = self.course == self.course_label(Some(id));
        for (cid, name) in self.courses.iter_mut() {
            if *cid == id {
                *name = new_name.clone();
            }
        }
        if was_current {
            self.course = new_name.clone();
        }
        self.sync_home_cursor_to_course();
        self.push_entry(Entry::Info(format!("课程已重命名为: {new_name}")));
        self.request_courses_refresh();
    }

    /// 新建课程（结构化入口：手输解析与向导直连共用）。
    pub(crate) fn create_course_flow(&mut self, name: String) {
        let was_current = self.course == name;
        self.run_course_op(move |store| {
            store
                .get_or_create_course(&name)
                .map_err(|e| e.to_string())?;
            let list = store.list_courses().map_err(|e| e.to_string())?;
            let verb = if list.iter().any(|(_, n)| n == &name) && was_current {
                "已存在，切换到"
            } else {
                "已创建并切换到"
            };
            Ok((format!("{verb}课程: {name}"), list, Some(name)))
        });
    }

    /// 课程管理操作的统一异步执行壳：DB 进 blocking 线程池（D2），
    /// 完成后经事件通道回填（消息 + 刷新列表 + 需切换的分区）。
    pub(crate) fn run_course_op<F>(&mut self, op: F)
    where
        F: FnOnce(&Store) -> CourseOpOutcome + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let outcome = spawn_blocking(move || op(&store))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let _ = tx.send(AppEvent::CourseManaged(outcome));
        });
    }

    pub(crate) fn on_course_managed(&mut self, outcome: CourseOpOutcome) {
        match outcome {
            Ok((msg, list, switch_to)) => {
                self.courses = list;
                let entered = switch_to.clone();
                // 课程创建/切换/删除后，Home 光标必须跟随当前课程（否则 Enter 进旧课）
                // 并换出/换入 per-course 分区（文档 §32）
                if let Some(c) = &switch_to {
                    self.swap_course_partition(c);
                }
                self.sync_home_cursor_to_course();
                // 从 Home 发起创建课程：创建成功 → 直达新课程 Course workspace
                // （见 Course 动作菜单：New conversation / Outline / Review Map / Import）
                if self.pending_course_enter {
                    self.pending_course_enter = false;
                    if let Some(c) = entered.as_deref()
                        && c != "all"
                    {
                        self.request_courses_refresh();
                        self.enter_course_workspace(c);
                        return;
                    }
                }
                self.request_courses_refresh();
                self.push_entry(Entry::Info(msg));
                self.sync_session_course();
                // 进入/创建课程后的上下文反馈卡（delete 回落 all 不算）
                if let Some(c) = entered.as_deref()
                    && c != "all"
                {
                    persist_last_course(c);
                    self.spawn_course_summary(c.to_owned());
                }
            }
            Err(e) => self.push_entry(Entry::Error(format!("课程操作失败: {e}"))),
        }
    }

    /// 课程摘要卡：异步拉取现有统计/会话数据（D2：DB 读进 blocking 线程池），
    /// 完成后经事件通道回流为 `Entry::Markdown` 卡片。
    /// 只在 /course 切换与创建成功后触发；"all" 与普通聊天不触发。
    pub(crate) fn spawn_course_summary(&mut self, course_name: String) {
        if course_name == "all" {
            return;
        }
        let Some(course_id) = self
            .courses
            .iter()
            .find(|(_, n)| *n == course_name.as_str())
            .map(|(id, _)| *id)
        else {
            return;
        };
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let data = spawn_blocking(
                move || -> Result<(usize, usize, usize, Option<String>), String> {
                    let (notes, concepts) =
                        store.course_stats(course_id).map_err(|e| e.to_string())?;
                    let mastery = store
                        .list_concepts_with_mastery(Some(course_id))
                        .map_err(|e| e.to_string())?;
                    // 需巩固 = 有作答且累计正确率 <70%（与 ReviewStatus::from_counts 同口径）
                    let weak = mastery
                        .iter()
                        .filter(|c| c.attempts > 0 && c.correct * 100 < c.attempts * 70)
                        .count();
                    let last_session = store
                        .list_sessions()
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|s| s.course_id == Some(course_id))
                        .and_then(|s| s.title);
                    Ok((notes, concepts, weak, last_session))
                },
            )
            .await;
            // 摘要失败静默（主操作反馈已在上方给出，不打扰用户）
            let Ok(Ok((notes, concepts, weak, last_session))) = data else {
                return;
            };
            let _ = tx.send(AppEvent::CourseSummary {
                course_name,
                notes,
                concepts,
                weak,
                last_session,
            });
        });
    }

    /// 课程摘要卡渲染（进入/创建课程后的上下文反馈）。
    pub(crate) fn on_course_summary(
        &mut self,
        course_name: String,
        notes: usize,
        concepts: usize,
        weak: usize,
        last_session: Option<String>,
    ) {
        self.push_entry(Entry::Markdown(crate::app::cards::course_summary(
            &course_name,
            notes,
            concepts,
            weak,
            last_session.as_deref(),
        )));
    }
}

/// 持久化上次所在课程（data/last_course.json，仿 budget.json；重启恢复用）。
pub(crate) fn persist_last_course(name: &str) {
    let path = super::resolve_last_course_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!(r#"{{"course": {name:?}}}"#));
}

#[cfg(test)]
pub(crate) mod course_delete_tests {
    use super::*;
    use crate::app::AppEvent;

    pub(crate) fn test_app() -> App {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        // 内存库与传入 App 的课程列表保持一致（真实启动时列表来自该库）
        store.get_or_create_course("rust").unwrap();
        store.get_or_create_course("csapp").unwrap();
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            vec![(1, "rust".into()), (2, "csapp".into())],
        )
    }

    fn drain_events(app: &mut App) {
        while let Ok(ev) = app.rx.try_recv() {
            if let AppEvent::CourseManaged(o) = ev {
                app.on_course_managed(o);
            }
        }
    }

    /// 多模型切换：同一 provider 下 A/B 自由切换，key/endpoint 不动（文档 Case 1）。
    #[test]
    fn multi_model_switch_keeps_credentials() {
        let mut app = test_app();
        let base = agent_providers::ProviderConfig {
            name: "deepseek".into(),
            endpoint: "https://api.deepseek.com/v1".into(),
            api_key: Some("sk-deepseek".into()),
            api_key_env: None,
            model: "deepseek-v4-flash".into(),
            models: vec![
                agent_providers::ModelConfig {
                    id: "deepseek-v4-flash".into(),
                    name: Some("DeepSeek V4 Flash".into()),
                    price_prompt: Some(2.0),
                    price_completion: Some(8.0),
                    price_prompt_cached: None,
                    context_length: 65536,
                    thinking: false,
                },
                agent_providers::ModelConfig {
                    id: "deepseek-v4-pro".into(),
                    name: Some("DeepSeek V4 Pro".into()),
                    price_prompt: Some(4.0),
                    price_completion: Some(16.0),
                    price_prompt_cached: None,
                    context_length: 65536,
                    thinking: true,
                },
            ],
            price_prompt: Some(2.0),
            price_completion: Some(8.0),
            price_prompt_cached: None,
            context_length: 65536,
            thinking: false,
        };
        app.all_providers = vec![base];
        app.provider_cfg = app.all_providers[0].clone();
        // 切换到 V4 Pro
        app.apply_model_switch("deepseek", "deepseek-v4-pro");
        assert_eq!(app.provider_cfg.model, "deepseek-v4-pro");
        assert_eq!(app.provider_cfg.price_prompt, Some(4.0));
        assert!(app.provider_cfg.thinking, "切模型带元数据");
        assert_eq!(
            app.provider_cfg.api_key.as_deref(),
            Some("sk-deepseek"),
            "切换模型不得要求重新输入 key"
        );
        // 切回 Flash（A/B 往返）
        app.apply_model_switch("deepseek", "deepseek-v4-flash");
        assert_eq!(app.provider_cfg.model, "deepseek-v4-flash");
        assert_eq!(app.provider_cfg.price_prompt, Some(2.0));
        assert!(!app.provider_cfg.thinking);
    }

    /// Model Role：roles 表配置的 role → 对应 provider/model；未配置 → 回退当前模型。
    #[test]
    fn resolve_role_cfg_uses_roles_table_and_falls_back() {
        use agent_providers::ModelRole;
        let mut app = test_app();
        app.all_providers.push(agent_providers::ProviderConfig {
            name: "deepseek".into(),
            endpoint: "https://api.deepseek.com/v1".into(),
            api_key: Some("sk-deepseek".into()),
            api_key_env: None,
            model: "deepseek-v4-pro".into(),
            models: Vec::new(),
            price_prompt: Some(4.0),
            price_completion: Some(16.0),
            price_prompt_cached: None,
            context_length: 65536,
            thinking: true,
        });
        app.provider_cfg.model = "deepseek-v4-flash".into();
        // 未配置 role → 回退当前模型
        let fallback = app.resolve_role_cfg(ModelRole::Reasoning);
        assert_eq!(fallback.name, "test", "未配置 role 回退当前 provider");
        assert_eq!(
            fallback.model, "deepseek-v4-flash",
            "未配置 role 用当前模型"
        );
        // 配置 fast role → 命中 roles 表
        app.roles
            .insert("fast".into(), "deepseek/deepseek-v4-pro".into());
        let fast = app.resolve_role_cfg(ModelRole::Fast);
        assert_eq!(fast.model, "deepseek-v4-pro", "命中 [roles].fast");
        assert!(fast.thinking, "角色模型带元数据");
        assert_eq!(
            fast.api_key.as_deref(),
            Some("sk-deepseek"),
            "角色切换不要求重新输入 key"
        );
        // roles 表指向未知 provider → 回退当前模型
        app.roles.insert("balanced".into(), "nope/m".into());
        let balanced = app.resolve_role_cfg(ModelRole::Balanced);
        assert_eq!(balanced.model, "deepseek-v4-flash", "未知 provider 回退");
    }

    /// `/model fast|balanced|reasoning`：打开该角色的模型选择面板（配置 `[roles]`），
    /// Enter 绑定后写 roles 落盘，不切换当前模型。
    #[test]
    fn model_role_command_opens_role_picker() {
        let mut app = test_app();
        app.all_providers.push(agent_providers::ProviderConfig {
            name: "deepseek".into(),
            endpoint: "https://api.deepseek.com/v1".into(),
            api_key: Some("sk-deepseek".into()),
            api_key_env: None,
            model: "deepseek-v4-pro".into(),
            models: Vec::new(),
            price_prompt: Some(4.0),
            price_completion: Some(16.0),
            price_prompt_cached: None,
            context_length: 65536,
            thinking: true,
        });
        app.provider_cfg.model = "deepseek-v4-flash".into();
        // /model fast → 进入 fast 角色模型选择（面板存在、role=fast、只剩真实模型）
        app.handle_model_command("fast");
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.role, Some("fast".to_string()));
        assert!(
            picker.options.iter().all(|o| !o.is_role),
            "角色模式只显示真实模型"
        );
        // 选定 fast 角色绑定 deepseek-v4-pro（模拟 Enter 确认）
        let picker = app.model_picker.as_mut().unwrap();
        let ds_idx = picker
            .options
            .iter()
            .position(|o| o.provider == "deepseek" && o.model == "deepseek-v4-pro")
            .expect("deepseek-v4-pro 应在候选中");
        picker.selected = ds_idx;
        app.handle_picker_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.roles.get("fast").unwrap(), "deepseek/deepseek-v4-pro");
        assert_eq!(
            app.provider_cfg.model, "deepseek-v4-flash",
            "角色绑定不改当前对话模型"
        );
        // 绑定后返回角色面板（不关闭），光标落回 fast 行，可继续配置其他角色
        let back = app.model_picker.as_ref().unwrap();
        assert_eq!(back.role, None, "回到普通模式");
        assert_eq!(
            back.selected, 0,
            "光标定位回刚配置的 fast 角色行（便于继续）"
        );
        assert!(
            back.options.iter().take(3).all(|o| o.is_role),
            "面板仍显示三个角色行"
        );
        assert!(app.toast.is_some(), "绑定成功有 toast 提示");
        assert!(
            app.entries
                .iter()
                .any(|e| matches!(e, Entry::Info(s) if s.contains("角色 `fast` = `deepseek/deepseek-v4-pro`")))
        );
        // /model provider/model 直接指定仍可用
        app.handle_model_command("deepseek/deepseek-v4-flash");
        assert_eq!(app.provider_cfg.model, "deepseek-v4-flash");
    }

    /// 无参 /model：面板顶部先列三个角色配置行，Enter 角色行进入对应角色模型选择。
    #[test]
    fn model_command_plain_lists_role_rows_first() {
        let mut app = test_app();
        app.handle_model_command("");
        let picker = app.model_picker.as_ref().unwrap();
        assert!(picker.role.is_none(), "无参 = 普通切换模式");
        assert!(
            picker.options.iter().take(3).all(|o| o.is_role),
            "顶部 3 行为角色配置行"
        );
        assert_eq!(picker.options[0].model_display, "fast");
        assert_eq!(picker.options[1].model_display, "balanced");
        assert_eq!(picker.options[2].model_display, "reasoning");
        // Enter 角色行 → 进入 fast 角色模式
        app.model_picker.as_mut().unwrap().selected = 0;
        app.handle_picker_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            app.model_picker.as_ref().unwrap().role,
            Some("fast".to_string()),
            "角色行 Enter 进入角色选择"
        );
    }

    /// 模型弹窗搜索过滤：编辑键落回普通输入路径（App.input 即搜索栏），refilter 实时缩窄；
    /// 空输入恢复全量；角色行也可被匹配。
    #[test]
    fn model_picker_filter_narrows_by_query() {
        let mut app = test_app();
        app.all_providers.push(agent_providers::ProviderConfig {
            name: "deepseek".into(),
            endpoint: "https://api.deepseek.com/v1".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "deepseek-v4-pro".into(),
            models: vec![agent_providers::ModelConfig {
                id: "deepseek-v4-pro".into(),
                name: Some("DeepSeek V4 Pro".into()),
                price_prompt: Some(4.0),
                price_completion: Some(16.0),
                price_prompt_cached: None,
                context_length: 65536,
                thinking: true,
            }],
            price_prompt: Some(4.0),
            price_completion: Some(16.0),
            price_prompt_cached: None,
            context_length: 65536,
            thinking: true,
        });
        app.handle_model_command("");
        let total = app.model_picker.as_ref().unwrap().filtered.len();
        assert_eq!(total, 5, "3 角色行 + test/m + deepseek/v4-pro");
        let key =
            |code| crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        // 输入 "pro" → 只剩匹配的模型行（deepseek-v4-pro），角色行被过滤掉
        for c in "pro".chars() {
            app.handle_key(key(crossterm::event::KeyCode::Char(c)));
        }
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.filtered.len(), 1, "只命中 v4-pro");
        assert_eq!(picker.options[picker.filtered[0]].model, "deepseek-v4-pro");
        // 空输入 → 恢复全量
        app.input.clear();
        app.sync_picker_filter();
        assert_eq!(app.model_picker.as_ref().unwrap().filtered.len(), 5);
        // 输入角色名 "fast" → 命中 fast 角色行
        for c in "fast".chars() {
            app.handle_key(key(crossterm::event::KeyCode::Char(c)));
        }
        let picker = app.model_picker.as_ref().unwrap();
        assert_eq!(picker.filtered.len(), 1, "只命中 fast 角色行");
        assert!(picker.options[picker.filtered[0]].is_role);
        assert_eq!(picker.options[picker.filtered[0]].model_display, "fast");
    }

    /// role_client：role 命中 roles 表 → 独立 client（不同模型）；未命中 → 当前 client。
    #[test]
    fn role_client_switches_model_only_when_configured() {
        use agent_providers::ModelRole;
        let mut app = test_app();
        app.all_providers.push(agent_providers::ProviderConfig {
            name: "deepseek".into(),
            endpoint: "https://api.deepseek.com/v1".into(),
            api_key: Some("sk-deepseek".into()),
            api_key_env: None,
            model: "deepseek-v4-pro".into(),
            models: Vec::new(),
            price_prompt: Some(4.0),
            price_completion: Some(16.0),
            price_prompt_cached: None,
            context_length: 65536,
            thinking: true,
        });
        app.provider_cfg.model = "deepseek-v4-flash".into();
        // 未配置 role → 复用当前 client
        let (client, cfg) = app.role_client(ModelRole::Reasoning);
        assert_eq!(cfg.model, "deepseek-v4-flash");
        assert!(
            Arc::ptr_eq(&client, &app.provider),
            "未命中 role 复用现有 client"
        );
        // 配置 fast → 新建独立 client，不破坏当前模型
        app.roles
            .insert("fast".into(), "deepseek/deepseek-v4-pro".into());
        let (fast_client, fast_cfg) = app.role_client(ModelRole::Fast);
        assert_eq!(fast_cfg.model, "deepseek-v4-pro");
        assert!(
            !Arc::ptr_eq(&fast_client, &app.provider),
            "角色命中时新建 client"
        );
        assert_eq!(
            app.provider_cfg.model, "deepseek-v4-flash",
            "当前模型不被角色覆盖"
        );
    }

    /// 模型 picker 全量分组：多 provider × 多 model 展开成扁平行，当前模型定位。
    #[test]
    fn model_picker_flattens_providers_and_locates_current() {
        use crate::palette::ModelPicker;
        let cfg_a = agent_providers::ProviderConfig {
            name: "deepseek".into(),
            endpoint: "e".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "deepseek-v4-flash".into(),
            models: vec![
                agent_providers::ModelConfig {
                    id: "deepseek-v4-flash".into(),
                    name: Some("V4 Flash".into()),
                    price_prompt: Some(2.0),
                    price_completion: Some(8.0),
                    price_prompt_cached: None,
                    context_length: 1000,
                    thinking: false,
                },
                agent_providers::ModelConfig {
                    id: "deepseek-v4-pro".into(),
                    name: Some("V4 Pro".into()),
                    price_prompt: Some(4.0),
                    price_completion: Some(16.0),
                    price_prompt_cached: None,
                    context_length: 1000,
                    thinking: true,
                },
            ],
            price_prompt: Some(2.0),
            price_completion: Some(8.0),
            price_prompt_cached: None,
            context_length: 1000,
            thinking: false,
        };
        let cfg_b = agent_providers::ProviderConfig {
            name: "glm".into(),
            endpoint: "e2".into(),
            api_key: Some("k2".into()),
            api_key_env: None,
            model: "glm-5".into(),
            models: vec![agent_providers::ModelConfig {
                id: "glm-5".into(),
                name: None,
                price_prompt: None,
                price_completion: None,
                price_prompt_cached: None,
                context_length: 1000,
                thinking: false,
            }],
            price_prompt: None,
            price_completion: None,
            price_prompt_cached: None,
            context_length: 1000,
            thinking: false,
        };
        let picker = ModelPicker::from_providers(
            &[cfg_a, cfg_b],
            "glm",
            "glm-5",
            &std::collections::BTreeMap::new(),
        );
        // 顶部 3 行角色配置 + 3 个真实模型
        assert_eq!(
            picker.options.len(),
            6,
            "3 角色行 + deepseek 2 模型 + glm 1 模型"
        );
        assert!(
            picker.options[0].is_role && picker.options[1].is_role && picker.options[2].is_role,
            "前 3 行为角色配置行"
        );
        assert_eq!(picker.options[5].provider, "glm");
        assert_eq!(picker.options[5].model, "glm-5");
        assert_eq!(picker.selected, 5, "当前 glm/glm-5 行定位");
        assert!(
            picker.options[3].known_pricing,
            "deepseek 模型 known_pricing"
        );
        assert!(!picker.options[5].known_pricing, "glm 无定价");
        assert!(picker.options[4].thinking, "V4 Pro thinking 元数据");
    }

    /// 问题 13b：模型多时可用 ↑↓/PgUp/PgDn 翻页——selected 能越过视口到达屏外选项，
    /// 渲染由 ListState 自动滚动跟随。
    #[test]
    fn model_picker_pages_beyond_viewport() {
        let mut app = test_app();
        let mut picker = crate::palette::ModelPicker::from_providers(
            &[],
            "p",
            "m",
            &std::collections::BTreeMap::new(),
        );
        // 造 30 个选项（> 视口高 24-2）
        picker.options = (0..30)
            .map(|i| crate::palette::ModelOption {
                provider: "p".into(),
                provider_display: "P".into(),
                model: format!("m{i}"),
                model_display: format!("M{i}"),
                known_pricing: true,
                thinking: false,
                is_role: false,
            })
            .collect();
        picker.filtered = (0..30).collect();
        picker.selected = 0;
        app.model_picker = Some(picker);

        let key =
            |code| crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        // Down ×10：逐行移动，应到达第 10 个
        for _ in 0..10 {
            app.handle_picker_key(key(crossterm::event::KeyCode::Down));
        }
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 10);
        // PageDown：再跳 10 个 → 20
        app.handle_picker_key(key(crossterm::event::KeyCode::PageDown));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 20);
        // End：直达末尾
        app.handle_picker_key(key(crossterm::event::KeyCode::End));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 29);
        // Home：回到开头
        app.handle_picker_key(key(crossterm::event::KeyCode::Home));
        assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
    }

    #[tokio::test]
    async fn delete_current_course_switches_to_all() {
        let mut app = test_app();
        app.course = "rust".into();
        app.handle_course_command("-delete rust");
        for _ in 0..10 {
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(20));
            drain_events(&mut app);
        }
        assert_eq!(app.course, "all", "删除当前课程应回落 all 分区");
        assert!(!app.courses.iter().any(|(_, n)| n == "rust"));
    }

    #[tokio::test]
    async fn delete_other_course_keeps_current() {
        let mut app = test_app();
        app.course = "csapp".into();
        app.handle_course_command("-delete rust");
        for _ in 0..10 {
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(20));
            drain_events(&mut app);
        }
        assert_eq!(app.course, "csapp", "删除其他课程不应改变当前分区");
    }
}

#[cfg(test)]
mod selection_mapping_tests {
    use super::*;

    fn test_app() -> App {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            Vec::new(),
        )
    }

    /// 回归：拖选行号映射与渲染同系（聊天区无边框）。
    /// 屏幕第 k 行（row = area.y + k）必须映射到渲染行 scroll_top + k。
    #[test]
    fn selection_maps_to_rendered_row() {
        let mut app = test_app();
        // chat 区：y=3, h=20（主区中下的典型布局），无滚动（贴底）
        app.chat_rect = ratatui::layout::Rect::new(22, 3, 60, 20);
        app.chat_lines = (0..100).map(|i| format!("line {i}")).collect();
        app.scroll_up = 0;
        // 渲染端：viewport=20，top = max_offset(80) - 0 = 80 → 屏幕首行=lines[80]
        // 点屏幕 y=3+7=10（第 8 行）→ 应映射 lines[87]
        app.update_text_selection(10, 30, true);
        assert_eq!(app.selection_anchor, Some(87), "必须与渲染行一致");
        assert_eq!(app.text_selection, Some((87, 87)));
    }

    /// 单行聊天流（无滚动）：row 直接对应行号。
    #[test]
    fn selection_without_scroll() {
        let mut app = test_app();
        app.chat_rect = ratatui::layout::Rect::new(22, 3, 60, 20);
        app.chat_lines = (0..10).map(|i| format!("line {i}")).collect();
        app.update_text_selection(3, 30, true); // 首行
        assert_eq!(app.selection_anchor, Some(0));
        app.update_text_selection(9, 30, true); // 第 7 行
        assert_eq!(app.selection_anchor, Some(6));
    }
}

#[cfg(test)]
mod overlay_backup_tests {
    use super::course_delete_tests::test_app;

    /// 回归：覆盖层备份嵌套守卫——二次 take 不得覆盖首次备份，
    /// 否则浏览器 Select→Search 再关闭时，原聊天内容丢失。
    #[test]
    fn take_is_nested_safe() {
        let mut app = test_app();
        app.input = "原始聊天内容".into();
        app.take_input_for_overlay(); // 首次：备份原内容
        assert_eq!(app.input, "");
        app.input = "搜索词".into();
        app.take_input_for_overlay(); // 二次：不得覆盖备份
        app.restore_input_backup();
        assert_eq!(app.input, "原始聊天内容", "原聊天内容不得被搜索词覆盖");
    }

    /// 无备份时 restore 不产生任何效果。
    #[test]
    fn restore_without_backup_is_noop() {
        let mut app = test_app();
        app.input = "保留".into();
        app.restore_input_backup();
        assert_eq!(app.input, "保留");
    }
}

#[cfg(test)]
mod onboarding_tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::cards;

    /// 建无课程的空 App（fresh 状态：store 也空）。
    fn empty_app() -> App {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            Vec::new(),
        )
    }

    /// 建两门课的 App（老用户态）。
    fn app_with_courses() -> App {
        super::course_delete_tests::test_app()
    }

    /// 计数 Markdown 卡片中含 needle 的条数（welcome/summary/empty 都是 Entry::Markdown）。
    fn count_md_containing(app: &App, needle: &str) -> usize {
        app.entries
            .iter()
            .filter(|e| matches!(e, Entry::Markdown(m) if m.contains(needle)))
            .count()
    }

    /// 排空事件：处理 CourseManaged（列表回填）与 CourseSummary（摘要卡）。
    fn drain(app: &mut App) {
        while let Ok(ev) = app.rx.try_recv() {
            match ev {
                AppEvent::CourseManaged(o) => app.on_course_managed(o),
                AppEvent::CourseSummary {
                    course_name,
                    notes,
                    concepts,
                    weak,
                    last_session,
                } => app.on_course_summary(course_name, notes, concepts, weak, last_session),
                _ => {}
            }
        }
    }

    /// 异步任务（spawn_blocking 读库）跑完需多轮让步。
    async fn settle(app: &mut App) {
        for _ in 0..10 {
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(20));
            drain(app);
        }
    }

    // ── 场景 A/B/I：Home workspace 启动，不往聊天流塞卡片 ──

    #[tokio::test]
    async fn startup_enters_home_not_chat() {
        // 空库：启动进 Home（原生 Welcome，不 push 聊天卡）
        let mut fresh = empty_app();
        fresh.push_startup_cards();
        assert_eq!(fresh.workspace, crate::app::Workspace::Home);
        assert_eq!(
            count_md_containing(&fresh, "# Welcome to StudyPilot"),
            0,
            "Welcome 由 Home workspace 原生渲染，不再进聊天流"
        );

        // 已有课程：同样进 Home，聊天流保持干净
        let mut existing = app_with_courses();
        existing.push_startup_cards();
        settle(&mut existing).await;
        assert_eq!(existing.workspace, crate::app::Workspace::Home);
        assert_eq!(count_md_containing(&existing, "# rust"), 0, "不 push 卡片");
    }

    // ── 场景 B：创建第一门课程 → Home 让位，进入 Course workspace ──

    #[tokio::test]
    async fn create_first_course_enters_course_workspace() {
        let mut app = empty_app();
        app.handle_course_command("-new rust");
        settle(&mut app).await;
        assert!(app.courses.iter().any(|(_, n)| n == "rust"), "课程应已创建");
        assert_eq!(app.course, "rust", "创建后当前课程应切到新课程");
    }

    // ── 场景 I：重启已有课程 → 仍进 Home（不自动跳工作区） ──

    #[tokio::test]
    async fn restart_stays_home() {
        let mut app = app_with_courses();
        app.push_startup_cards();
        settle(&mut app).await;
        assert_eq!(app.workspace, crate::app::Workspace::Home);
        assert!(
            app.courses.iter().any(|(_, n)| n == "rust"),
            "课程上下文已恢复"
        );
    }

    // ── last course 持久化：/course 切换/创建写盘，启动恢复 ──

    #[test]
    fn last_course_persist_and_restore() {
        // 每次运行用唯一隔离路径（并行测试防共享文件竞争）
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = format!("/tmp/opencode/last_course-{}-{ns}.json", std::process::id());
        crate::app::last_course_path_override(Some(tmp.clone()));
        let path = std::path::Path::new(&tmp);
        let _ = std::fs::remove_file(path);

        persist_last_course("csapp");
        let mut app = app_with_courses();
        assert_eq!(app.course, "rust", "默认课程为 rust");
        app.restore_last_course();
        assert_eq!(app.course, "csapp", "重启应恢复上次所在课程");

        // 课程已不存在 → 忽略，保留默认
        persist_last_course("已删除课");
        app.course = "rust".into();
        app.restore_last_course();
        assert_eq!(app.course, "rust", "失效课程不恢复");

        let _ = std::fs::remove_file(path);
        crate::app::last_course_path_override(None); // 复位，防污染同线程后续测试
    }

    // ── 场景 4/5：切换只弹一次；普通命令不弹 ──

    #[tokio::test]
    async fn switch_course_pushes_summary_once() {
        let mut app = app_with_courses();
        app.course = "rust".into();
        app.handle_course_command("csapp");
        settle(&mut app).await;
        assert_eq!(app.course, "csapp");
        assert_eq!(
            count_md_containing(&app, "# csapp"),
            1,
            "切换课程只应弹一次摘要卡"
        );
    }

    #[tokio::test]
    async fn chat_like_commands_do_not_push_summary() {
        let mut app = app_with_courses();
        app.course = "rust".into();
        // /review 无参走向导：打开的是向导，不是课程上下文反馈
        app.handle_review_command("");
        settle(&mut app).await;
        assert_eq!(
            count_md_containing(&app, "need reinforcement"),
            0,
            "普通命令不应触发 Course Summary"
        );
        assert_eq!(
            count_md_containing(&app, "No materials yet."),
            0,
            "普通命令不应触发 Course Summary"
        );
    }

    // ── 场景 6/7：空课程 /review /outline → 引导卡，不露内部错误 ──

    #[tokio::test]
    async fn empty_course_review_shows_guidance() {
        let mut app = app_with_courses();
        // 引擎在空课程时上报的原始文案
        app.on_review_ready(Err("该课程还没有笔记，先 /import 导入资料".into()));
        assert_eq!(
            count_md_containing(&app, "# Nothing to review yet"),
            1,
            "空课程 /review 应显示引导卡"
        );
        assert!(
            !app.entries.iter().any(|e| matches!(e, Entry::Error(_))),
            "不应出现内部错误条目"
        );
    }

    #[tokio::test]
    async fn empty_course_outline_shows_guidance() {
        let mut app = app_with_courses();
        app.on_outline_ready(Err(
            "复习地图生成失败: 该课程还没有概念：先 /import 导入资料".into(),
        ));
        assert_eq!(
            count_md_containing(&app, "# Nothing to outline yet"),
            1,
            "空课程 /outline 应显示引导卡"
        );
        assert!(
            !app.entries.iter().any(|e| matches!(e, Entry::Error(_))),
            "不应出现内部错误条目"
        );
    }

    #[tokio::test]
    async fn real_error_still_shown_as_error() {
        let mut app = app_with_courses();
        app.on_review_ready(Err("出题失败: 服务端 500".into()));
        assert!(
            app.entries
                .iter()
                .any(|e| matches!(e, Entry::Error(m) if m.contains("出题失败"))),
            "真实错误仍走 Error 条目"
        );
    }

    // ── 场景 8/9：摘要卡反映真实库状态；无历史不造假 ──

    #[tokio::test]
    async fn summary_reflects_real_counts() {
        let mut app = app_with_courses();
        // rust 课程(id=1)：2 篇笔记 + 2 概念（1 弱）+ 最近会话
        let store = app.store.clone();
        let note_ids: Vec<i64> = (0..2)
            .map(|i| {
                let out = store
                    .insert_note(storage::NewNote {
                        course_id: Some(1),
                        title: &format!("note{i}"),
                        source_path: None,
                        content: &format!("content-{i} unique 内容"),
                        fts_content: Some(&format!("content-{i} 分词")),
                    })
                    .unwrap();
                match out {
                    storage::InsertOutcome::Created(n) => n.id,
                    _ => panic!("应创建成功"),
                }
            })
            .collect();
        let mastered = store.get_or_create_concept("mastered", Some(1)).unwrap();
        let weak = store.get_or_create_concept("weak", Some(1)).unwrap();
        for &nid in &note_ids {
            store.link_note_concept(nid, mastered).unwrap();
            store.link_note_concept(nid, weak).unwrap();
        }
        store.update_concept_mastery(mastered, true).unwrap();
        store.update_concept_mastery(mastered, true).unwrap();
        store.update_concept_mastery(mastered, true).unwrap();
        store.update_concept_mastery(weak, false).unwrap();
        store
            .create_session(Some("Ownership & Borrowing"), Some(1))
            .unwrap();

        app.course = "rust".into();
        app.spawn_course_summary("rust".into());
        settle(&mut app).await;

        let card: Vec<&Entry> = app
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Markdown(m) if m.contains("# rust")))
            .collect();
        assert_eq!(card.len(), 1);
        let Entry::Markdown(md) = card[0] else {
            unreachable!()
        };
        assert!(md.contains("2 notes · 2 concepts"), "真实计数不符: {md}");
        assert!(
            md.contains("△ 1 need reinforcement"),
            "弱概念计数不符: {md}"
        );
        assert!(
            md.contains("Ownership & Borrowing"),
            "最近会话应显示真实标题: {md}"
        );
    }

    #[test]
    fn summary_without_session_shows_get_started() {
        let md = cards::course_summary("Rust", 5, 65, 3, None);
        assert!(md.contains("5 notes · 65 concepts · △ 3 need reinforcement"));
        assert!(!md.contains("Continue"), "无历史会话不得造假: {md}");
        assert!(
            md.contains("Your course is ready"),
            "应显示 Get started 引导: {md}"
        );
        assert!(md.contains("`/review` to practice"));
        assert!(md.contains("`/outline` to view your knowledge map"));

        // 空材料态
        let empty = cards::course_summary("Rust", 0, 0, 0, None);
        assert!(empty.contains("0 notes · 0 concepts"));
        assert!(empty.contains("No materials yet."));
    }

    /// 有最近 session：突出 Continue，不造假。
    #[test]
    fn summary_with_session_shows_continue() {
        let md = cards::course_summary("Rust", 5, 65, 3, Some("Ownership & Borrowing"));
        assert!(md.contains("**Continue:** Ownership & Borrowing"));
        assert!(md.contains("`/review` · `/review-map` · `/outline` · `/import`"));
        assert!(
            !md.contains("Your course is ready"),
            "有历史时不显示空态引导"
        );
    }

    #[test]
    fn welcome_and_empty_cards_are_concise() {
        // 视觉一致性：三张卡都是 Markdown，且命令全部走 inline code
        for md in [
            cards::welcome_guide(),
            cards::empty_review(),
            cards::empty_outline(),
        ] {
            assert!(md.starts_with("# "), "卡片应以标题开头");
            assert!(md.contains("`/"), "命令必须 inline code");
        }
    }

    // ── Home / Course / Session 顶层导航转换 ──

    /// 构造「有最近 session」的 App：sidebar_sessions 直接注入。
    fn app_with_session() -> App {
        let mut app = app_with_courses();
        app.sidebar_sessions = vec![storage::SessionMeta {
            id: 26,
            title: Some("Ownership & Borrowing".into()),
            course_id: Some(1),
            created_at: "2026-09-02 10:00:00".into(),
        }];
        app
    }

    #[tokio::test]
    async fn home_activate_enters_course_workspace() {
        // 无 session：Home 光标 = AI 入口(0) + 课程列表(1..) + New Course，Enter 首课程 → Course workspace
        let mut app = app_with_courses();
        assert_eq!(app.home_cursor_count(), 4, "AI 入口 + 2 门课 + New Course");
        app.home_cursor = 1; // 第一门课 rust（0 是 AI 入口）
        app.home_activate();
        settle(&mut app).await;
        assert_eq!(app.workspace, crate::app::Workspace::Course);
        assert_eq!(app.course, "rust", "应进入光标所在的课程");
    }

    #[tokio::test]
    async fn home_continue_enters_session_workspace() {
        let mut app = app_with_session();
        assert_eq!(
            app.home_cursor_count(),
            5,
            "AI 入口 + Continue + 2 门课 + New Course"
        );
        // 光标在 Continue（1，AI 入口之后）→ 打开最近 session
        app.home_cursor = 1;
        app.home_activate();
        settle(&mut app).await;
        assert_eq!(app.workspace, crate::app::Workspace::Session);
        assert_eq!(app.course, "rust", "session 归属课程");
    }

    #[tokio::test]
    async fn home_course_switch_selects_csapp() {
        let mut app = app_with_courses();
        app.home_cursor = 2; // AI(0) + rust(1) + csapp(2)
        app.home_activate();
        settle(&mut app).await;
        assert_eq!(app.workspace, crate::app::Workspace::Course);
        assert_eq!(app.course, "csapp");
    }

    #[tokio::test]
    async fn course_new_conversation_enters_session() {
        // 无 session 的课程：光标 0 = New conversation
        let mut app = app_with_courses();
        app.enter_course_workspace("rust");
        assert_eq!(app.course_cursor_count(), 4, "无 Continue：4 个动作");
        app.course_activate();
        settle(&mut app).await;
        assert_eq!(app.workspace, crate::app::Workspace::Session);
        assert!(app.entries.iter().all(|e| !matches!(e, Entry::User(_))));
    }

    #[tokio::test]
    async fn course_continue_opens_recent_session() {
        let mut app = app_with_session();
        app.enter_course_workspace("rust");
        assert_eq!(
            app.course_cursor_count(),
            5 + 1,
            "Continue + 4 动作 + 1 recent"
        );
        app.course_activate(); // 光标 0 = Continue
        settle(&mut app).await;
        assert_eq!(app.workspace, crate::app::Workspace::Session);
    }

    #[tokio::test]
    async fn esc_navigates_session_back_to_course_then_home() {
        let mut app = app_with_session();
        app.enter_session_workspace();
        assert_eq!(app.workspace, crate::app::Workspace::Session);
        app.go_back();
        assert_eq!(
            app.workspace,
            crate::app::Workspace::Course,
            "Session→Course"
        );
        app.go_back();
        assert_eq!(app.workspace, crate::app::Workspace::Home, "Course→Home");
    }

    #[tokio::test]
    async fn no_session_hides_continue_in_home() {
        // 无 session：Home 不应显示 Continue 项（cursor_count 无 Continue 的 +1）
        let app = app_with_courses();
        assert_eq!(
            app.home_cursor_count(),
            4,
            "AI 入口 + 2 门课 + New Course，无 Continue"
        );
        assert_eq!(app.continue_session(), None, "无 session 不得伪造 Continue");
    }

    #[tokio::test]
    async fn home_new_course_opens_creation_wizard() {
        // 光标到 [+ New Course]（末尾）→ 打开创建向导（对话框直接叠在 Home 上）
        let mut app = app_with_courses();
        app.home_cursor = app.home_cursor_count() - 1;
        app.home_activate();
        assert_eq!(app.workspace, crate::app::Workspace::Home);
        assert!(app.wizard.is_some(), "应打开课程创建向导");
    }
}

#[cfg(test)]
mod course_context_tests {
    use super::*;
    use crate::app::AppEvent;
    use crate::app::Workspace;

    fn drain_full(app: &mut App) {
        while let Ok(ev) = app.rx.try_recv() {
            match ev {
                AppEvent::CourseManaged(o) => app.on_course_managed(o),
                AppEvent::CoursesRefreshed(list, stats) => {
                    app.courses = list;
                    app.sidebar_course_stats = stats;
                }
                AppEvent::SessionsLoaded(list) => app.on_sessions_loaded(list),
                AppEvent::CourseSummary {
                    course_name,
                    notes,
                    concepts,
                    weak,
                    last_session,
                } => app.on_course_summary(course_name, notes, concepts, weak, last_session),
                _ => {}
            }
        }
    }

    async fn settle_full(app: &mut App) {
        for _ in 0..20 {
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(15));
            drain_full(app);
        }
    }

    #[tokio::test]
    async fn create_brand_new_course_keeps_context() {
        // fresh 库：只有 rust，真实创建新课程 pytorch
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.get_or_create_course("rust").unwrap();
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        let mut app = App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            vec![(1, "rust".into())],
        );
        app.course = "rust".into();
        assert_eq!(app.workspace, Workspace::Home);

        app.input = "/course -new pytorch".into();
        app.submit();
        settle_full(&mut app).await;
        assert_eq!(app.course, "pytorch", "创建后当前课程应为 pytorch");

        // Home 光标移到 pytorch（第 2 项），Enter 进 Course
        app.home_cursor = 2; // AI(0) + rust(1) + pytorch(2)
        app.home_activate();
        settle_full(&mut app).await;
        assert_eq!(app.workspace, Workspace::Course);
        assert_eq!(app.course, "pytorch", "Course workspace 应为 pytorch");

        // New conversation → Session
        app.course_cursor = 0;
        app.course_activate();
        assert_eq!(app.workspace, Workspace::Session);
        assert_eq!(
            app.course, "pytorch",
            "Session 上下文应为 pytorch —— 复现根因"
        );
        // 发消息自动建会话 → 会话应归属 pytorch
        app.input = "hello".into();
        app.submit();
        settle_full(&mut app).await;
        let store = app.store.clone();
        let sessions = store.list_sessions().unwrap();
        eprintln!(
            "INFO: sessions={:?}",
            sessions
                .iter()
                .map(|s| (s.id, s.title.clone(), s.course_id))
                .collect::<Vec<_>>()
        );
        assert!(
            sessions.iter().any(|s| s.course_id == Some(2)),
            "新会话应归属 pytorch(course_id=2)"
        );
    }

    /// 复现用户报告：创建 csapp 后 Home 显示 csapp，但从 Home 进入 Session 仍显示 rust。
    /// 关键前提：已存在 rust 的最近 session（continue_session 有值）。
    #[tokio::test]
    async fn create_course_with_prior_session_keeps_context() {
        // 构造：rust(course=1) 已有最近 session；Home 光标在 rust session 上（cursor 0）
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.get_or_create_course("rust").unwrap();
        store.create_session(Some("旧对话"), Some(1)).unwrap();
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        let mut app = App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            vec![(1, "rust".into())],
        );
        app.course = "rust".into();
        app.request_sessions_refresh();
        settle_full(&mut app).await;
        assert!(app.continue_session().is_some(), "应有可继续的旧 session");

        // 在 Home 创建新课程 pytorch
        app.input = "/course -new pytorch".into();
        app.submit();
        settle_full(&mut app).await;
        assert_eq!(app.course, "pytorch", "创建后当前课程应为 pytorch");

        // 进入新课程 Course → New conversation → Session，全程 context 应为 pytorch
        app.home_cursor = 3; // AI(0) + Continue(1) + rust(2) + pytorch(3)
        app.home_activate();
        settle_full(&mut app).await;
        assert_eq!(app.workspace, Workspace::Course);
        assert_eq!(app.course, "pytorch", "Course workspace 应为 pytorch");
        app.course_cursor = 0;
        app.course_activate();
        assert_eq!(app.workspace, Workspace::Session);
        assert_eq!(app.course, "pytorch", "Session 上下文应为 pytorch");
    }

    /// Home 光标在「+ New Course」创建：向导直接叠在 Home 上，创建成功直达新课程 Course 页。
    #[tokio::test]
    async fn create_via_home_wizard_then_enter_keeps_context() {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.get_or_create_course("rust").unwrap();
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        let mut app = App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            vec![(1, "rust".into())],
        );
        app.course = "rust".into();
        assert_eq!(app.workspace, Workspace::Home);

        // Home 光标到末尾 [+ New Course] → Enter：对话框直接叠在 Home 上（不进 Session）
        app.home_cursor = app.home_cursor_count() - 1;
        app.home_activate();
        assert_eq!(
            app.workspace,
            Workspace::Home,
            "创建向导不应切到 Session，直接叠在 Home"
        );
        assert!(app.wizard.is_some());
        // 填向导课程名并完成
        let w = app.wizard.as_mut().unwrap();
        assert!(w.confirm("pytorch".into()), "最后一步");
        app.finish_wizard();
        settle_full(&mut app).await;
        assert_eq!(app.course, "pytorch", "创建后当前课程应为 pytorch");
        assert_eq!(
            app.workspace,
            Workspace::Course,
            "从 Home 创建成功 → 直达新课程 Course 页（New conversation/Outline 等立即可用）"
        );

        // Course 页里 New conversation → Session；Esc 回退回 Course
        app.course_cursor = 0;
        app.course_activate();
        assert_eq!(app.workspace, Workspace::Session);
        assert_eq!(app.course, "pytorch");
        app.go_back();
        assert_eq!(app.workspace, Workspace::Course, "Session→Course");
        assert_eq!(app.course, "pytorch", "Course 应为 pytorch");
    }

    /// Session 内用命令面板建课程：不被拽走，留在 Session。
    #[tokio::test]
    async fn create_from_session_stays_in_session() {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.get_or_create_course("rust").unwrap();
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        let mut app = App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            vec![(1, "rust".into())],
        );
        app.course = "rust".into();
        app.enter_session_workspace();
        app.input = "/course -new pytorch".into();
        app.submit();
        settle_full(&mut app).await;
        assert_eq!(app.course, "pytorch", "创建后当前课程应为 pytorch");
        assert_eq!(
            app.workspace,
            Workspace::Session,
            "Session 内建课不切换 workspace"
        );
    }

    /// 回归：Home 创建 csapp 后光标停在原位直接 Enter，必须进入 csapp 而非 rust。
    /// 旧 bug：app.course 已切但 home_cursor 未同步 → Enter 进旧课。
    #[tokio::test]
    async fn create_course_then_enter_without_cursor_move() {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: Some(0.0),
            price_completion: Some(0.0),
            price_prompt_cached: Some(0.0),
            context_length: 1000,
            thinking: false,
            models: Vec::new(),
        };
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.get_or_create_course("rust").unwrap();
        let client = Arc::new(OpenAiClient::new(cfg.clone()).unwrap());
        let mut app = App::new(
            client,
            store,
            cfg.clone(),
            vec![cfg],
            std::collections::BTreeMap::new(),
            5.0,
            vec![(1, "rust".into())],
        );
        app.course = "rust".into();
        app.home_cursor = 0; // 光标停在 Home 第一项
        assert_eq!(app.workspace, Workspace::Home);

        // 创建新课程 csapp
        app.input = "/course -new csapp".into();
        app.submit();
        settle_full(&mut app).await;
        assert_eq!(
            app.course, "csapp",
            "app.course 已切到 csapp（Home 视觉如此）"
        );

        // 用户不做任何额外操作，直接 Enter（以为自己在新课程）
        app.home_activate();
        settle_full(&mut app).await;
        assert_eq!(
            app.course, "csapp",
            "用户以为进入 csapp，实际进入 {} —— 根因：home_cursor 未随 app.course 迁移",
            app.course
        );
    }

    /// 回归（文档 §32/§38）：per-course 分区隔离——entries/session_cost 不跨课串，
    /// 切回恢复。
    #[tokio::test]
    async fn partition_isolates_entries_between_courses() {
        let mut app = super::course_delete_tests::test_app(); // rust, csapp
        app.course = "rust".into();
        // rust 分区写点内容
        app.push_entry(Entry::User("rust 的聊天".into()));
        app.session_cost = 3.5;
        // 切到 csapp：分区隔离，不应看到 rust 内容
        app.switch_course("csapp");
        assert!(
            app.entries
                .iter()
                .all(|e| !matches!(e, Entry::User(m) if m.contains("rust"))),
            "csapp 分区不应出现 rust 的聊天"
        );
        assert_eq!(app.session_cost, 0.0, "csapp 分区花费独立");
        app.entries.push(Entry::User("csapp 的聊天".into()));
        app.session_cost = 1.0;
        // 切回 rust：恢复 rust 分区
        app.switch_course("rust");
        let has_rust = app
            .entries
            .iter()
            .any(|e| matches!(e, Entry::User(m) if m.contains("rust")));
        assert!(has_rust, "切回 rust 应恢复其聊天");
        assert!(
            app.entries
                .iter()
                .all(|e| !matches!(e, Entry::User(m) if m.contains("csapp"))),
            "rust 分区不应混入 csapp 内容"
        );
        assert_eq!(app.session_cost, 3.5, "rust 分区花费恢复");
    }

    /// 文档 §38-7：current course = csapp，最近 session 属于 rust——
    /// Continue → 恢复 rust 的 session（及其课程）；New Conversation → 用当前课程 csapp。
    #[tokio::test]
    async fn continue_uses_session_course_new_uses_current_course() {
        let mut app = super::course_delete_tests::test_app(); // rust(1), csapp(2)
        app.course = "csapp".into();
        app.sidebar_sessions = vec![storage::SessionMeta {
            id: 42,
            title: Some("Rust 的旧会话".into()),
            course_id: Some(1), // 属于 rust
            created_at: "2026-09-02 10:00:00".into(),
        }];

        // Home Continue（cursor 1 = AI 入口后）→ 应切到 rust 并开其 session
        app.home_cursor = 1;
        app.home_activate();
        assert_eq!(app.course, "rust", "Continue 应恢复 session 所属课程");
        assert_eq!(app.workspace, crate::app::Workspace::Session);
        settle_full(&mut app).await;

        // 重新回到 Home + csapp：New Conversation 用当前课程
        app.course = "csapp".into();
        app.enter_course_workspace("csapp");
        assert_eq!(app.course_cursor_count(), 4, "csapp 无 session：4 个动作");
        app.course_activate(); // cursor 0 = New Conversation
        assert_eq!(
            app.course, "csapp",
            "New Conversation 不继承旧 session 课程"
        );
        assert_eq!(app.workspace, crate::app::Workspace::Session);
    }

    /// 文档 §38-8：删除当前课程 → Home 光标/当前课程同步，不残留已删课 entries。
    #[tokio::test]
    async fn delete_current_course_syncs_context() {
        let mut app = super::course_delete_tests::test_app(); // rust, csapp
        app.course = "rust".into();
        app.push_entry(Entry::User("rust 内容".into()));
        app.delete_course("rust");
        settle_full(&mut app).await;
        // 删除当前课程 → 回落 all 区
        assert_eq!(app.course, "all", "删除当前课程应回落 all");
        assert!(
            !app.courses.iter().any(|(_, n)| n == "rust"),
            "rust 应从列表移除"
        );
        assert_eq!(app.home_cursor, 0, "Home 光标应复位到 all 后的首项");
        assert!(
            !app.entries
                .iter()
                .any(|e| matches!(e, Entry::User(m) if m.contains("rust"))),
            "已删课的 entries 不应残留（分区已切走）"
        );
    }

    /// 文档 §十-1：Switch Course picker 只列真实课程，不出现 all。
    #[test]
    fn switch_course_picker_excludes_all() {
        let mut app = super::course_delete_tests::test_app(); // rust, csapp
        app.open_list_picker(crate::palette::PickKind::CourseSwitch);
        let Some(lp) = &app.list_picker else {
            panic!("应打开 CourseSwitch picker");
        };
        assert!(!lp.items.is_empty(), "应至少列出一门课");
        assert!(
            lp.items.iter().all(|c| c.label != "all"),
            "picker 不应含 all：{:?}",
            lp.items.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
        assert_eq!(lp.items.len(), 2, "只列真实课程 rust/csapp");
    }

    /// 文档 §十-2：Home 的 Your courses 只来自 courses 表真实实体。
    #[test]
    fn home_course_list_excludes_all() {
        let app = super::course_delete_tests::test_app();
        assert!(
            app.courses.iter().all(|(_, n)| n != "all"),
            "courses 表不应含 all 伪课程"
        );
    }

    /// 文档 §十：New Session 默认必须有真实 course_id——Global 下拒绝。
    #[test]
    fn new_session_requires_real_course() {
        let mut app = super::course_delete_tests::test_app();
        app.switch_course("all"); // 显式进入 Global scope
        app.start_new_session();
        assert!(
            app.entries
                .iter()
                .any(|e| matches!(e, Entry::Error(m) if m.contains("需要具体课程"))),
            "Global 下 New Session 应被拒绝并引导选课"
        );
    }

    /// 文档 §十：current course 不因 UI 操作变为 Global；Global 只能显式 /course all。
    #[tokio::test]
    async fn global_scope_is_explicit_only() {
        let mut app = super::course_delete_tests::test_app();
        // 进入课程：current course 是真实课程
        app.enter_course_workspace("csapp");
        settle_full(&mut app).await;
        assert_eq!(app.course, "csapp");
        assert!(app.current_course_id().is_some(), "真实课程有 id");
        // 显式 Global
        app.switch_course("all");
        settle_full(&mut app).await;
        assert_eq!(app.course, "all", "/course all 显式进入 Global");
        assert!(app.current_course_id().is_none(), "Global 无 course_id");
        assert_eq!(
            app.current_scope_label(),
            "Global",
            "UI 显示 Global 而非 all"
        );
    }

    /// 文档 §五：Continue Learning 对 Global session 显示 Global 标签，不伪装成课程。
    #[test]
    fn continue_learning_labels_global_session() {
        let mut app = super::course_delete_tests::test_app();
        app.sidebar_sessions = vec![storage::SessionMeta {
            id: 9,
            title: Some("你是谁".into()),
            course_id: None, // Global scope
            created_at: "2026-09-02 10:00:00".into(),
        }];
        let Some((_, course, title)) = app.continue_session() else {
            panic!("应有 Continue 目标");
        };
        assert_eq!(course, "Global", "Global session 标签应为 Global");
        assert_eq!(title, "你是谁", "不过滤 Session title");
    }

    /// 文档 §五：Global session 不覆盖真实课程 Continue 目标（优先最近真实课程 session）。
    #[test]
    fn global_session_does_not_override_real_course_continue() {
        let mut app = super::course_delete_tests::test_app();
        app.sidebar_sessions = vec![
            storage::SessionMeta {
                id: 11,
                title: Some("Global 最新会话".into()),
                course_id: None,
                created_at: "2026-09-03 10:00:00".into(),
            },
            storage::SessionMeta {
                id: 10,
                title: Some("Rust 学习".into()),
                course_id: Some(1),
                created_at: "2026-09-02 10:00:00".into(),
            },
        ];
        let Some((_, course, title)) = app.continue_session() else {
            panic!("应有 Continue 目标");
        };
        assert_eq!(course, "rust", "应优先真实课程 session");
        assert_eq!(title, "Rust 学习");
    }

    /// 文档 §四-6：Review 默认必须真实课程——Global 下走向导应被拒。
    #[test]
    fn review_defaults_to_real_course() {
        let mut app = super::course_delete_tests::test_app();
        app.switch_course("all");
        app.open_review_wizard();
        assert!(
            app.entries
                .iter()
                .any(|e| matches!(e, Entry::Error(m) if m.contains("需要具体课程"))),
            "Global 下 /review 无参应引导选课"
        );
        assert!(app.wizard.is_none(), "Global 下不应打开复习向导");
    }

    /// 统一命令入口为 Ctrl+K：Home/Course 中 Ctrl+K 打开命令面板，
    /// 普通键入字符不再打开面板（此前任意键都触发）。
    #[tokio::test]
    async fn ctrl_k_opens_palette_in_home_course() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

        fn key(code: KeyCode, ctrl: bool) -> KeyEvent {
            KeyEvent {
                code,
                modifiers: if ctrl {
                    KeyModifiers::CONTROL
                } else {
                    KeyModifiers::NONE
                },
                kind: KeyEventKind::Press,
                state: crossterm::event::KeyEventState::NONE,
            }
        }

        // Home：Ctrl+K 开面板
        let mut app = super::course_delete_tests::test_app();
        assert_eq!(app.workspace, crate::app::Workspace::Home);
        app.handle_key(key(KeyCode::Char('k'), true));
        assert!(app.palette.is_some(), "Home 中 Ctrl+K 应打开命令面板");

        // Home：普通键入不再开面板
        let mut app = super::course_delete_tests::test_app();
        app.handle_key(key(KeyCode::Char('r'), false));
        assert!(
            app.palette.is_none(),
            "Home 中普通键入不应再打开命令面板（命令入口统一 Ctrl+K）"
        );

        // Course：Ctrl+K 开面板
        let mut app = super::course_delete_tests::test_app();
        app.enter_course_workspace("rust");
        settle_full(&mut app).await;
        app.handle_key(key(KeyCode::Char('k'), true));
        assert!(app.palette.is_some(), "Course 中 Ctrl+K 应打开命令面板");
    }

    /// 首次运行 AI 待配置检测（文档 §九）：无明文/env key → 需要 Setup。
    #[test]
    fn ai_needs_setup_detects_missing_key() {
        let mut app = super::course_delete_tests::test_app();
        app.provider_cfg.api_key = None;
        app.provider_cfg.api_key_env = None;
        assert!(app.ai_needs_setup(), "无 key 应需要配置");

        app.provider_cfg.api_key = Some("sk-test".into());
        assert!(!app.ai_needs_setup(), "明文 key 视为已配置");

        // env 引用：已设置 → 已配置；未设置 → 需要配置
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();
        app.provider_cfg.api_key = None;
        app.provider_cfg.api_key_env = Some("STUDYPILOT_TEST_KEY".into());
        unsafe {
            std::env::set_var("STUDYPILOT_TEST_KEY", "sk-env");
        }
        assert!(!app.ai_needs_setup(), "env key 已设置视为已配置");
        unsafe {
            std::env::remove_var("STUDYPILOT_TEST_KEY");
        }
        assert!(app.ai_needs_setup(), "env key 未设置应需要配置");
    }
}
