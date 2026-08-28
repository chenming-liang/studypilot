//! 复习流程状态推进：判分回流、题面渲染、掌握度闭环。

use std::sync::Arc;

use tokio::task::spawn_blocking;

use super::{App, AppEvent, Entry};
use crate::review;
impl App {
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
    pub(crate) fn advance_review(&mut self) {
        let Some(rs) = &mut self.review else { return };
        rs.current += 1;
        rs.selected_option = None;
        if rs.current >= rs.questions.len() {
            self.finish_review();
        }
    }

    pub(crate) fn finish_review(&mut self) {
        let rs = self.review.take();
        let Some(rs) = rs else { return };

        let total = rs.questions.len();
        let correct_count = rs.results.iter().filter(|r| r.correct).count();
        self.push_entry(Entry::Info("══ Review Complete ══".into()));
        self.push_entry(Entry::Info(format!("正确 {correct_count}/{total}")));
        for (i, (q, r)) in rs.questions.iter().zip(rs.results.iter()).enumerate() {
            let mark = if r.correct { "✓" } else { "✗" };
            let score = r.score.map(|s| format!(" · {s}/100")).unwrap_or_default();
            self.push_entry(Entry::Info(format!(
                "{mark} Q{} · {}{score}",
                i + 1,
                q.question.chars().take(24).collect::<String>()
            )));
            if !r.missing.is_empty() {
                self.push_entry(Entry::Info(format!("   缺失: {}", r.missing.join("；"))));
            }
        }
        self.push_entry(Entry::Info(format!(
            "正确率 {:.0}%",
            if total > 0 {
                correct_count as f64 * 100.0 / total as f64
            } else {
                0.0
            }
        )));
    }

    pub(crate) fn exit_review(&mut self, msg: &str) {
        // 中途退出：若有进度，同样出摘要卡（部分完成）
        let has_progress = self
            .review
            .as_ref()
            .map(|rs| !rs.results.is_empty())
            .unwrap_or(false);
        self.review = None;
        if has_progress {
            self.finish_review();
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
                let count = rs.questions.len();
                self.review = Some(rs);
                self.push_entry(Entry::Info(format!("· Review started · {count} questions")));
            }
            Err(e) => {
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
