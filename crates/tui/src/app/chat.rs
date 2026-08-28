//! 聊天任务接线：agent 循环、system prompt、usage/成本。

use std::sync::Arc;

use agent_core::{Agent, Provider, ToolBox, Usage};
use agent_providers::{ProviderConfig, estimate_cost};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;
use tools::{ListCoursesTool, SearchNotesTool};

use super::{AgentEvent, App, AppEvent, Entry};
use storage::Store;

impl App {
    pub(crate) fn spawn_chat(&mut self) {
        let provider: Arc<dyn Provider> = self.provider.clone();
        let store = Arc::clone(&self.store);
        let course_id = self.current_course_id();
        let history = self.history.clone();
        let tx = self.tx.clone();
        let token = CancellationToken::new();
        let token_for_task = token.clone();

        let system_prompt = self.build_rag_system_prompt();
        let tools = ToolBox::new()
            .register(Box::new(SearchNotesTool::new(
                Arc::clone(&store),
                course_id,
            )))
            .register(Box::new(ListCoursesTool::new(store)));

        let agent = Agent::new(provider)
            .with_tools(tools)
            .with_system_prompt(system_prompt);

        tokio::spawn(async move {
            let _ = tx.send(AppEvent::Agent(AgentEvent::Thinking));
            tokio::select! {
                res = agent.run_messages(&history) => match res {
                    Ok(result) => {
                        let _ = tx.send(AppEvent::Agent(AgentEvent::Done {
                            content: result.content,
                            reasoning_chars: result.reasoning_chars,
                            usage: result.total_usage,
                            new_messages: result.new_messages,
                            tool_trace: result.tool_trace,
                        }));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Agent(AgentEvent::Failed(e.to_string())));
                    }
                },
                _ = token_for_task.cancelled() => {
                    let _ = tx.send(AppEvent::Agent(AgentEvent::Interrupted));
                }
            }
        });

