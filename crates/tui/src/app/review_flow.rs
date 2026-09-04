//! 复习流程状态推进：判分回流、题面渲染、掌握度闭环。

use std::sync::Arc;

use agent_core::{Message, Provider};
use serde::Deserialize;
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;
use tools::SearchNotesTool;

use super::{App, AppEvent, Entry};
use crate::review;
impl App {
    /// 启动 Flashcard Warm-up（正式 Review 前置）：生成卡片期间挂 inflight，
    /// 事件 WarmupReady 回流后进入暖场覆盖层。范围 = 本次复习选中的概念/section。
    /// `n` = 正式 Review 题数（调用方显式传入；warm-up 卡数由 LLM 自主 3~8）。
    pub(crate) fn start_warmup(
        &mut self,
        scope_text: String,
        course_id: Option<i64>,
        course_name: String,
        n: usize,
    ) {
        if self.review.is_some() {
            self.push_entry(Entry::Error("复习进行中，请先完成或 Esc 退出".into()));
            return;
        }
        // 预算熔断（R6）
        if self.total_cost >= self.max_cost {
            self.push_entry(Entry::Error(format!(
                "已达预算上限 ¥{:.2}（累计 ¥{:.4}），拒绝出题。可用 /budget 调高上限",
                self.max_cost, self.total_cost
            )));
            return;
        }
        let store = Arc::clone(&self.store);
        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let tx = self.tx.clone();
        let cancel = CancellationToken::new();
        self.inflight = Some(cancel.clone());
        // 先建空暖场状态（卡片到达后填充）：覆盖层即时出现（"生成中…"），
        // scope/course/n 先记录，供 finish_warmup 传给正式 Review。
        self.warmup = Some(review::WarmupState {
            cards: Vec::new(),
            current: 0,
            revealed: false,
            ratings: Vec::new(),
            scope_text: scope_text.clone(),
            course_id,
            course_name: course_name.clone(),
            n,
        });
        self.push_entry(Entry::Info(format!(
            "Flashcard Warm-up · 范围「{scope_text}」生成中…"
        )));
        tokio::spawn(async move {
            let result = review::generate_warmup_cards(
                store,
                provider,
                provider_cfg,
                course_id,
                course_name,
                scope_text,
                cancel,
            )
            .await;
            let _ = tx.send(AppEvent::WarmupReady(result));
        });
    }

    /// Warm-up 卡片回流：填充暖场状态（覆盖层渲染第一张）。
    pub(crate) fn on_warmup_ready(&mut self, result: Result<Vec<review::Flashcard>, String>) {
        self.inflight = None;
        self.request_cost_sync();
        match result {
            Ok(cards) => {
                let n_cards = cards.len();
                if let Some(w) = &mut self.warmup {
                    w.cards = cards;
                    w.ratings = vec![None; n_cards];
                    w.current = 0;
                    w.revealed = false;
                }
                self.push_entry(Entry::Info(format!(
                    "Warm-up: {n_cards} 张卡片 · Space 翻开 · 1/2/3 自评 · Enter 下一张"
                )));
            }
            Err(e) => {
                self.warmup = None;
                self.push_entry(Entry::Error(format!("Flashcard 生成失败: {e}")));
            }
        }
    }

    /// 暖场完成：计算 Focus Concepts（△/○）→ 传给正式 Review（复用现有引擎）。
    /// Flashcard 绝不写 mastery（不调 update_concept_mastery）。
    pub(crate) fn finish_warmup(&mut self) {
        let Some(w) = self.warmup.take() else { return };
        let focus = w.focus_concepts();
        let scope: Option<String> = if focus.is_empty() {
            // 全部 Got it：回到原始范围正常复习
            Some(w.scope_text.clone())
        } else {
            Some(focus.join("、"))
        };
        let mut focus_msg = String::from("Warm-up complete\n\nFocus:");
        if focus.is_empty() {
            focus_msg.push_str(" 全部 ✓ 通过");
        } else {
            for c in &focus {
                focus_msg.push_str(&format!("\n△ {c}"));
            }
        }
        self.push_entry(Entry::Markdown(focus_msg));
        self.push_entry(Entry::Info("Starting review…".into()));
        self.run_review(w.course_id, w.course_name, scope, w.n);
    }

