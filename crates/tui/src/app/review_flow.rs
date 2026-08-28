//! 复习流程状态推进：判分回流、题面渲染、掌握度闭环。

use std::sync::Arc;

use tokio::task::spawn_blocking;

use super::{App, AppEvent, Entry};
use crate::review;
impl App {
    /// 完成一道题：记录 attempt + 更新掌握度 + 推进到下一题。
    pub(crate) fn finish_review_question(
        &mut self,
        correct: bool,
        score: Option<i64>,
        feedback: &str,
        missing: &[String],
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
            // attempts/mastery 是掌握度闭环的数据源，静默丢失会让复习质量悄悄劣化
            if let Err(msg) = result {
                let _ = tx.send(AppEvent::DatabaseFailed(msg));
            }
        });

        // ③ 更新 review 状态
        let advance = if let Some(rs) = &mut self.review {
            rs.results.push(review::ReviewResult {
                correct,
                score,
                feedback: feedback.to_owned(),
                missing: missing.to_vec(),
            });
            rs.current += 1;
            rs.current >= rs.questions.len()
        } else {
            false
        };

        // ④ 渲染下一题或结束
        if advance {
            self.finish_review();
        } else {
            self.render_current_question();
        }
    }

    pub(crate) fn render_current_question(&mut self) {
        // 提取数据，避免借用冲突
        let info = self.review.as_ref().and_then(|rs| {
            rs.questions
                .get(rs.current)
                .map(|q| (rs.current, rs.questions.len(), q.clone()))
        });
        let Some((current, total, q)) = info else {
            return;
        };

        // 进度圆点：✓ 答对 · ✗ 答错 · ● 当前 · ○ 未到（Review 进度感）
        let mut dots = String::new();
        let answered = self
            .review
            .as_ref()
            .map(|rs| rs.results.clone())
            .unwrap_or_default();
        for r in &answered {
            if !dots.is_empty() {
                dots.push(' ');
            }
            dots.push(if r.correct { '✓' } else { '✗' });
        }
        for i in answered.len()..total {
            if !dots.is_empty() {
                dots.push(' ');
            }
            dots.push(if i == current { '●' } else { '○' });
        }
        self.push_entry(Entry::Info(format!("Review · {dots}")));
        let type_str = if q.q_type == review::QType::Choice {
            "选择题"
        } else {
            "简答题"
        };
        self.push_entry(Entry::Info(format!(
            "Question {}/{} · {type_str}",
            current + 1,
            total
        )));
        // 题干用 USER 暖黄（Review 模式语义色）
        self.entries.push(Entry::User(q.question.clone()));
        if q.q_type == review::QType::Choice {
            for (i, opt) in q.options.iter().enumerate() {
                self.push_entry(Entry::Info(format!("  {}. {opt}", i + 1)));
            }
            self.push_entry(Entry::Info("数字键 1-9 作答".into()));
        } else {
            self.push_entry(Entry::Info("输入答案后 Enter 提交（Esc 退出复习）".into()));
        }
    }

    pub(crate) fn finish_review(&mut self) {
        let rs = self.review.take();
        let Some(rs) = rs else { return };

        let total = rs.questions.len();
        let correct_count = rs.results.iter().filter(|r| r.correct).count();
        self.push_entry(Entry::Info(format!(
            "=== 复习完成: {correct_count}/{total} 正确 ==="
        )));
        let missed: Vec<&str> = rs
            .results
            .iter()
            .filter(|r| !r.correct)
            .map(|r| r.feedback.as_str())
            .collect();
        if !missed.is_empty() {
            self.push_entry(Entry::Info("薄弱点:".into()));
            for m in &missed {
                self.push_entry(Entry::Info(format!("  • {m}")));
            }
        }
    }

    pub(crate) fn exit_review(&mut self, msg: &str) {
        self.review = None;
        self.push_entry(Entry::Info(msg.to_owned()));
    }

    pub(crate) fn on_review_ready(&mut self, result: Result<review::ReviewState, String>) {
        self.inflight = None; // 出题/取消完成
        self.request_cost_sync();
        match result {
            Ok(rs) => {
                let count = rs.questions.len();
                self.push_entry(Entry::Info(format!("复习开始: 共 {count} 题（Esc 退出）")));
                self.review = Some(rs);
                self.render_current_question();
            }
            Err(e) => {
                self.push_entry(Entry::Error(format!("出题失败: {e}")));
            }
        }
    }

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
                self.push_entry(Entry::Info(feedback.clone()));
                if !missing.is_empty() {
                    self.push_entry(Entry::Info(format!("缺失要点: {}", missing.join("；"))));
                }
                self.finish_review_question(correct, Some(score), &feedback, &missing);
            }
            Err(e) => {
                self.push_entry(Entry::Error(format!("批改失败: {e}")));
                self.finish_review_question(false, None, &e, &Vec::new());
            }
        }
    }
}