        self.inflight = Some(token);
    }

    /// 当前课程分区对应的 course_id（all = None = 不过滤）。
    /// 会话归属课程的显示名（course_id None = all 区）。
    pub(crate) fn course_label(&self, course_id: Option<i64>) -> String {
        match course_id {
            Some(id) => self
                .courses
                .iter()
                .find(|(cid, _)| *cid == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "all".into()),
            None => "all".into(),
        }
    }

    pub(crate) fn current_course_id(&self) -> Option<i64> {
        if self.course == "all" {
            None
        } else {
            self.courses
                .iter()
                .find(|(_, n)| *n == self.course)
                .map(|(id, _)| *id)
        }
    }
    /// RAG system prompt：AI Tutor 教学模式（非笔记搬运工）。
    pub(crate) fn build_rag_system_prompt(&self) -> String {
        let course_list: Vec<&str> = std::iter::once("all")
            .chain(self.courses.iter().map(|(_, n)| n.as_str()))
            .collect();
        format!(
            "你是一名课程学习导师。当前课程: {}。可切换: {}。\n\n\
             目标：帮助学生理解知识，而非总结笔记。\n\n\
             回答结构（严格遵循）：\n\
             ## 心智模型\n先建立一个直觉性的图景（如：变量--拥有-->数据）\n\n\
             ## 为什么需要\n问题背景：没有它会怎样？\n\n\
             ## 核心机制\n课程语境下的定义与关键规则\n\n\
             ## 示例\n一个最小代码例子，展示核心机制（选最有教学价值的，不要罗列）\n\n\
             ## 常见误区\n学生容易踩的坑\n\n\
             ## 相关知识\n关联概念，提示下一步学习方向\n\n\
             重要约束：\n\
             - 像老师一样重新讲解，绝不重复检索片段中的原句\n\
             - 覆盖 20% 的核心并讲清楚，胜过覆盖 100% 但像文档摘要\n\
             - 笔记优先：笔记中有相关内容时优先依据笔记讲解，并在关键事实后\
               标注一次 [n] 引用（[n] 只指向笔记片段）\n\
             - 笔记未覆盖的部分可以用你自己的知识补充，但必须在该部分末尾\
               明确标注「（笔记外补充）」，绝不能伪装成笔记内容\n\
             - 一个例子讲透核心思想，胜过五个浅例子\n\n\
             回答前调用一次 search_notes 检索相关笔记即可。",
            self.course,
            course_list.join(", ")
        )
    }

    pub(crate) fn handle_agent_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Thinking => {}
            AgentEvent::Done {
                content,
                reasoning_chars,
                usage,
                new_messages,
                tool_trace,
            } => {
                self.total_usage.prompt_tokens += usage.prompt_tokens;
                self.total_usage.completion_tokens += usage.completion_tokens;
                let delta = estimate_cost(&self.provider_cfg, &usage);
                self.total_cost += delta;
                self.session_cost += delta;
                for m in &new_messages {
                    self.history.push(m.clone());
                    self.persist(m.clone());
                }
                if !tool_trace.is_empty() {
                    let total_hits: usize = tool_trace.iter().map(|t| t.hit_count).sum();
                    let lines: Vec<String> = tool_trace
                        .iter()
                        .map(|t| {
                            format!(
                                "  {} {} → {} 条",
                                if t.ok { "✓" } else { "✗" },
                                t.tool_name,
                                t.hit_count
                            )
                        })
                        .collect();
                    self.push_entry(Entry::Info(format!(
                        "Agent trace ({total_hits} 条命中):\n{}",
                        lines.join("\n")
                    )));
                }
                self.push_entry(Entry::Assistant {
                    content,
                    reasoning_chars,
                });
                self.inflight = None;
                self.record_usage(usage);
            }
            AgentEvent::Failed(e) => {
                self.push_entry(Entry::Error(format!("请求失败: {e}")));
                self.inflight = None;
            }
            AgentEvent::Interrupted => {
                self.push_entry(Entry::Info("[已中断当前请求]".into()));
                self.inflight = None;
            }
        }
    }

    // ---- 会话持久化（R5）----

    /// 从 usage_log 对账累计成本（R6）：review/outline/import/grade 的花费
    /// 不经过 chat 的 Done 事件路径，靠读取 DB 补齐熔断口径。
    /// 取 max 防止对账时该笔 usage 尚未落库导致回退（成本单调递增）。
    pub(crate) fn request_cost_sync(&self) {
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let total = spawn_blocking(move || store.total_recorded_cost())
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            if let Ok(v) = total {
                let _ = tx.send(AppEvent::CostSynced(v));
            }
        });
    }

    /// 后台任务里的 usage 落库统一壳（chat 以外各 LLM 命令共用）。
    pub(crate) fn log_llm_usage(store: &Arc<Store>, pc: &ProviderConfig, kind: &str, u: &Usage) {
        let store = Arc::clone(store);
        let pc = pc.clone();
        let kind = kind.to_owned();
        let (prompt, completion) = (u.prompt_tokens, u.completion_tokens);
        let kind_ref = kind.to_owned();
        tokio::spawn(async move {
            let cost = estimate_cost(
                &pc,
                &Usage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                },
            );
            let result = spawn_blocking(move || {
                store.append_usage(&pc.name, &pc.model, &kind, prompt, completion, cost)
            })
            .await;
            // kind 在闭包内被消耗，日志改用模型名定位
            if let Err(e) = result {
                tracing::warn!("{kind_ref} usage_log 写入失败: {e}");
            }
        });
    }

    pub(crate) fn record_usage(&self, usage: Usage) {
        let pc = self.provider_cfg.clone();
        let cost = estimate_cost(&pc, &usage);
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            let result = spawn_blocking(move || {
                store.append_usage(
                    &pc.name,
                    &pc.model,
                    "chat",
                    usage.prompt_tokens,
                    usage.completion_tokens,
                    cost,
                )
            })
            .await;
            if let Err(e) = result {
                tracing::warn!("usage_log 写入失败: {e}");
            }
        });
    }
}