    /// 完成一道题：记录 attempt + 更新掌握度 + 进入反馈停留态。
    /// （Workspace 渲染直接读 ReviewState；不再向聊天流逐题输出）
    pub(crate) fn finish_review_question(
        &mut self,
        correct: bool,
        score: Option<i64>,
        feedback: &str,
        missing: &[String],
        user_choice: Option<usize>,
    ) {
        // ① 提取题目数据（不可变借用结束后再操作 self）
        let (db_id, concept_id) = match &self.review {
            Some(rs) => match rs.questions.get(rs.current) {
                Some(q) => (q.db_id, q.concept_id),
                None => return,
            },
            None => return,
        };

        // ② 记录 attempt + 更新掌握度（spawn_blocking，D2）；失败上抛可见
        let store = Arc::clone(&self.store);
        let answer = feedback.to_owned();
        let missing_json = if missing.is_empty() {
            None
        } else {
            serde_json::to_string(missing).ok()
        };
        let is_correct = correct;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result: Result<(), String> = spawn_blocking(move || -> Result<(), String> {
                store
                    .record_attempt(db_id, &answer, score, missing_json.as_deref())
                    .map_err(|e| format!("attempt 写入失败: {e}"))?;
                if let Some(cid) = concept_id {
                    store
                        .update_concept_mastery(cid, is_correct)
                        .map_err(|e| format!("掌握度更新失败: {e}"))?;
                }
                Ok(())
            })
            .await
            .unwrap_or_else(|e| Err(format!("任务错误: {e}")));
            if let Err(msg) = result {
                let _ = tx.send(AppEvent::DatabaseFailed(msg));
            }
        });

        // ③ 记录结果 → 进入反馈停留态（渲染由 workspace 依 results.len()>current 判定）
        if let Some(rs) = &mut self.review {
            rs.results.push(review::ReviewResult {
                correct,
                score,
                feedback: feedback.to_owned(),
                missing: missing.to_vec(),
                user_choice,
            });
            rs.selected_option = None;
        }
    }

    /// 反馈停留态按 Enter：推进到下一题或完成复习（Summary 卡回聊天流）。
    /// 逐题生成：推进后若下一题尚未生成到位，进入等待态（workspace 渲染"出题中"），
    /// 题目到达（ReviewQuestionReady）后自动显示。
    pub(crate) fn advance_review(&mut self) {
        let Some(rs) = &mut self.review else { return };
        rs.current += 1;
        rs.selected_option = None;
        rs.followups.clear();
        if rs.current >= rs.planned {
            self.finish_review();
        }
        // current < planned 但 questions.len() == current：等待态，不 finish
    }

    /// 反馈停留态提交追问：问题即时显示（思考行占位），后台 agent loop（带
    /// search_notes 工具）回答后填充。追问不改判分、不落库；失败放红色可见。
    pub(crate) fn submit_followup(&mut self) {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let Some(rs) = &self.review else { return };
        let Some(q) = rs.questions.get(rs.current) else {
            return;
        };
        let idx = rs.current;
        let result = rs.results.get(rs.current);
        // 题目上下文快照（借用结束后再 spawn）
        let question_text = q.question.clone();
        let options = q.options.clone();
        let answer_letter = q.answer.map(|a| (b'A' + a as u8) as char);
        let user_letter = result
            .and_then(|r| r.user_choice)
            .map(|c| (b'A' + c as u8) as char);
        let is_correct = result.map(|r| r.correct).unwrap_or(false);
        let explanation = q.explanation.clone();
        let missing = result.map(|r| r.missing.clone()).unwrap_or_default();
        // 追问历史只喂成功轮（Option 化后过滤 pending/失败轮）
        let history: Vec<(String, String)> = rs
            .followups
            .iter()
            .filter_map(|t| {
                t.answer
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .map(|a| (t.question.clone(), a.clone()))
            })
            .collect();
        let course_id = rs.ctx.course_id;

        self.input.clear();
        self.cursor_pos = 0;
        self.scroll_up = 0; // 新内容（追问行 + 思考行）吸附底部
        let cancel = tokio_util::sync::CancellationToken::new();
        self.followup_pending = Some(cancel.clone());
        // 问题即时显示 + 思考行占位（问题 5：不再等回答一起出现）
        if let Some(rs) = &mut self.review {
            rs.followups.push(review::FollowupTurn {
                question: text.clone(),
                answer: None,
                citations: Vec::new(),
            });
        }

        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let store = Arc::clone(&self.store);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut ctx = format!("题目：{question_text}\n");
            for (i, o) in options.iter().enumerate() {
                ctx.push_str(&format!("  {}. {o}\n", (b'A' + i as u8) as char));
            }
            if let Some(a) = answer_letter {
                ctx.push_str(&format!("正确答案：{a}\n"));
            }
            if let Some(u) = user_letter {
                ctx.push_str(&format!("学生选择：{u}\n"));
            }
            ctx.push_str(if is_correct {
                "本题判定：答对\n"
            } else {
                "本题判定：答错\n"
            });
            if let Some(e) = &explanation {
                ctx.push_str(&format!("解析：{e}\n"));
            }
            if !missing.is_empty() {
                ctx.push_str(&format!("缺失要点：{}\n", missing.join("；")));
            }
            for (fq, fa) in &history {
                ctx.push_str(&format!("\n学生此前追问：{fq}\n导师此前回答：{fa}\n"));
            }
            let user_msg = format!(
                "{ctx}\n学生追问：{text}\n\n\
                 （回答前先用 search_notes 查笔记是否覆盖该知识点：\
                 查到的内容按笔记回答并标明出处；笔记没覆盖的部分用你的知识补充并明示「笔记外补充」。）"
            );
            let agent = agent_core::Agent::new(provider)
                .with_tools(
                    agent_core::ToolBox::new().register(Box::new(SearchNotesTool::new(
                        Arc::clone(&store),
                        course_id,
                    ))),
                )
                .with_system_prompt(
                    "你是课程学习导师，学生刚复习完一道题后来追问。\
                     回答要简洁准确、切中学生疑问，用 markdown（代码用围栏标注语言）。不要复述整道题。\
                     涉及「笔记里有没有讲」的问题必须先查笔记（search_notes），不许凭空判断。",
                );
            let history = [Message::user(&user_msg)];
            tokio::select! {
                res = agent.run_messages(&history) => match res {
                    Ok(result) => {
                        App::log_llm_usage(&store, &provider_cfg, "review", &result.total_usage);
                        // search_notes 工具结果 → [n] → 笔记来源映射（渲染脚注）
                        let citations = super::extract_citations(
                            &result.new_messages,
                            &result.content,
                        );
                        let _ = tx.send(AppEvent::ReviewFollowup(
                            idx,
                            text,
                            Ok(result.content),
                            citations,
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::ReviewFollowup(
                            idx,
                            text,
                            Err(e.to_string()),
                            Vec::new(),
                        ));
                    }
                },
                _ = cancel.cancelled() => {
                    // Esc 中断：填充占位行为中断标记（workspace 红色可见）
                    let _ = tx.send(AppEvent::ReviewFollowup(
                        idx,
                        text,
                        Err("（已中断）".into()),
                        Vec::new(),
                    ));
                }
            }
        });
    }

    /// 追问回答回流：校验索引 → 填充当前题的追问对话（workspace 渲染）。
    pub(crate) fn on_review_followup(
        &mut self,
        idx: usize,
        question: String,
        result: Result<String, String>,
        citations: Vec<(usize, String)>,
    ) {
        self.followup_pending = None;
        self.scroll_up = 0; // 回答到达吸附底部
        let idx_ok = self
            .review
            .as_ref()
            .map(|rs| rs.current == idx)
            .unwrap_or(false);
        if !idx_ok {
            tracing::warn!(idx, "过期追问回答被丢弃");
            return;
        }
        let Some(rs) = &mut self.review else { return };
        // 填充最后一条 pending 的追问（按问句匹配，防错位）
        if let Some(turn) = rs.followups.last_mut()
            && turn.answer.is_none()
            && turn.question == question
        {
            turn.answer = Some(result);
            turn.citations = citations;
        }
    }

    /// 完成整个复习：从状态渲染摘要卡。`advance_review` 到尾题时调用。
    pub(crate) fn finish_review(&mut self) {
        let Some(rs) = self.review.take() else { return };
        self.finish_review_state(rs);
    }

    /// 渲染复习摘要卡（markdown 富文本）+ 触发异步 LLM 小结建议。按状态渲染，供完成/中途退出共用。
    fn finish_review_state(&mut self, rs: review::ReviewState) {
        let total = rs.questions.len();
        let correct_count = rs.results.iter().filter(|r| r.correct).count();
        let rate = if total > 0 {
            correct_count as f64 * 100.0 / total as f64
        } else {
            0.0
        };
        let mut md =
            format!("## 复习完成\n\n**正确 {correct_count}/{total}** · 正确率 {rate:.0}%\n");
        for (i, (q, r)) in rs.questions.iter().zip(rs.results.iter()).enumerate() {
            let mark = if r.correct { "✓" } else { "✗" };
            let score = r.score.map(|s| format!("（{s}/100）")).unwrap_or_default();
            // 摘要 = 考点（概念名），fallback 剥围栏题面——截断题面既不完整又误导
            let topic = q.concept_name.clone().unwrap_or_else(|| {
                q.question
                    .replace("```rust", "")
                    .replace("```c", "")
                    .replace("```", "")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(40)
                    .collect()
            });
            // 每题独立段落：单 \n 在 markdown 管线是软换行不折行，必须 \n\n
            md.push_str(&format!("\n\n{mark} **Q{}** 【{topic}】{score}", i + 1));
            if !r.missing.is_empty() {
                md.push_str(&format!("\n\n   缺失: {}", r.missing.join("；")));
            }
        }
        self.push_entry(Entry::Markdown(md));
        self.push_entry(Entry::Info(
            "· 概念状态已更新，/outline 查看最新复习地图".into(),
        ));

        // 异步 LLM 小结建议（Observe 整轮结果 → Decide 下一步；不阻塞已显示的静态卡）
        // Agent Trace：START（完成/失败行由 on_review_advice / 失败路径追加）
        self.push_entry(Entry::Tool {
            text: "[整理] 正在生成本轮复习小结…".into(),
            ok: None,
        });
        self.spawn_review_advice(&rs);
    }

    /// 整轮复习结束后：把每题结果交给 LLM，生成「已掌握/需巩固/下一步建议」。
    /// 失败静默（小结卡已给出足够信息，不让推荐成为新的失败点）。
    fn spawn_review_advice(&mut self, rs: &review::ReviewState) {
        let store = Arc::clone(&self.store);
        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let tx = self.tx.clone();
        let course = self.course.clone();
        let ids: Vec<i64> = rs.questions.iter().filter_map(|q| q.concept_id).collect();
        let rows: Vec<QuestionSnapshot> = rs
            .questions
            .iter()
            .zip(rs.results.iter())
            .map(|(q, r)| {
                let short = q.question.chars().take(60).collect::<String>();
                (q.concept_id, short, r.correct, r.score, r.missing.clone())
            })
            .collect();

        tokio::spawn(async move {
            let store_for_names = Arc::clone(&store);
            let names = spawn_blocking(move || store_for_names.concept_names(&ids))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
            let mut per_question = String::new();
            for (i, (cid, q, ok, score, missing)) in rows.iter().enumerate() {
                let cname = cid
                    .and_then(|id| names.get(&id))
                    .map(String::as_str)
                    .unwrap_or("?");
                let mark = if *ok { "✓" } else { "✗" };
                let score_txt = score.map(|s| format!("（{s}/100）")).unwrap_or_default();
                per_question.push_str(&format!(
                    "{mark} 概念「{cname}」｜Q{i} {q}{score_txt}\n",
                    i = i + 1
                ));
                for m in missing {
                    per_question.push_str(&format!("    缺失: {m}\n"));
                }
            }
            let prompt = format!(
                "你是《{course}》课程的学习导师。学生刚做完一轮复习，结果如下：\n{per_question}\n\n\
                 请输出 JSON：\n\
                 {{\"mastered\": [\"概念名\"], \"consolidate\": [\"概念名\"], \"next\": \"下一步学习建议\"}}\n\n\
                 要求：\n\
                 - mastered：这轮答对、已基本掌握的概念\n\
                 - consolidate：答错或缺失要点的概念\n\
                 - next：针对 consolidate 给出具体下一步（先补哪个概念、读哪类笔记、做什么练习），1~3 句，不要空话"
            );
            let messages = [
                Message::system("只输出 JSON 本体。"),
                Message::user(&prompt),
            ];
            let pc = provider_cfg.clone();
            let store_usage = Arc::clone(&store);
            let content = match provider.chat_json(&messages).await {
                Ok(resp) => {
                    App::log_llm_usage(&store_usage, &pc, "review", &resp.usage);
                    Some(resp.content)
                }
                Err(e) if e.is_json_mode_unsupported() => provider
                    .chat(&messages, &[])
                    .await
                    .map(|resp| {
                        App::log_llm_usage(&store_usage, &pc, "review", &resp.usage);
                        resp.content
                    })
                    .ok(),
                Err(e) => {
                    tracing::warn!("复习小结生成失败: {e}");
                    let _ = tx.send(AppEvent::ReviewAdvice(format!(
                        "⚠ 小结生成失败（已跳过）: {e}"
                    )));
                    None
                }
            };
            let Some(json) = content else {
                let _ = tx.send(AppEvent::ReviewAdvice(
                    "⚠ 小结生成失败（已跳过）：模型无响应".into(),
                ));
                return;
            };
            let advice = serde_json::from_str::<ReviewAdvice>(json.trim())
                .ok()
                .or_else(|| {
                    agent_core::first_json_block(&json).and_then(|b| serde_json::from_str(b).ok())
                });
            let Some(a) = advice else {
                tracing::warn!("复习小结 JSON 解析失败");
                let _ = tx.send(AppEvent::ReviewAdvice(
                    "⚠ 小结生成失败（已跳过）：输出解析失败".into(),
                ));
                return;
            };
            let mut out = String::from("── 复习小结 ──");
            if !a.mastered.is_empty() {
                out.push_str(&format!("\n✓ 已掌握: {}", a.mastered.join("、")));
            }
            if !a.consolidate.is_empty() {
                out.push_str(&format!("\n△ 需巩固: {}", a.consolidate.join("、")));
            }
            if let Some(n) = &a.next
                && !n.trim().is_empty()
            {
                out.push_str(&format!("\n→ 下一步: {n}"));
            }
            let _ = tx.send(AppEvent::ReviewAdvice(out));
        });
    }

    pub(crate) fn on_review_advice(&mut self, text: String) {
        // Agent Trace：完成（START 行已在 finish_review_state）
        self.push_entry(Entry::Tool {
            text: "✓ [整理] 复习小结已生成".into(),
            ok: Some(true),
        });
        // 小结建议分组着色（✓绿/△黄/→蓝），解析失败静默降级为 Markdown 块
        let parse = |line: &str| line.trim_start_matches("- ").to_owned();
        let (mut mastered, mut consolidate, mut next) = (Vec::new(), Vec::new(), None);
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("✓ 已掌握: ") {
                mastered = v
                    .split('、')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(parse)
                    .collect();
            } else if let Some(v) = line.strip_prefix("△ 需巩固: ") {
                consolidate = v
                    .split('、')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(parse)
                    .collect();
            } else if let Some(v) = line.strip_prefix("→ 下一步: ") {
                next = Some(v.to_owned());
            }
        }
        if mastered.is_empty() && consolidate.is_empty() && next.is_none() {
            self.push_entry(Entry::Markdown(text));
            return;
        }
        self.push_entry(Entry::Advice {
            mastered,
            consolidate,
            next,
        });
    }

    pub(crate) fn exit_review(&mut self, msg: &str) {
        // 取消在途的逐题生成（迟到事件由 on_review_question_ready 的 None 守卫丢弃）
        if let Some(t) = self.review_gen.take() {
            t.cancel();
        }
        // 中途退出：若有进度，同样出部分摘要卡（finish_review_state 不依赖 self.review）
        if let Some(rs) = self.review.take()
            && !rs.results.is_empty()
        {
            self.finish_review_state(rs);
        }
        self.push_entry(Entry::Info(msg.to_owned()));
    }
}

