//! M8 复习模式：LLM 出题 → 选择题本地判分 / 简答题 LLM 批改 → 掌握度闭环。
//! 规划：选择题数字键 1-4 作答、简答题文本框提交、进度 [3/5]、Esc 退出。

use std::sync::Arc;

use agent_core::{Message, Provider};
use agent_providers::{OpenAiClient, ProviderConfig, estimate_cost};
use serde::Deserialize;

use storage::Store;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::app::AppEvent;

/// 一道题目（从 LLM JSON 解析）。
#[derive(Debug, Clone, Deserialize)]
pub struct QuizQuestion {
    #[serde(rename = "type")]
    pub q_type: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub answer: Option<i64>,
    #[serde(default)]
    pub key_points: Vec<String>,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub concept_id: Option<i64>,
    #[serde(default)]
    pub concept: Option<String>,
}

/// LLM 出题结果。
#[derive(Debug, Deserialize)]
struct QuizResponse {
    questions: Vec<QuizQuestion>,
}

/// 简答题批改结果。
#[derive(Debug, Deserialize)]
struct GradeResult {
    score: i64,
    #[serde(default)]
    missing: Vec<String>,
    #[serde(default)]
    comment: Option<String>,
}

/// 复习模式的运行时状态。
#[derive(Debug, Clone)]
pub struct ReviewState {
    #[allow(dead_code)]
    pub quiz_id: i64,
    pub questions: Vec<ReviewQuestion>,
    pub current: usize,
    pub results: Vec<ReviewResult>,
    /// 选择题光标（workspace ↑↓ 移动；None = 未开始选择）
    pub selected_option: Option<usize>,
}

impl ReviewState {
    /// 当前题是否已作答（进入反馈停留态）。
    pub fn awaiting_feedback(&self) -> bool {
        self.results.len() > self.current
    }
}

/// 一道题目（含 DB id）。
#[derive(Debug, Clone)]
pub struct ReviewQuestion {
    pub db_id: i64,
    pub q_type: QType,
    pub question: String,
    pub options: Vec<String>,
    pub answer: Option<i64>,
    pub key_points: Vec<String>,
    pub explanation: Option<String>,
    pub concept_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QType {
    Choice,
    ShortAnswer,
}

/// 单题作答结果。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReviewResult {
    pub correct: bool,
    pub score: Option<i64>,
    pub feedback: String,
    pub missing: Vec<String>,
}

