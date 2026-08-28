//! 斜杠命令分发与各命令 handler（D6：命令直连，不进 agent loop）。

use std::sync::Arc;

use agent_core::Message;
use agent_providers::OpenAiClient;
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, CourseOpOutcome, Entry, ModelPicker};
use crate::course_cmd::{CourseAction, parse_course_action, parse_review_action};
use crate::review;
use storage::Store;

impl App {
    /// 用户按 Enter 提交。斜杠命令随时可执行；
    /// 普通消息请求中忽略新提交（串行，避免费用与状态混乱）。
    pub(crate) fn submit(&mut self) {
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
        let arg = parts.collect::<Vec<_>>().join(" ");

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
                    "我能做什么？",
                    "",
                    "【学习】直接打字提问即可，无需命令",
                    "  例: \"解释一下虚拟内存\"  \"这里为什么必须用 mutex\"",
                    "  回答策略: 笔记优先并标注 [n] 引用；笔记没写的会用",
                    "  自身知识补充并明示「（笔记外补充）」",
                    "",
                    "【知识库】",
                    "  /import <目录> [--course 名]   批量导入 md/pdf/pptx",
                    "  /notes                        列出当前分区笔记",
                    "  /delete <id> | /move <id> <课程>  管理笔记（不碰磁盘原文件）",
                    "  /course <课程|all>            切换分区；-new/-delete 管理",
                    "",
                    "【复习】",
                    "  /review <课程> [概念] [--n 数量]  出题（选择+简答，掌握度低优先）",
                    "",
                    "【大纲】",
                    "  /outline [课程] [--export]    生成课程知识大纲",
                    "",
                    "【系统】",
                    "  /model 切换模型 · /budget 预算 · /new /sessions /open /rename 会话",
                    "  /export /load 会话导出导入 · /course -new/-delete 分区管理",
                    "",
                    "快捷键: Ctrl+K 查看全部命令（支持过滤与参数向导）",
                    "        v 选择模式 · Ctrl+C 中断/退出 · Ctrl+Q 强退",
                ] {
                    self.push_entry(Entry::Info(l.into()));
                }
            }
            "/course" => self.handle_course_command(&arg),
            "/model" => self.handle_model_command(arg.trim()),
            "/new" => self.start_new_session(),
            "/sessions" => {
                // 不直接打印缓存的旧列表：等 SessionsLoaded 事件回来再展示最新数据
                self.pending_sessions_print = true;
                self.request_sessions_refresh();
            }
            "/open" => match arg.trim().parse::<i64>() {
                Ok(id) => self.open_session(id),
                Err(_) => self.push_entry(Entry::Error("用法: /open <会话 id>".into())),
            },
            "/export" => self.export_session(),
            "/rename" => self.rename_session(arg.trim()),
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
            "/review" => self.handle_review_command(arg.trim()),
            _ => {
                self.push_entry(Entry::Error(format!(
                    "未知命令 `{cmd}`，输入 /help 查看可用命令"
                )));
            }
        }
    }

    pub(crate) fn not_implemented(&mut self, cmd: &str, milestone: &str) {
        self.push_entry(Entry::Error(format!(
            "{cmd} 尚未实现（计划于 {milestone}）"
        )));
    }

    // ---- 预算管理 ----

    /// `/budget`：查/改/重置预算。
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

        // 设置新上限
        match arg.parse::<f64>() {
            Ok(val) if val > 0.0 => {
                self.max_cost = val;
                self.push_entry(Entry::Info(format!("预算上限已设为 ¥{val:.2}")));
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
        if self.review.is_some() {
            self.push_entry(Entry::Error("复习进行中，请先完成或 Esc 退出".into()));
            return;
        }

        let known: Vec<String> = self.courses.iter().map(|(_, n)| n.clone()).collect();
        let spec = match parse_review_action(arg, &known) {
            Ok(s) => s,
            Err(e) => {
                self.push_entry(Entry::Error(e));
                return;
            }
        };
        let course_name = spec.course;
        if course_name == "all" {
            self.push_entry(Entry::Error("复习需指定具体课程，不能为 all".into()));
            return;
        }

        let Some(course_id) = self
            .courses
            .iter()
            .find(|(_, n)| *n == course_name.as_str())
            .map(|(id, _)| *id)
        else {
            self.push_entry(Entry::Error(format!("课程 `{course_name}` 不存在")));
            return;
        };

        let n = spec.n;
        let scope = spec.scope;

        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();

        self.push_entry(Entry::Info(format!(
            "开始出题: 《{course_name}》范围「{scope}」，{n} 题"
        )));

        tokio::spawn(async move {
            review::start_review(
                store,
                provider,
                provider_cfg,
                course_id,
                course_name,
                scope,
                n,
                tx,
                CancellationToken::new(),
            )
            .await;
        });
    }

    /// `/notes`：列出当前课程（或 all）的笔记摘要。
    pub(crate) fn handle_notes_command(&mut self) {
        let cid = if self.course == "all" {
            None
        } else {
            self.courses
                .iter()
                .find(|(_, n)| *n == self.course)
                .map(|(id, _)| *id)
        };
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result =
                spawn_blocking(move || store.list_notes(cid, 50).map_err(|e| e.to_string()))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
            let _ = tx.send(AppEvent::NotesListed(result));
        });
    }

    /// `/delete <id>` 或 `/delete --course <name>`
    pub(crate) fn handle_delete_command(&mut self, arg: &str) {
        if arg.is_empty() {
            self.push_entry(Entry::Error(
                "用法: /delete <笔记 id> 或 /delete --course <课程名>".into(),
            ));
            return;
        }

        if let Some(course_name) = arg
            .strip_prefix("--course ")
            .or_else(|| arg.strip_prefix("--course"))
        {
            let name = course_name.trim();
            if name.is_empty() || name == "all" {
                self.push_entry(Entry::Error("--course 不能为空或 all".into()));
                return;
            }
            let course_id = self
                .courses
                .iter()
                .find(|(_, n)| *n == name)
                .map(|(id, _)| *id);
            let Some(cid) = course_id else {
                self.push_entry(Entry::Error(format!("课程 `{name}` 不存在")));
                return;
            };
            let store = Arc::clone(&self.store);
            let tx = self.tx.clone();
            let name = name.to_owned();
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
            return;
        }

        // 单条删除
        match arg.parse::<i64>() {
            Ok(id) => {
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
            Err(_) => {
                self.push_entry(Entry::Error(format!(
                    "无法解析 id `{arg}`。用法: /delete <笔记 id>"
                )));
            }
        }
    }

    /// `/move <id> <课程名>`
    pub(crate) fn handle_move_command(&mut self, arg: &str) {
        let parts: Vec<&str> = arg.split_whitespace().collect();
        if parts.len() < 2 {
            self.push_entry(Entry::Error("用法: /move <笔记 id> <目标课程名>".into()));
            return;
        }
        let id = parts[0];
        let target = parts[1..].join(" ");
        let Ok(id) = id.parse::<i64>() else {
            self.push_entry(Entry::Error(format!("无法解析 id `{id}`")));
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

    /// `/model [名称]`：无参列出全部 provider（当前打标），带参运行时切换。
    /// `/model`：无参弹窗选择；`<名>` 直接切换。
    pub(crate) fn handle_model_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let selected = self
                .all_providers
                .iter()
                .position(|p| p.name == self.provider_cfg.name)
                .unwrap_or(0);
            self.model_picker = Some(ModelPicker {
                options: self.all_providers.clone(),
                selected,
            });
            return;
        }
        self.apply_provider_switch(arg);
    }

    /// 执行 provider 切换：解析 key、构建客户端、替换当前 provider。
    pub(crate) fn apply_provider_switch(&mut self, name: &str) {
        let Some(cfg) = self.all_providers.iter().find(|p| p.name == name).cloned() else {
            let names: Vec<&str> = self.all_providers.iter().map(|p| p.name.as_str()).collect();
            self.push_entry(Entry::Error(format!(
                "未知 provider `{name}`。可用: {}",
                names.join(", ")
            )));
            return;
        };
        match OpenAiClient::new(cfg.clone()) {
            Ok(client) => {
                self.provider = Arc::new(client);
                let old = self.provider_cfg.name.clone();
                self.provider_cfg = cfg;
                self.push_entry(Entry::Info(format!(
                    "已从 `{old}` 切换到 `{} │ {}`（思考模式: {}）",
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
            CourseAction::Switch(name) => {
                self.course = name.clone();
                self.push_entry(Entry::Info(format!("已切换到课程: {name}")));
            }
            CourseAction::Create(name) => {
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
            CourseAction::Delete(name) => {
                let is_current = self.course == name;
                self.run_course_op(move |store| {
                    let deleted = store.delete_course(&name).map_err(|e| e.to_string())?;
                    if !deleted {
                        return Err(format!("课程 `{name}` 不存在"));
                    }
                    let list = store.list_courses().map_err(|e| e.to_string())?;
                    // 笔记/概念的 course_id 由外键 ON DELETE SET NULL 回落 all 区，数据不丢
                    let msg = format!("已删除课程: {name}（其笔记已回落 all 区）");
                    let switch_to = is_current.then(|| "all".to_owned());
                    Ok((msg, list, switch_to))
                });
            }
            CourseAction::SwitchById(id) => match self.courses.iter().find(|(i, _)| *i == id) {
                Some((_, name)) => {
                    self.course = name.clone();
                    self.push_entry(Entry::Info(format!("已切换到课程: {name}")));
                }
                None => self.push_entry(Entry::Error(format!("课程 #{id} 不存在（可能已删除）"))),
            },
            CourseAction::DeleteById(id) => {
                let Some((_, name)) = self.courses.iter().find(|(i, _)| *i == id).cloned() else {
                    self.push_entry(Entry::Error(format!("课程 #{id} 不存在（可能已删除）")));
                    return;
                };
                let is_current = self.course == name;
                let name2 = name.clone();
                self.run_course_op(move |store| {
                    let deleted = store.delete_course_by_id(id).map_err(|e| e.to_string())?;
                    if !deleted {
                        return Err(format!("课程 #{id} 不存在"));
                    }
                    let list = store.list_courses().map_err(|e| e.to_string())?;
                    let switch_to = is_current.then(|| "all".to_owned());
                    Ok((
                        format!("已删除课程: {name2}（其笔记已回落 all 区）"),
                        list,
                        switch_to,
                    ))
                });
            }
            CourseAction::Invalid(msg) => self.push_entry(Entry::Error(msg)),
        }
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
                if let Some(c) = switch_to {
                    self.course = c;
                }
                self.push_entry(Entry::Info(msg));
            }
            Err(e) => self.push_entry(Entry::Error(format!("课程操作失败: {e}"))),
        }
    }
}

#[cfg(test)]
mod course_delete_tests {
    use super::*;
    use crate::app::AppEvent;

    fn test_app() -> App {
        let cfg = agent_providers::ProviderConfig {
            name: "test".into(),
            endpoint: "http://localhost".into(),
            api_key: Some("k".into()),
            api_key_env: None,
            model: "m".into(),
            price_prompt: 0.0,
            price_completion: 0.0,
            context_length: 1000,
            thinking: false,
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