impl App {
    /// 出题完成（或失败）：清 inflight + 进 Review workspace 渲染。
    pub(crate) fn on_review_ready(&mut self, result: Result<review::ReviewState, String>) {
        self.inflight = None; // 出题/取消完成
        self.request_cost_sync();
        match result {
            Ok(rs) => {
                let planned = rs.planned;
                self.review = Some(rs);
                self.scroll_up = 0; // 进 workspace 跟随底部
                self.push_entry(Entry::Info(format!(
                    "· Review started · {planned} questions（逐题生成，答当前题时后台预取下一题）"
                )));
                // 首题已显示：立即预取第 2 题
                self.maybe_spawn_next_question();
            }
            Err(e) => {
                if e.contains(crate::app::cards::EMPTY_COURSE_MARKER) {
                    self.push_entry(Entry::Markdown(crate::app::cards::empty_review()));
                } else {
                    self.push_entry(Entry::Error(format!("出题失败: {e}")));
                }
            }
        }
    }

    /// 预取下一题（守卫：未在生成中、未达计划数、**预算未超限**）。
    /// 供首题显示后 / 每题到达后调用。
    pub(crate) fn maybe_spawn_next_question(&mut self) {
        // 预算熔断（R6）：逐题生成是 review 最大开销源，spawn 前检查
        if self.total_cost >= self.max_cost {
            if let Some(rs) = self.review.take()
                && !rs.results.is_empty()
            {
                self.finish_review_state(rs);
            }
            self.push_entry(Entry::Error(format!(
                "已达预算上限 ¥{:.2}（累计 ¥{:.4}），出题中止。可用 /budget 调高上限",
                self.max_cost, self.total_cost
            )));
            return;
        }
        let (index, planned, quiz_id, ctx, qtype, asked) = {
            let Some(rs) = &self.review else { return };
            use crate::review::QType;
            if rs.next_pending || rs.questions.len() >= rs.planned {
                return;
            }
            let done_choice = rs
                .questions
                .iter()
                .filter(|q| q.q_type == QType::Choice)
                .count();
            let done_short = rs
                .questions
                .iter()
                .filter(|q| q.q_type == QType::ShortAnswer)
                .count();
            // evidence history：已答的题带学生判分；未答的当前题只带题面
            let answered = rs.results.len();
            let mut same_round: Vec<review::QuestionContext> = rs
                .questions
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    let grading = rs.results.get(i).map(|r| {
                        let missing = if r.missing.is_empty() {
                            String::new()
                        } else {
                            format!("，缺失: {}", r.missing.join("；"))
                        };
                        format!(
                            "{}（{}）{missing}",
                            if r.correct { "答对" } else { "答错" },
                            r.score.map(|s| format!("{s}/100")).unwrap_or_default()
                        )
                    });
                    review::QuestionContext {
                        qtype: match q.q_type {
                            QType::Choice => "choice",
                            QType::ShortAnswer => "short_answer",
                        },
                        concept: q.concept_name.clone(),
                        text: q.question.clone(),
                        grading,
                        aspect: q.aspect.clone(),
                    }
                })
                .collect();
            // 当前正在作答的题（学生还没提交）也进上下文：下一题避免换皮它
            if let Some(cur) = rs.questions.get(answered) {
                same_round.push(review::QuestionContext {
                    qtype: match cur.q_type {
                        QType::Choice => "choice",
                        QType::ShortAnswer => "short_answer",
                    },
                    concept: cur.concept_name.clone(),
                    text: cur.question.clone(),
                    grading: None,
                    aspect: cur.aspect.clone(),
                });
            }
            (
                rs.questions.len(),
                rs.planned,
                rs.quiz_id,
                std::sync::Arc::clone(&rs.ctx),
                review::pick_qtype(rs.planned, done_choice, done_short),
                same_round,
            )
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        if let Some(rs) = &mut self.review {
            rs.next_pending = true;
        }
        self.review_gen = Some(cancel.clone());
        let store = Arc::clone(&self.store);
        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let tx = self.tx.clone();
        tokio::spawn(review::generate_review_question(
            store,
            provider,
            provider_cfg,
            ctx,
            quiz_id,
            index,
            planned,
            qtype,
            asked,
            cancel,
            tx,
        ));
    }

    /// 逐题生成回流：追加题目（当前等待态自动显示）；失败重试已耗尽则优雅收束。
    pub(crate) fn on_review_question_ready(
        &mut self,
        result: Result<(review::ReviewQuestion, Option<String>), String>,
    ) {
        self.review_gen = None;
        self.request_cost_sync();
        let Some(rs) = &mut self.review else {
            return; // 已退出复习：迟到事件丢弃
        };
        match result {
            Ok((q, concept_name)) => {
                let mut q = q;
                q.concept_name = concept_name;
                rs.questions.push(q);
                rs.next_pending = false;
                // 若当前正指向等待槽位，workspace 会自动渲染新题
                self.maybe_spawn_next_question(); // 继续预取
            }
            Err(e) => {
                // 生成重试耗尽：已答的出部分摘要，不硬断
                self.review_gen = None;
                let rs = self.review.take();
                if let Some(rs) = rs
                    && !rs.results.is_empty()
                {
                    self.finish_review_state(rs);
                }
                self.push_entry(Entry::Error(format!("出题失败: {e}")));
            }
        }
    }

    /// 简答题批改回流：校验索引 → 记录 → 反馈停留态。
    pub(crate) fn on_review_graded(
        &mut self,
        question_index: usize,
        result: Result<(i64, Vec<String>, Option<String>), String>,
    ) {
        self.review_grading = false;
        self.request_cost_sync();
        // 校验事件索引：过期/错位的批改结果直接丢弃（防 attempts/mastery 污染）
        let idx_ok = self
            .review
            .as_ref()
            .map(|rs| rs.current == question_index)
            .unwrap_or(false);
        if !idx_ok {
            tracing::warn!(question_index, "过期批改结果被丢弃");
            return;
        }
        match result {
            Ok((score, missing, comment)) => {
                let correct = score >= 70;
                let feedback = format!(
                    "得分: {score}/100{}",
                    if let Some(c) = &comment {
                        format!(" | {c}")
                    } else {
                        String::new()
                    }
                );
                self.finish_review_question(correct, Some(score), &feedback, &missing, None);
            }
            Err(e) => {
                self.push_entry(Entry::Error(format!("批改失败: {e}")));
                self.finish_review_question(false, None, &e, &Vec::new(), None);
            }
        }
    }
}

/// 单题结果快照：(concept_id, 题面摘要, 答对, 得分, 缺失要点)。
type QuestionSnapshot = (Option<i64>, String, bool, Option<i64>, Vec<String>);

/// 复习小结 LLM 输出（掌握度总结 + 下一步建议）。
#[derive(Debug, Deserialize)]
struct ReviewAdvice {
    #[serde(default)]
    mastered: Vec<String>,
    #[serde(default)]
    consolidate: Vec<String>,
    #[serde(default)]
    next: Option<String>,
}