/// 启动复习出题流程。
/// 1. 收集课程笔记片段 + 概念 + 掌握度（薄弱优先）
/// 2. LLM 生成题目（D4 JSON mode 降级链）
/// 3. 存入 quizzes + questions 表
/// 4. 发送 QuizReady 事件
#[allow(clippy::too_many_arguments)]
pub async fn start_review(
    store: Arc<Store>,
    provider: Arc<OpenAiClient>,
    provider_cfg: ProviderConfig,
    course_id: Option<i64>,
    course_name: String,
    scope: String,
    n: usize,
    tx: UnboundedSender<AppEvent>,
    cancel: CancellationToken,
) {
    // ① 收集资料（D2：spawn_blocking）
    let store_clone = Arc::clone(&store);
    let scope_for_search = scope.clone();
    let gathered = tokio::task::spawn_blocking(
        move || -> storage::Result<(Vec<storage::ChunkHit>, Vec<storage::ConceptMastery>)> {
            // 用课程范围检索相关片段
            let hits = store_clone.search_chunks(&scope_for_search, course_id, 10)?;
            // 概念 + 掌握度（薄弱优先）
            let concepts = store_clone.list_concepts_with_mastery(course_id)?;
            Ok((hits, concepts))
        },
    )
    .await;

    let (chunks, concepts) = match gathered {
        Ok(Ok((c, m))) => (c, m),
        Ok(Err(e)) => {
            let _ = tx.send(AppEvent::ReviewReady(Err(format!("收集资料失败: {e}"))));
            return;
        }
        Err(e) => {
            let _ = tx.send(AppEvent::ReviewReady(Err(format!("任务错误: {e}"))));
            return;
        }
    };

    // 兜底：概念/关键词检索未命中时，取该课程全部 chunk（复习整课不该依赖
    // "概念词恰好能搜到笔记"——课程名作 query 通常零命中）
    let chunks = if chunks.is_empty() {
        let fallback = tokio::task::spawn_blocking({
            let store = Arc::clone(&store);
            move || {
                store
                    .chunks_by_course(course_id, 24)
                    .map_err(|e| e.to_string())
            }
        })
        .await
        .map_err(|e| format!("任务错误: {e}"))
        .and_then(|r| r);
        match fallback {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppEvent::ReviewReady(Err(format!("收集资料失败: {e}"))));
                return;
            }
        }
    } else {
        chunks
    };

    if chunks.is_empty() {
        let _ = tx.send(AppEvent::ReviewReady(Err(
            "该课程还没有笔记，先 /import 导入资料".into(),
        )));
        return;
    }

    // ② 构建出题 prompt
    let material: String = chunks
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let preview: String = h.content.chars().take(1000).collect();
            format!("[{}] {} > {}\n{}", i + 1, h.note_title, h.heading, preview)
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    let concept_list: String = concepts
        .iter()
        .map(|c| format!("{}({}/{})", c.name, c.correct, c.attempts))
        .collect::<Vec<_>>()
        .join("、");

    let prompt = format!(
        "基于以下《{course_name}》课程的笔记资料和概念列表，生成 {n} 道复习题。\n\
         要求：选择题和简答题混合，题目出自笔记内容（不要使用模型自身知识）。\n\
         输出 JSON：\n\
         {{\"questions\": [{{\n\
           \"type\": \"choice\" 或 \"short_answer\",\n\
           \"question\": \"题目\",\n\
           \"options\": [\"A\",\"B\",\"C\",\"D\"],  // 仅选择题\n\
           \"answer\": 0,  // 选择题正确选项下标（0-based）\n\
           \"key_points\": [\"要点\"],  // 仅简答题\n\
           \"explanation\": \"解析\",\n\
           \"concept\": \"对应概念名\"\n\
         }}]}}\n\n\
         笔记资料:\n{material}\n\n概念(正确/总次数): {concept_list}\n\n\
         优先考察掌握度低（次数少或正确率低）的概念。"
    );

    let messages = [
        Message::system("只输出 JSON，不要 markdown 代码块。"),
        Message::user(&prompt),
    ];

    // ③ D4 降级链
    let content = if cancel.is_cancelled() {
        // 取消也要发事件，否则 on_review_ready 不触发、inflight 卡死
        let _ = tx.send(AppEvent::ReviewReady(Err("已取消".into())));
        return;
    } else {
        match provider.chat_json(&messages).await {
            Ok(resp) => Some(resp),
            Err(e) if e.is_json_mode_unsupported() => provider.chat(&messages, &[]).await.ok(),
            Err(e) => {
                tracing::warn!("出题失败: {e}");
                None
            }
        }
    };

    let Some(resp) = content else {
        let _ = tx.send(AppEvent::ReviewReady(Err("LLM 无响应".into())));
        return;
    };

    // R6：记录 usage
    let cost = estimate_cost(&provider_cfg, &resp.usage);
    let store_usage = Arc::clone(&store);
    let pc_usage = provider_cfg.clone();
    let u = resp.usage;
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            store_usage.append_usage(
                &pc_usage.name,
                &pc_usage.model,
                "review",
                u.prompt_tokens,
                u.completion_tokens,
                cost,
            )
        })
        .await;
    });

    // ④ 解析题目 JSON
    let quiz: QuizResponse = match parse_quiz_json(&resp.content) {
        Some(q) => q,
        None => {
            let _ = tx.send(AppEvent::ReviewReady(Err("题目 JSON 解析失败".into())));
            return;
        }
    };

    // ⑤ 存入 DB
    let store_clone = Arc::clone(&store);
    let quiz_id = tokio::task::spawn_blocking(move || -> storage::Result<i64> {
        let qid = store_clone.create_quiz(course_id, &scope.clone())?;
        for q in &quiz.questions {
            let options_json = if q.options.is_empty() {
                None
            } else {
                serde_json::to_string(&q.options).ok()
            };
            let key_points_json = if q.key_points.is_empty() {
                None
            } else {
                serde_json::to_string(&q.key_points).ok()
            };
            // 通过 concept 名查 id：精确匹配 → 包含匹配（LLM 拼写不可控）；
            // 仍失败则记日志——该题游离于掌握度闭环之外需可感知
            let concept_db_id = q.concept.as_deref().and_then(|name| {
                let list = store_clone
                    .list_concepts_with_mastery(course_id)
                    .unwrap_or_default();
                list.iter()
                    .find(|c| c.name == name)
                    .or_else(|| {
                        list.iter()
                            .find(|c| c.name.contains(name) || name.contains(&c.name))
                    })
                    .map(|c| c.concept_id)
            });
            if let Some(name) = &q.concept
                && concept_db_id.is_none()
            {
                tracing::warn!(concept = %name, "概念匹配失败，该题不参与掌握度闭环");
            }
            store_clone.insert_question(
                qid,
                &q.q_type,
                &q.question,
                options_json.as_deref(),
                q.answer,
                key_points_json.as_deref(),
                q.explanation.as_deref(),
                concept_db_id,
            )?;
        }
        Ok(qid)
    })
    .await
    .ok()
    .and_then(|r| r.ok());

    let Some(quiz_id) = quiz_id else {
        let _ = tx.send(AppEvent::ReviewReady(Err("测验存储失败".into())));
        return;
    };

    // ⑥ 转换为 ReviewQuestion 并发送
    let store_clone = Arc::clone(&store);
    let questions = tokio::task::spawn_blocking(move || -> storage::Result<Vec<ReviewQuestion>> {
        let records = store_clone.get_quiz_questions(quiz_id)?;
        Ok(records
            .into_iter()
            .map(|r| ReviewQuestion {
                db_id: r.id,
                q_type: if r.q_type == "choice" {
                    QType::Choice
                } else {
                    QType::ShortAnswer
                },
                question: r.question,
                options: r
                    .options_json
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                answer: r.answer,
                key_points: r
                    .key_points_json
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                explanation: r.explanation,
                concept_id: r.concept_id,
            })
            .collect())
    })
    .await
    .ok()
    .and_then(|r| r.ok());

    let Some(questions) = questions else {
        let _ = tx.send(AppEvent::ReviewReady(Err("题目加载失败".into())));
        return;
    };

    if questions.is_empty() {
        let _ = tx.send(AppEvent::ReviewReady(Err("未生成任何题目".into())));
        return;
    }

    let _ = tx.send(AppEvent::ReviewReady(Ok(ReviewState {
        quiz_id,
        questions,
        current: 0,
        results: Vec::new(),
        selected_option: None,
    })));
}

