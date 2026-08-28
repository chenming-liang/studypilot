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
    /// 正确项字母 A/B/C/D（或旧格式下标数字），解析时统一转下标
    #[serde(default)]
    pub answer: Option<String>,
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
    /// 用户选的选项下标（选择题作答时记录，供 workspace 答后渲染 ✗/✓ 染色；简答为 None）
    pub user_choice: Option<usize>,
}

/// 出题素材收集结果：(范围指令, 素材[(概念名?, chunk)], 概念+掌握度)。
type Gathered = (
    String,
    Vec<(Option<String>, storage::ChunkHit)>,
    Vec<storage::ConceptMastery>,
);

/// 启动复习出题流程。
/// 1. 收集素材 + 概念 + 掌握度
/// 2. LLM 生成题目（D4 JSON mode 降级链）
/// 3. 存入 quizzes + questions 表
/// 4. 发送 QuizReady 事件
///
/// 范围两种来源（决定素材与 prompt 的范围指令）：
/// - `scope = Some(文本)`：用户自由文本（--concept），文本当检索 query + 注入
///   prompt 让 AI 自主解析该范围覆盖哪些知识点，只在该范围内出题。
/// - `scope = None`：随机范围——按掌握度加权随机抽概念子集（薄弱优先但范围随机，
///   从源头避免每轮都盯着同一段素材复读），按概念取素材。
#[allow(clippy::too_many_arguments)]
pub async fn start_review(
    store: Arc<Store>,
    provider: Arc<OpenAiClient>,
    provider_cfg: ProviderConfig,
    course_id: Option<i64>,
    course_name: String,
    scope: Option<String>,
    n: usize,
    tx: UnboundedSender<AppEvent>,
    cancel: CancellationToken,
) {
    // ① 收集素材（D2：spawn_blocking）
    // 返回 (范围指令, 素材[(概念名?, chunk)], 概念列表)
    let store_clone = Arc::clone(&store);
    let scope_owned = scope.clone();
    let gathered = tokio::task::spawn_blocking(
        move || -> storage::Result<Gathered> {
            let concepts = store_clone.list_concepts_with_mastery(course_id)?;
            match &scope_owned {
                Some(text) => {
                    // 用户自由文本范围：文本当检索 query，命中率远高于课程名
                    let hits = store_clone.search_chunks(text, course_id, 12)?;
                    let chunks = if hits.is_empty() {
                        store_clone
                            .chunks_by_course(course_id, 24)?
                            .into_iter()
                            .map(|h| (None, h))
                            .collect()
                    } else {
                        hits.into_iter().map(|h| (None, h)).collect()
                    };
                    let directive = format!(
                        "学生指定复习范围：「{text}」。请先按你的理解判断这段范围覆盖哪些知识点，然后只在这个范围内出题。"
                    );
                    Ok((directive, chunks, concepts))
                }
                None => {
                    // 随机范围：掌握度加权随机抽概念（正确率低/未测过权重高）
                    let mut rng = Prng::from_clock();
                    let weights: Vec<f64> = concepts
                        .iter()
                        .map(|c| {
                            let acc = if c.attempts > 0 {
                                c.correct as f64 / c.attempts as f64
                            } else {
                                0.0
                            };
                            1.0 + (1.0 - acc) * 3.0
                        })
                        .collect();
                    let pick_k = n.min(concepts.len()).max(1);
                    let chosen = weighted_sample(&weights, pick_k, &mut rng);
                    let ids: Vec<i64> = chosen.iter().map(|&i| concepts[i].concept_id).collect();
                    let names: Vec<&str> = chosen.iter().map(|&i| concepts[i].name.as_str()).collect();
                    let directive = format!("本次复习覆盖以下概念：{}。请只在这些概念范围内出题，每题绑定一个概念。", names.join("、"));
                    let with_material = store_clone.chunks_by_concepts(&ids, 3)?;
                    let chunks = if with_material.is_empty() {
                        // 课程无概念或概念未关联笔记：退化为按课取材
                        store_clone
                            .chunks_by_course(course_id, 24)?
                            .into_iter()
                            .map(|h| (None, h))
                            .collect()
                    } else {
                        let name_of: std::collections::HashMap<i64, &str> = ids
                            .iter()
                            .zip(names.iter())
                            .map(|(&id, &nm)| (id, nm))
                            .collect();
                        with_material
                            .into_iter()
                            .map(|(cid, h)| (name_of.get(&cid).map(|s| s.to_string()), h))
                            .collect()
                    };
                    Ok((directive, chunks, concepts))
                }
            }
        }
    )
    .await;

    let (directive, chunks, concepts) = match gathered {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            let _ = tx.send(AppEvent::ReviewReady(Err(format!("收集资料失败: {e}"))));
            return;
        }
        Err(e) => {
            let _ = tx.send(AppEvent::ReviewReady(Err(format!("任务错误: {e}"))));
            return;
        }
    };

    if chunks.is_empty() {
        let _ = tx.send(AppEvent::ReviewReady(Err(
            "该课程还没有笔记，先 /import 导入资料".into(),
        )));
        return;
    }

    // ② 构建出题 prompt（重构：考知识不考材料 + 认知层级 + 选项/简答质量 + 范围指令）
    let choice_n = n.div_ceil(2);
    let short_n = n / 2;
    let material: String = chunks
        .iter()
        .enumerate()
        .map(|(i, (concept, h))| {
            let preview: String = h.content.chars().take(700).collect();
            let tag = concept
                .as_deref()
                .map(|c| format!("（概念：{c}）"))
                .unwrap_or_default();
            format!(
                "[{}] 《{}》 > {}{}\n{}",
                i + 1,
                h.note_title,
                h.heading,
                tag,
                preview
            )
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    let concept_list: String = concepts
        .iter()
        .map(|c| format!("{}({}/{})", c.name, c.correct, c.attempts))
        .collect::<Vec<_>>()
        .join("、");

    let prompt = format!(
        "你是《{course_name}》课程的复习出题老师，为学生出 {n} 道复习题。\n\n\
         # 出题范围\n{directive}\n\n\
         # 素材（笔记片段）\n{material}\n\n\
         # 概念与掌握度（名: 答对/总次数）\n{concept_list}\n\n\
         # 出题要求\n\
         - 共 {n} 道：{choice_n} 道选择题 + {short_n} 道简答题。\n\
         - P1 考知识，不考材料：好题的判据是——把题干里「根据笔记」「第X章」「某示例」这类前缀删掉后依然成立。素材只支撑答案，禁止出现在题干里。禁止问「笔记/章节/示例讲了什么」。\n\
         - P2 认知层级：覆盖至少 3 种——Recall（什么是 X）、Explain（为什么 X，X 的机制）、Predict（如果…会怎样，可给代码预测输出）、Apply（X 和 Y 的区别，用 X 解决…）。禁止全是「是什么/复述」。\n\
         - P3 选择题：单选唯一正确、四个选项互斥；干扰项与正确项同质（长度句式相近、源于常见误区、看起来都像对的）；禁止「以上都对/都不对」。\n\
         - P4 简答题：key_points 给 2~4 个可独立判分的要点；问题要引发解释或推理，不能一句话答完。\n\
         - P5 针对性：掌握度低的概念优先；同一概念最多出 1 道；每题 concept 必须取自概念列表。\n\n\
         # 禁止事项（反例，禁止照此出题）\n\
         ✗ 「根据笔记，第 9 章主要讲什么？」——考材料\n\
         ✗ 「在 ffiles1 示例中，abcde.txt 内容为 abcde 时输出是？」——照抄示例\n\
         ✗ 「为什么 Malloc Lab 必须用 valgrind」——环境安装类琐事\n\
         ✗ 逐字复述笔记原句\n\n\
         # 输出格式（只输出 JSON 本体，禁止用代码块包裹整个输出）\n\
         选择题示例：\n\
         {{\"type\":\"choice\",\"question\":\"题干（不带『根据笔记/第X章』前缀）\",\"options\":[\"正确项\",\"干扰项1\",\"干扰项2\",\"干扰项3\"],\"answer\":\"B\",\"explanation\":\"解析\",\"concept\":\"概念名\"}}\n\
         简答题示例：\n\
         {{\"type\":\"short_answer\",\"question\":\"题干\",\"key_points\":[\"要点1\",\"要点2\"],\"explanation\":\"解析\",\"concept\":\"概念名\"}}\n\n\
         优先考察掌握度低（次数少或正确率低）的概念。"
    );

    let messages = [
        Message::system(
            "只输出 JSON 本体（不要用代码块包裹整个输出）。题干或解析中的代码用 ```c 等围栏包裹（写在 JSON 字符串内，换行用 \\n 转义）。",
        ),
        Message::user(&prompt),
    ];

    // ③ D4 降级链（await 期间观察取消：Ctrl+C 立即中断，不等 LLM 返回）
    let content = if cancel.is_cancelled() {
        // 取消也要发事件，否则 on_review_ready 不触发、inflight 卡死
        let _ = tx.send(AppEvent::ReviewReady(Err("已取消".into())));
        return;
    } else {
        match agent_providers::with_cancel(provider.chat_json(&messages), &cancel).await {
            Some(Ok(resp)) => Some(resp),
            Some(Err(e)) if e.is_json_mode_unsupported() => {
                agent_providers::with_cancel(provider.chat(&messages, &[]), &cancel)
                    .await
                    .and_then(|r| r.ok())
            }
            Some(Err(e)) => {
                tracing::warn!("出题失败: {e}");
                None
            }
            None => {
                let _ = tx.send(AppEvent::ReviewReady(Err("已取消".into())));
                return;
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
    let scope_desc = scope
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| "random（随机范围）".into());
    let store_clone = Arc::clone(&store);
    let quiz_id = tokio::task::spawn_blocking(move || -> storage::Result<i64> {
        let qid = store_clone.create_quiz(course_id, &scope_desc)?;
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
            // answer 字母/数字 → 下标（LLM 数选项下标经常数错，字母更稳）
            let answer_idx = q.answer.as_deref().and_then(answer_to_index);
            store_clone.insert_question(
                qid,
                &q.q_type,
                &q.question,
                options_json.as_deref(),
                answer_idx,
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

/// 解析出题 JSON（D4 降级链：直解 → 截取第一个 {...} 块），并对文本字段
/// 做 LLM 输出规范化（换行双重转义还原）。
fn parse_quiz_json(content: &str) -> Option<QuizResponse> {
    let trimmed = agent_core::trim_code_fence(content);
    let mut quiz: QuizResponse =
        serde_json::from_str::<QuizResponse>(trimmed)
            .ok()
            .or_else(|| {
                agent_core::first_json_block(trimmed).and_then(|b| serde_json::from_str(b).ok())
            })?;
    for q in &mut quiz.questions {
        q.question = normalize_text(&q.question);
        q.explanation = q.explanation.as_deref().map(normalize_text);
        for o in &mut q.options {
            *o = normalize_text(o);
        }
        for k in &mut q.key_points {
            *k = normalize_text(k);
        }
    }
    Some(quiz)
}

/// LLM 输出规范化：模型在 JSON 里常把换行写成 `\\n`（双重转义，JSON 解析后是
/// 字面反斜杠+n），渲染成一行很丑。这里把字面 `\n`/`\t` 还原为真实换行/制表符。
/// （已知局限：C 字符串字面量里真正的 `\n` 也会被还原，但实际几乎都是行分隔。）
fn normalize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 选择题答案字母 → 下标。LLM 输出 `"answer": "B"` 或旧格式下标数字，统一转 0-based。
/// 非法输入返回 None（由上层"答案缺失/越界跳过"路径兜底）。
fn answer_to_index(s: &str) -> Option<i64> {
    let t = s.trim().to_uppercase();
    if let Some(c) = t.chars().next()
        && c.is_ascii_alphabetic()
        && t.len() == 1
    {
        return (c as u8).checked_sub(b'A').map(|i| i as i64);
    }
    t.parse::<i64>().ok()
}

/// 极简 xorshift64 PRNG（无外部依赖，随机范围抽概念够用）。
struct Prng(u64);

impl Prng {
    fn from_clock() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// 按权重随机抽 `k` 个索引（不放回）。权重越大越可能被抽中；总权重 ≤0 时停止。
fn weighted_sample(weights: &[f64], k: usize, rng: &mut Prng) -> Vec<usize> {
    let mut remaining: Vec<(usize, f64)> = weights.iter().copied().enumerate().collect();
    let mut chosen = Vec::new();
    while chosen.len() < k && !remaining.is_empty() {
        let total: f64 = remaining.iter().map(|(_, w)| *w).sum();
        if total <= 0.0 {
            break;
        }
        let r = (rng.next() % 1_000_000) as f64 / 1_000_000.0 * total;
        let mut acc = 0.0f64;
        let pos = remaining
            .iter()
            .position(|(_, w)| {
                acc += *w;
                acc >= r
            })
            .unwrap_or(remaining.len() - 1);
        chosen.push(remaining[pos].0);
        remaining.swap_remove(pos);
    }
    chosen
}

#[cfg(test)]
mod quiz_parse_tests {
    use super::*;

    #[test]
    fn normalize_converts_double_escaped_newline() {
        // 字面 `\n`（反斜杠+n 两个字符）→ 真实换行；`\t` → 制表符
        let s = "int main() {\\n    return 0;\\n}";
        assert_eq!(normalize_text(s), "int main() {\n    return 0;\n}");
        // 普通反斜杠（如 Windows 路径）不受影响
        assert_eq!(normalize_text("a\\b"), "a\\b");
    }

    #[test]
    fn parse_quiz_json_normalizes_question_text() {
        // LLM 在 JSON 里把换行双重转义为 \\n（转义反斜杠+n），解析后须还原
        let json = r#"{"questions":[{"type":"short_answer","question":"执行以下代码：\\nint fd1;\\nprintf(\"c1\");","options":[],"answer":null,"key_points":[],"explanation":null,"concept":null}]}"#;
        let q = parse_quiz_json(json).unwrap();
        assert_eq!(
            q.questions[0].question,
            "执行以下代码：\nint fd1;\nprintf(\"c1\");"
        );
    }

    #[test]
    fn answer_letter_maps_to_index() {
        // 新格式：字母 A/B/C/D（大小写均可）
        assert_eq!(answer_to_index("B"), Some(1));
        assert_eq!(answer_to_index("d"), Some(3));
        assert_eq!(answer_to_index(" A "), Some(0));
        // 旧格式：数字下标仍兼容
        assert_eq!(answer_to_index("2"), Some(2));
        // 非法输入 → None（上层兜底跳过）
        assert_eq!(answer_to_index(""), None);
        assert_eq!(answer_to_index("zz"), None);
        // 超出 A-D 的字母 → 越界下标（判分时按"答案非法跳过"兜底）
        assert_eq!(answer_to_index("E"), Some(4));
    }

    #[test]
    fn weighted_sample_respects_count_and_zero_weights() {
        let mut rng = Prng(42);
        // 抽满 k 个、索引不重复
        let s = weighted_sample(&[1.0, 1.0, 1.0], 3, &mut rng);
        assert_eq!(s.len(), 3);
        let mut sorted = s.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2]);
        // 全零权重 → 抽不出
        assert!(weighted_sample(&[0.0, 0.0], 2, &mut rng).is_empty());
        // k 超过数量 → 全量抽完
        assert_eq!(weighted_sample(&[1.0, 2.0], 5, &mut rng).len(), 2);
    }

    #[test]
    fn weighted_sample_prefers_heavy_items() {
        // 权重 0 vs 100：多轮抽样应显著偏向重权重项（确定性种子下验证）
        let mut heavy = 0;
        for seed in 1..50u64 {
            let mut rng = Prng(seed);
            for _ in 0..20 {
                if weighted_sample(&[0.1, 100.0], 1, &mut rng)[0] == 1 {
                    heavy += 1;
                }
            }
        }
        assert!(heavy > 800, "重权重项应被高概率抽中, 实际 {heavy}/1000");
    }
}