/// 简答题批改：用户答案 + 笔记原文 + key_points → LLM → score/missing。
pub async fn grade_short_answer(
    provider: Arc<OpenAiClient>,
    provider_cfg: ProviderConfig,
    store: Arc<Store>,
    question: &ReviewQuestion,
    user_answer: &str,
    tx: UnboundedSender<AppEvent>,
    question_index: usize,
) {
    let key_points = question.key_points.join("；");
    let question_text = &question.question;
    let prompt = format!(
        "批改简答题。\n题目: {question_text}\n参考要点: {key_points}\n用户答案: {user_answer}\n\n\
         输出 JSON：{{\"score\": 0-100, \"missing\": [\"缺失要点\"], \"comment\": \"评语\"}}\n\
         评分标准：完全正确 90-100，大部分正确 70-89，部分正确 40-69，错误 0-39。"
    );

    let messages = [
        Message::system("只输出 JSON，不要 markdown 代码块。"),
        Message::user(&prompt),
    ];

    let content = match provider.chat_json(&messages).await {
        Ok(resp) => Some(resp),
        Err(e) if e.is_json_mode_unsupported() => provider.chat(&messages, &[]).await.ok(),
        Err(e) => {
            tracing::warn!("批改失败: {e}");
            None
        }
    };

    let Some(resp) = content else {
        let _ = tx.send(AppEvent::ReviewGraded(
            question_index,
            Err("批改 LLM 无响应".into()),
        ));
        return;
    };

    // R6
    let cost = estimate_cost(&provider_cfg, &resp.usage);
    let u = resp.usage;
    let store_clone = Arc::clone(&store);
    let pc = provider_cfg.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            store_clone.append_usage(
                &pc.name,
                &pc.model,
                "grade",
                u.prompt_tokens,
                u.completion_tokens,
                cost,
            )
        })
        .await;
    });

    // 解析批改结果（D4 降级链的截取兜底，共用 agent_core 工具）
    let grade: GradeResult = match serde_json::from_str(resp.content.trim()) {
        Ok(g) => g,
        Err(_) => {
            let Some(json_block) = agent_core::first_json_block(&resp.content) else {
                let _ = tx.send(AppEvent::ReviewGraded(
                    question_index,
                    Err("批改 JSON 解析失败".into()),
                ));
                return;
            };
            match serde_json::from_str(json_block) {
                Ok(g) => g,
                Err(e) => {
                    let _ = tx.send(AppEvent::ReviewGraded(
                        question_index,
                        Err(format!("批改 JSON 解析失败: {e}")),
                    ));
                    return;
                }
            }
        }
    };

    let _ = tx.send(AppEvent::ReviewGraded(
        question_index,
        Ok((grade.score, grade.missing.clone(), grade.comment.clone())),
    ));
}

/// 解析出题 JSON（D4 降级链：直解 → 截取第一个 {...} 块）。
fn parse_quiz_json(content: &str) -> Option<QuizResponse> {
    let trimmed = agent_core::trim_code_fence(content);
    serde_json::from_str::<QuizResponse>(trimmed)
        .ok()
        .or_else(|| {
            agent_core::first_json_block(trimmed).and_then(|b| serde_json::from_str(b).ok())
        })
}
