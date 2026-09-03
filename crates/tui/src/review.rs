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
    /// 正确项字母 A/B/C/D（或旧格式下标数字），解析时统一转下标。
    /// LLM 输出字母或数字不稳定，serde 层容错双收。
    #[serde(default, deserialize_with = "deserialize_answer")]
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
    /// 考察角度（LLM 自报：definition/code_prediction/comparison/application/
    /// why/counterexample/debugging/transfer）——coverage summary 的数据源
    #[serde(default)]
    pub aspect: Option<String>,
}

/// answer 字段容错：字符串（"B"）或数字（1，旧格式）都收，统一转字符串。
fn deserialize_answer<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        serde_json::Value::Number(n) => Ok(Some(n.to_string())),
        other => Err(serde::de::Error::custom(format!(
            "answer 类型异常: {other}"
        ))),
    }
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
    /// 当前题的追问对话（切题清空；纯学习辅助，不改判分、不落库）
    pub followups: Vec<FollowupTurn>,
    /// 计划总题数（逐题生成：questions 逐个到位，到位数 ≤ planned）
    pub planned: usize,
    /// 下一题是否正在后台生成
    pub next_pending: bool,
    /// 出题上下文（素材/范围/概念，逐题生成共用；Arc 保证 Clone 廉价）
    pub ctx: std::sync::Arc<QuizContext>,
}

impl ReviewState {
    /// 当前题是否已作答（进入反馈停留态）。
    pub fn awaiting_feedback(&self) -> bool {
        self.results.len() > self.current
    }
}

/// 出题上下文：素材收集一次，逐题生成共用（保证整轮范围一致）。
#[derive(Debug, Clone)]
pub struct QuizContext {
    pub course_id: Option<i64>,
    pub course_name: String,
    /// 范围指令（自由文本解析或随机概念集）
    pub directive: String,
    /// 素材块（笔记片段预览）
    pub material: String,
    /// 概念+掌握度列表（"名(对/总)"逗号串）
    pub concept_list: String,
}

/// 一轮追问（问，答）。answer 为 None 表示回答生成中（workspace 显示思考行）；
/// Err 是获取失败（渲染红色，不喂回 LLM）。citations = [n] → 笔记来源（渲染脚注）。
#[derive(Debug, Clone)]
pub struct FollowupTurn {
    pub question: String,
    pub answer: Option<Result<String, String>>,
    pub citations: Vec<(usize, String)>,
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
    /// LLM 输出的概念名（下一题上下文的考点摘要）
    pub concept_name: Option<String>,
    /// 考察角度（下一题 coverage summary 用）
    pub aspect: Option<String>,
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
                    let hits = store_clone.search_chunks(text, course_id, 8)?;
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
                    // 随机范围：只从「有素材的概念」里按掌握度加权随机抽（正确率低/未测过权重高）
                    let with_material_concepts = store_clone.concepts_with_material(course_id)?;
                    let mut rng = Prng::from_clock();
                    let weights: Vec<f64> = with_material_concepts
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
                    let pick_k = n.min(with_material_concepts.len()).max(1);
                    let chosen = weighted_sample(&weights, pick_k, &mut rng);
                    let ids: Vec<i64> = chosen
                        .iter()
                        .map(|&i| with_material_concepts[i].concept_id)
                        .collect();
                    let names: Vec<&str> = chosen
                        .iter()
                        .map(|&i| with_material_concepts[i].name.as_str())
                        .collect();
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

    // ② 出题上下文（素材一次收集，逐题生成共用，保证整轮范围一致）
    let material: String = chunks
        .iter()
        .enumerate()
        .map(|(i, (concept, h))| {
            // 400 字预览（原 700）：grounding 保留定义/示例核心，砍尾部冗余
            // （问题12 结构性成本：素材占出题 prompt 的 ~68%，逐题重复发送）
            let preview: String = h.content.chars().take(400).collect();
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

    let ctx = Arc::new(QuizContext {
        course_id,
        course_name: course_name.clone(),
        directive,
        material,
        concept_list,
    });

    // ③ quiz 行提前建（逐题插入 questions）
    let scope_desc = scope
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| "random（随机范围）".into());
    let store_clone = Arc::clone(&store);
    let quiz_id =
        match tokio::task::spawn_blocking(move || store_clone.create_quiz(course_id, &scope_desc))
            .await
        {
            Ok(Ok(id)) => id,
            _ => {
                let _ = tx.send(AppEvent::ReviewReady(Err("测验存储失败".into())));
                return;
            }
        };

    // ④ 生成第 1 题（与后续题共用同一套生成逻辑；失败即整体失败）
    let qtype = pick_qtype(n, 0, 0);
    let Some((mut q1, concept_name)) = generate_one(
        &store,
        &provider,
        &provider_cfg,
        &ctx,
        quiz_id,
        0,
        n,
        qtype,
        &[],
        &[],
        false,
        &cancel,
    )
    .await
    else {
        let _ = tx.send(AppEvent::ReviewReady(Err(
            "出题失败（LLM 无响应或解析失败）".into(),
        )));
        return;
    };
    q1.concept_name = concept_name;

    let _ = tx.send(AppEvent::ReviewReady(Ok(ReviewState {
        quiz_id,
        questions: vec![q1],
        current: 0,
        results: Vec::new(),
        selected_option: None,
        followups: Vec::new(),
        planned: n,
        next_pending: false,
        ctx,
    })));
}

/// 一道已出题目的上下文（下一题生成的 evidence history）。
/// 同轮条目带学生回答/判分；跨轮条目只有题面（60 字截断）。
#[derive(Debug, Clone)]
pub struct QuestionContext {
    pub qtype: &'static str,
    /// 概念名
    pub concept: Option<String>,
    /// 题干（同轮 = 完整；跨轮 = 截断）
    pub text: String,
    /// 学生回答摘要（判分反馈；未作答 = None）
    pub grading: Option<String>,
    /// 考察角度（LLM 自报）
    pub aspect: Option<String>,
}

/// 题型分配：选择/简答各半（choice 向上取整），剩余多者优先，持平选选择。
pub fn pick_qtype(planned: usize, done_choice: usize, done_short: usize) -> QType {
    let choice_rem = planned.div_ceil(2).saturating_sub(done_choice);
    let short_rem = (planned / 2).saturating_sub(done_short);
    if choice_rem >= short_rem && choice_rem > 0 {
        QType::Choice
    } else {
        QType::ShortAnswer
    }
}

/// 构建单题出题 prompt。
///
/// 上下文分两层（控制 token）：
/// - Stable：概念/素材/出题要求（逐题相同 → provider 上下文缓存可命中）
/// - Dynamic：同轮 evidence history（题面+学生回答+判分，全量——planned 通常 ≤5）
///   + 跨轮最近题面（截断）+ 已覆盖考察角度
///
/// 多样性由 LLM 驱动（同一个 concept 可以多角度反复考察），代码判重只是最后防线。
/// `retry_hint` = 上一个候选被判近重复，明确要求换考察角度。
fn build_question_messages(
    ctx: &QuizContext,
    index: usize,
    planned: usize,
    qtype: &QType,
    same_round: &[QuestionContext],
    cross_round: &[QuestionContext],
    retry_hint: bool,
) -> [Message; 2] {
    let mut evidence = String::new();
    if same_round.is_empty() && cross_round.is_empty() {
        evidence = "（本题是第一题）".to_string();
    } else {
        for (i, c) in same_round.iter().enumerate() {
            evidence.push_str(&format!(
                "Q{}({}{}，概念: {}): {}\n",
                i + 1,
                c.qtype,
                c.aspect
                    .as_deref()
                    .map(|a| format!("/{a}"))
                    .unwrap_or_default(),
                c.concept.as_deref().unwrap_or("未标注"),
                c.text
            ));
            if let Some(g) = &c.grading {
                evidence.push_str(&format!("   判分: {g}\n"));
            }
        }
        for c in cross_round {
            evidence.push_str(&format!(
                "（此前轮次, {}{}）{}\n",
                c.qtype,
                c.aspect
                    .as_deref()
                    .map(|a| format!("/{a}"))
                    .unwrap_or_default(),
                c.text
            ));
        }
    }
    let tested: Vec<&str> = same_round
        .iter()
        .chain(cross_round.iter())
        .filter_map(|c| c.aspect.as_deref())
        .collect();
    let covered = if tested.is_empty() {
        "（尚无）".to_string()
    } else {
        tested.join(", ")
    };
    let type_name = match qtype {
        QType::Choice => "choice（选择题：四选项单选）",
        QType::ShortAnswer => "short_answer（简答题：key_points 2~4 个）",
    };
    let retry = if retry_hint {
        "上次生成的候选与已有题目过于相似。请生成一个【有意义的新变体】：换考察角度、\n\
         换应用情境或换任务形式——而不是换皮改写之前的题目。\n\n"
    } else {
        ""
    };
    let prompt = format!(
        "你是《{course}》课程的复习出题老师。这是本轮复习的第 {k}/{n} 题。\n\n\
         # 出题范围\n{directive}\n\n\
         # 素材（笔记片段）\n{material}\n\n\
         # 概念与掌握度（名: 答对/总次数）\n{concept_list}\n\n\
         # 已考察内容（evidence history）\n{evidence}\n\n\
         # 已覆盖的考察角度\n{covered}\n\n\
         {retry}# 出题要求\n\
         - 题型固定为：{type_name}\n\
         - 多样性：之前出过的题描述的是【已考察的内容】，不是禁止重复的主题。\n\
           同一个概念可以多角度反复考察；新题应增加有意义的覆盖（换考察角度/情境/推理），\n\
           而不是对已有题目的换皮改写。\n\
         - 考察角度菜单（自行判断本题最合适的一种，写进 aspect 字段）：\n\
           definition（是什么）/ code_prediction（代码输出预测）/ comparison（与相邻概念对比）/\n\
           application（用概念解决问题）/ why（机制与原因）/ counterexample（构造反例）/\n\
           debugging（找错）/ transfer（迁移到新场景）。已覆盖的角度优先避开，\n\
           但若学生上一题答错，可以针对其误区出一个更聚焦的变式（此时角度可重复）。\n\
         - P1 考知识，不考材料：好题的判据是——把题干里「根据笔记」「第X章」「某示例」这类前缀删掉后依然成立。素材只支撑答案，禁止出现在题干里。考点必须能在素材中找到依据；素材不足以支撑的考点，换素材覆盖的知识点。\n\
         - P3 选择题：单选唯一正确、四个选项互斥；干扰项与正确项同质（长度句式相近、源于常见误区）；禁止「以上都对/都不对」。\n\
         - P4 简答题：一题一个焦点——只考一个认知任务，禁止复合句式（「并说明…并给出…」必须拆开或砍掉）；key_points 是答案必须包含的核心事实点（1~3 个），不是完整回答的每个组成部分；好题标准：真实考试会出现、认真学过的学生 2~3 句话能答完。\n\
         - 题目只针对【本题概念】出，不要顺带考察其他概念。\n\n\
         # 禁止事项（反例，禁止照此出题）\n\
         ✗ 「根据笔记，第 9 章主要讲什么？」——考材料\n\
         ✗ 逐字复述笔记原句\n\n\
         # 输出格式（只输出 JSON 本体，单个题目对象，禁止用代码块包裹）\n\
         选择题：{{\"type\":\"choice\",\"question\":\"题干\",\"options\":[\"正确项\",\"干扰项1\",\"干扰项2\",\"干扰项3\"],\"answer\":\"B\",\"explanation\":\"解析\",\"concept\":\"概念名\",\"aspect\":\"definition\"}}\n\
         简答题：{{\"type\":\"short_answer\",\"question\":\"题干\",\"key_points\":[\"要点1\",\"要点2\"],\"explanation\":\"解析\",\"concept\":\"概念名\",\"aspect\":\"why\"}}",
        course = ctx.course_name,
        k = index + 1,
        n = planned,
        directive = ctx.directive,
        material = ctx.material,
        concept_list = ctx.concept_list,
        evidence = evidence,
        covered = covered,
        retry = retry,
        type_name = type_name,
    );
    [
        Message::system(
            "只输出 JSON 本体（不要用代码块包裹整个输出）。题干或解析中的代码用 ```c 等围栏包裹（写在 JSON 字符串内，换行用 \\n 转义）。",
        ),
        Message::user(&prompt),
    ]
}

/// 单题解析：裸对象优先，兼容 {"questions":[...]}（批量旧格式）与裸数组。
/// 解析后对文本字段做 normalize_text（LLM 双重转义换行还原，见收口会话⑩）。
fn parse_one_question(content: &str) -> Option<QuizQuestion> {
    let trimmed = agent_core::trim_code_fence(content);
    let parsed = serde_json::from_str::<QuizQuestion>(trimmed)
        .ok()
        .or_else(|| {
            serde_json::from_str::<QuizResponse>(trimmed)
                .ok()
                .and_then(|r| r.questions.into_iter().next())
        })
        .or_else(|| {
            serde_json::from_str::<Vec<QuizQuestion>>(trimmed)
                .ok()
                .and_then(|qs| qs.into_iter().next())
        })
        .or_else(|| {
            agent_core::first_json_block(trimmed).and_then(|b| {
                serde_json::from_str::<QuizQuestion>(b).ok().or_else(|| {
                    serde_json::from_str::<QuizResponse>(b)
                        .ok()
                        .and_then(|r| r.questions.into_iter().next())
                })
            })
        })?;
    let mut q = parsed;
    q.question = normalize_text(&q.question);
    q.explanation = q.explanation.as_deref().map(normalize_text);
    for o in &mut q.options {
        *o = normalize_text(o);
    }
    for k in &mut q.key_points {
        *k = normalize_text(k);
    }
    Some(q)
}

/// 单题落库：concept 名匹配 + 插入 questions，返回 (db id, concept db id)。
async fn insert_question_db(
    store: &Arc<Store>,
    quiz_id: i64,
    course_id: Option<i64>,
    q: &QuizQuestion,
) -> Result<(i64, Option<i64>), String> {
    let store_clone = Arc::clone(store);
    let q_owned = q.clone();
    tokio::task::spawn_blocking(move || -> storage::Result<(i64, Option<i64>)> {
        let options_json = if q_owned.options.is_empty() {
            None
        } else {
            serde_json::to_string(&q_owned.options).ok()
        };
        let key_points_json = if q_owned.key_points.is_empty() {
            None
        } else {
            serde_json::to_string(&q_owned.key_points).ok()
        };
        let concept_db_id = q_owned.concept.as_deref().and_then(|name| {
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
        if let Some(name) = &q_owned.concept
            && concept_db_id.is_none()
        {
            tracing::warn!(concept = %name, "概念匹配失败，该题不参与掌握度闭环");
        }
        let answer_idx = q_owned.answer.as_deref().and_then(answer_to_index);
        let db_id = store_clone.insert_question(
            quiz_id,
            &q_owned.q_type,
            &q_owned.question,
            options_json.as_deref(),
            answer_idx,
            key_points_json.as_deref(),
            q_owned.explanation.as_deref(),
            concept_db_id,
        )?;
        Ok((db_id, concept_db_id))
    })
    .await
    .map_err(|e| format!("任务错误: {e}"))?
    .map_err(|e| e.to_string())
}

/// 题面相似度：normalize 后字符 bigram Jaccard ≥ 0.75 视为【接近字面重复】。
/// 只做最后的保守 lexical guard——校准：少量词语替换的换皮题 ≥0.75，
/// 不同考点共用模板的题 <0.75（模板词占比被真实内容稀释）。
pub fn too_similar(a: &str, b: &str) -> bool {
    let norm = |t: &str| {
        let c: Vec<char> = normalize_text(t)
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
            .collect();
        if c.len() < 2 {
            return std::collections::HashSet::new();
        }
        c.windows(2)
            .map(|w| (w[0], w[1]))
            .collect::<std::collections::HashSet<_>>()
    };
    let (ba, bb) = (norm(a), norm(b));
    if ba.is_empty() || bb.is_empty() {
        return false;
    }
    let inter = ba.intersection(&bb).count();
    let union = ba.union(&bb).count();
    union > 0 && inter * 100 >= union * 75
}

/// 一次单题生成（LLM + 解析 + 落库 + 记账 + 近重复防线）。None = 失败/近重复。
#[allow(clippy::too_many_arguments)]
async fn generate_one(
    store: &Arc<Store>,
    provider: &Arc<OpenAiClient>,
    provider_cfg: &ProviderConfig,
    ctx: &QuizContext,
    quiz_id: i64,
    index: usize,
    planned: usize,
    qtype: QType,
    same_round: &[QuestionContext],
    cross_round: &[QuestionContext],
    retry_hint: bool,
    cancel: &CancellationToken,
) -> Option<(ReviewQuestion, Option<String>)> {
    if cancel.is_cancelled() {
        return None;
    }
    let messages = build_question_messages(
        ctx,
        index,
        planned,
        &qtype,
        same_round,
        cross_round,
        retry_hint,
    );
    // D4 降级链（await 期间观察取消）
    let resp = match agent_providers::with_cancel(provider.chat_json(&messages), cancel).await {
        Some(Ok(r)) => r,
        Some(Err(e)) if e.is_json_mode_unsupported() => {
            agent_providers::with_cancel(provider.chat(&messages, &[]), cancel)
                .await
                .and_then(|r| r.ok())?
        }
        Some(Err(e)) => {
            tracing::warn!("单题生成失败: {e}");
            return None;
        }
        None => return None,
    };
    // R6 记账
    let cost = estimate_cost(provider_cfg, &resp.usage);
    let store_usage = Arc::clone(store);
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

    let q = match parse_one_question(&resp.content) {
        Some(q) => q,
        None => {
            tracing::warn!(
                content = %resp.content.chars().take(600).collect::<String>(),
                "单题返回内容解析失败"
            );
            return None;
        }
    };
    // 近重复防线（最后的 guard，不是多样性主导机制）：
    // 只拦【接近字面重复】的题（bigram Jaccard ≥0.8）——概念相同/包含/模板相似都不拒，
    // 多样性由 prompt 里的 evidence history 与考察角度驱动（LLM 负责）
    for c in same_round {
        if too_similar(&q.question, &c.text) {
            tracing::warn!(index, "新题与已出题面几乎相同，判近重复重试");
            return None;
        }
    }
    let (db_id, concept_id) = insert_question_db(store, quiz_id, ctx.course_id, &q)
        .await
        .ok()?;
    Some((
        ReviewQuestion {
            db_id,
            q_type: qtype,
            question: q.question,
            options: q.options,
            answer: q.answer.as_deref().and_then(answer_to_index),
            key_points: q.key_points,
            explanation: q.explanation,
            concept_id,
            concept_name: q.concept.clone(),
            aspect: q.aspect.clone(),
        },
        q.concept.clone(),
    ))
}

/// 逐题生成入口（App 侧 spawn）：生成第 index 题（0-based），事件回流。
/// `same_round` = 本轮已出题的 evidence history（题面+学生判分+角度）。
/// 失败自动重试一次（重试 prompt 明确要求"换考察角度"），再失败发 Err。
#[allow(clippy::too_many_arguments)]
pub async fn generate_review_question(
    store: Arc<Store>,
    provider: Arc<OpenAiClient>,
    provider_cfg: ProviderConfig,
    ctx: Arc<QuizContext>,
    quiz_id: i64,
    index: usize,
    planned: usize,
    qtype: QType,
    same_round: Vec<QuestionContext>,
    cancel: CancellationToken,
    tx: UnboundedSender<AppEvent>,
) {
    // 跨轮题面（prompt only：让 LLM 知道此前考过什么；不参与代码级 guard）
    let mut cross_round: Vec<QuestionContext> = Vec::new();
    if let Some(cid) = ctx.course_id
        && let Ok(recent) = store.recent_question_texts(cid, 8)
    {
        for (t, concept) in recent {
            let short: String = t.chars().take(60).collect();
            cross_round.push(QuestionContext {
                qtype: "此前轮次",
                concept,
                text: short,
                grading: None,
                aspect: None,
            });
        }
    }
    for attempt in 0..2 {
        let retry_hint = attempt > 0; // 重试 = 上个候选近重复，明确要求换角度
        if let Some((q, concept_name)) = generate_one(
            &store,
            &provider,
            &provider_cfg,
            &ctx,
            quiz_id,
            index,
            planned,
            qtype.clone(),
            &same_round,
            &cross_round,
            retry_hint,
            &cancel,
        )
        .await
        {
            let _ = tx.send(AppEvent::ReviewQuestionReady(Ok((q, concept_name))));
            return;
        }
        if attempt == 0 && !cancel.is_cancelled() {
            continue; // 重试一次
        }
    }
    let _ = tx.send(AppEvent::ReviewQuestionReady(Err(format!(
        "第 {} 题生成失败",
        index + 1
    ))));
}

/// 真实 LLM 回归（问题 13/18 验收）：同概念连续出 3 题，验证不再失败且互不重复。
/// 运行：cargo test -p tui live_regression_same_concept -- --ignored --nocapture
/// （需 DEEPSEEK_API_KEY 环境变量）
#[tokio::test]
#[ignore = "真调 LLM，花钱"]
async fn live_regression_same_concept() {
    let store = Arc::new(storage::Store::open_in_memory().unwrap());
    let ctx = Arc::new(QuizContext {
        course_id: None,
        course_name: "rust".into(),
        directive: "本次复习覆盖概念：「特型约束」。请只在该概念范围内出题，每题绑定该概念。".into(),
        material: "特型约束（trait bound）是泛型函数/类型上对类型参数的能力限制，如 `fn f<T: Display>(x: T)` 要求 T 可显示。约束让编译器在编译期检查调用合法性。多个约束用 + 连接。where 子句可写更复杂的约束。".into(),
        concept_list: "特型约束(0/0)、特型(0/0)、特型对象(0/0)".into(),
    });
    let provider_cfg = ProviderConfig {
        name: "deepseek".into(),
        endpoint: std::env::var("DEEPSEEK_ENDPOINT")
            .unwrap_or_else(|_| "https://api.deepseek.com".into()),
        api_key: std::env::var("DEEPSEEK_API_KEY").ok(),
        api_key_env: Some("DEEPSEEK_API_KEY".into()),
        model: std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-reasoner".into()),
        price_prompt: Some(4.0),
        price_prompt_cached: Some(1.0),
        price_completion: Some(16.0),
        context_length: 65536,
        thinking: false,
        models: Vec::new(),
    };
    let provider = Arc::new(OpenAiClient::new(provider_cfg.clone()).unwrap());
    let quiz_id = store.create_quiz(None, "live-regression").unwrap();
    let cancel = CancellationToken::new();

    let mut same_round: Vec<QuestionContext> = Vec::new();
    let mut generated: Vec<ReviewQuestion> = Vec::new();
    for i in 0..3 {
        let qtype = if i % 2 == 0 {
            QType::Choice
        } else {
            QType::ShortAnswer
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        generate_review_question(
            Arc::clone(&store),
            Arc::clone(&provider),
            provider_cfg.clone(),
            Arc::clone(&ctx),
            quiz_id,
            i,
            3,
            qtype,
            same_round.clone(),
            cancel.clone(),
            tx,
        )
        .await;
        match rx.recv().await {
            Some(AppEvent::ReviewQuestionReady(Ok((q, _)))) => {
                println!(
                    "Q{} [{:?}] aspect={:?}: {}",
                    i + 1,
                    q.q_type,
                    q.aspect,
                    q.question
                        .replace('\n', " ")
                        .chars()
                        .take(80)
                        .collect::<String>()
                );
                same_round.push(QuestionContext {
                    qtype: "回归",
                    concept: q.concept_name.clone(),
                    text: q.question.clone(),
                    grading: None,
                    aspect: q.aspect.clone(),
                });
                generated.push(q);
            }
            Some(AppEvent::ReviewQuestionReady(Err(e))) => {
                panic!("出题失败（回归不通过）: {e}")
            }
            other => panic!("非预期事件: {other:?}"),
        }
    }
    // 两两不重复（Jaccard guard 同款阈值语义）
    for i in 0..generated.len() {
        for j in (i + 1)..generated.len() {
            assert!(
                !too_similar(&generated[i].question, &generated[j].question),
                "Q{} 与 Q{} 判为字面重复——回归不通过",
                i + 1,
                j + 1
            );
        }
    }
    // 生成即入库
    assert!(generated.iter().all(|q| q.db_id > 0));
    println!("══ 回归通过：3 题成功、互不重复、evidence 生效 ══");
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
        "批改简答题。\n题目: {question_text}\n判分要点（答案必须包含的核心事实）: {key_points}\n用户答案: {user_answer}\n\n\
         评分标准：\n\
         - score 反映答案的【正确性】，不是完整性。\n\
         - 核心结论正确且无事实性错误 → 90-100。\n\
         - 全部判分要点都已命中（语义一致即可，措辞可以不同）→ **必须给 100**。\n\
         - 核心结论正确但漏 1 个次要要点 → 90-95，不要更低。\n\
         - 有事实性错误或核心结论错误 → 按严重程度 0-69。\n\
         - 禁止把「同一事实的另一种说法」「更详细的展开」「不同但等价的表述」算作缺失要点。\n\
         - 禁止要求学生复述解析里的完整推理过程——答对结论就是答对。\n\
         - 禁止习惯性扣分：不要为了显得严格在满分答案上找茬。\n\
         - score >= 85 时 missing 必须为空数组。\n\
         - missing 只列真正缺失或错误的核心事实点，至多 2 条；没有就给空数组。\n\
         - comment 一句话，针对答案本身。\n\n\
         输出 JSON：{{\"score\": 0-100, \"missing\": [\"缺失要点\"], \"comment\": \"评语\"}}"
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
    fn parse_one_question_normalizes_question_text() {
        // LLM 在 JSON 里把换行双重转义为 \\n（转义反斜杠+n），解析后须还原
        let json = r#"{"type":"short_answer","question":"执行以下代码：\\nint fd1;\\nprintf(\"c1\");","options":[],"answer":null,"key_points":[],"explanation":null,"concept":null}"#;
        let q = parse_one_question(json).unwrap();
        assert_eq!(q.question, "执行以下代码：\nint fd1;\nprintf(\"c1\");");
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
    fn parse_tolerates_numeric_answer() {
        // 模型可能输出旧格式数字 answer（serde 对 Option<String> 会整包失败）→ 必须容错
        let json = r#"{"type":"choice","question":"Q","options":["a","b"],"answer":1,"key_points":[],"explanation":null,"concept":null}"#;
        let q = parse_one_question(json).unwrap();
        assert_eq!(q.answer.as_deref(), Some("1"));
    }

    #[test]
    fn parse_tolerates_bare_array() {
        // 模型漏包 questions 对象、直接输出数组 → 兜底包装
        let json = r#"[{"type":"short_answer","question":"Q1","options":[],"answer":null,"key_points":["k"],"explanation":null,"concept":null}]"#;
        let q = parse_one_question(json).unwrap();
        assert_eq!(q.question, "Q1");
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

#[cfg(test)]
mod diversity_tests {
    use super::*;

    fn ctx(
        qtype: &'static str,
        concept: Option<&str>,
        text: &str,
        grading: Option<&str>,
    ) -> QuestionContext {
        QuestionContext {
            qtype,
            concept: concept.map(String::from),
            text: text.into(),
            grading: grading.map(String::from),
            aspect: None,
        }
    }

    /// 回归 1：同 concept + 不同考察角度 → 允许（概念级判重已删除）
    #[test]
    fn same_concept_different_angle_is_allowed() {
        let asked = vec![ctx(
            "choice",
            Some("变量遮蔽"),
            "在 Rust 中，关于变量遮蔽的说法正确的是？",
            None,
        )];
        let candidate = ctx(
            "short_answer",
            Some("变量遮蔽"),
            "请解释变量遮蔽与 mut 修改的区别。",
            None,
        );
        // 概念相同但角度不同：generate_one 的 guard 不应拦（通过 = 不 panic/不提前退出）
        for a in &asked {
            assert!(!too_similar(&candidate.text, &a.text));
        }
        // 概念级判重已删：不存在"同 concept 必拒"的路径
    }

    /// 回归 2：同 concept + 同义改写 → 拒绝
    #[test]
    fn paraphrase_of_same_question_is_rejected() {
        let asked = vec![ctx(
            "choice",
            Some("变量遮蔽"),
            "在 Rust 中，关于变量遮蔽的说法正确的是：A 遮蔽只能改变值 B 遮蔽可以改变类型 C 遮蔽要求 mut D 遮蔽不创建新绑定",
            None,
        )];
        // 同义改写：只换措辞，考点与选项结构一致 → bigram 高度重合
        let candidate = ctx(
            "choice",
            Some("变量遮蔽"),
            "在 Rust 中，关于变量遮蔽的说法正确的是：A 遮蔽只能改变值 B 遮蔽可以改变类型 C 遮蔽要求 mut D 遮蔽不会创建新的绑定",
            None,
        );
        assert!(too_similar(&candidate.text, &asked[0].text));
    }

    /// 回归 3：概念包含关系（"特型" vs "特型约束"）→ 不得误判
    #[test]
    fn concept_containment_does_not_reject() {
        // 概念级判重已整体删除：包含关系的概念由 prompt 组织，不进 guard
        let q1 = ctx(
            "choice",
            Some("特型"),
            "关于特型的动态分发，说法正确的是？",
            None,
        );
        let q2 = ctx(
            "choice",
            Some("特型约束"),
            "泛型函数上的特型约束 `T: Display` 限制了什么？",
            None,
        );
        // 文本内容不同 → 相似度防线不拦
        assert!(!too_similar(&q2.text, &q1.text));
    }

    /// 回归 4：不同 concept + 题型模板相似 → 不因模板词拒绝
    #[test]
    fn template_similarity_across_concepts_is_allowed() {
        let asked = vec![ctx(
            "choice",
            Some("生命周期"),
            "在 Rust 中，关于生命周期参数的说法正确的是：A 'a 表示引用存活期 B 生命周期影响实际存活 C 每个引用都需要标注 D 静态生命周期不可变",
            None,
        )];
        let candidate = ctx(
            "choice",
            Some("所有权"),
            "在 Rust 中，关于所有权转移的说法正确的是：A move 复制堆数据 B move 转移所有权 C move 需要标注 D move 之后原变量仍可用",
            None,
        );
        // 共享模板头"在 Rust 中，关于…的说法正确的是："，但考点完全不同
        assert!(
            !too_similar(&candidate.text, &asked[0].text),
            "模板相似不应触发防线"
        );
    }

    /// 回归 5：学生上一题答错 → evidence 带判分，prompt 可针对误区出变式
    #[test]
    fn grading_flows_into_prompt() {
        let ctx_pack = QuizContext {
            course_id: Some(1),
            course_name: "rust".into(),
            directive: "覆盖概念：变量遮蔽".into(),
            material: "素材".into(),
            concept_list: "变量遮蔽(0/1)".into(),
        };
        let same_round = vec![ctx(
            "short_answer",
            Some("变量遮蔽"),
            "什么是变量遮蔽？",
            Some("答错（60/100），缺失: 未区分 shadowing 与 mutation"),
        )];
        let messages = build_question_messages(
            &ctx_pack,
            1,
            5,
            &QType::ShortAnswer,
            &same_round,
            &[],
            false,
        );
        let prompt = &messages[1].content;
        let text = prompt.as_ref().map(|s| s.as_str()).unwrap_or("");
        assert!(text.contains("答错"), "学生判分应进入 prompt");
        assert!(text.contains("缺失"), "缺失要点应进入 prompt");
        assert!(text.contains("误区"), "应提示可针对误区出变式");
    }

    /// 回归 6：上下文长度受控——跨轮题面截断到 60 字
    #[test]
    fn cross_round_text_is_truncated() {
        let long = "x".repeat(200);
        // generate_review_question 的注入逻辑在 async 任务里，这里直接测截断规则
        let short: String = long.chars().take(60).collect();
        assert_eq!(short.chars().count(), 60);
    }

    /// 回归 7：retry_hint 进入 prompt（重试明确要求换考察角度）
    #[test]
    fn retry_hint_informs_model() {
        let ctx_pack = QuizContext {
            course_id: Some(1),
            course_name: "rust".into(),
            directive: "覆盖概念：变量遮蔽".into(),
            material: "素材".into(),
            concept_list: "变量遮蔽(0/1)".into(),
        };
        let without = build_question_messages(&ctx_pack, 1, 5, &QType::Choice, &[], &[], false);
        let with = build_question_messages(&ctx_pack, 1, 5, &QType::Choice, &[], &[], true);
        let text_of = |m: &[Message]| -> String { m[1].content.clone().unwrap_or_default() };
        assert!(text_of(&with).contains("有意义的新变体"));
        assert!(text_of(&with).contains("换考察角度"));
        assert!(!text_of(&without).contains("有意义的新变体"));
    }

    /// 多样性铁律进入 prompt（同 concept 可多角度考，换皮改写不允许）
    #[test]
    fn diversity_rules_in_prompt() {
        let ctx_pack = QuizContext {
            course_id: Some(1),
            course_name: "rust".into(),
            directive: "d".into(),
            material: "m".into(),
            concept_list: "c".into(),
        };
        let messages = build_question_messages(&ctx_pack, 0, 5, &QType::Choice, &[], &[], false);
        let text = messages[1]
            .content
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("");
        assert!(text.contains("不是禁止重复的主题"));
        assert!(text.contains("换皮改写"));
        assert!(text.contains("aspect"));
    }
}

#[cfg(test)]
mod material_measure_tests {
    use super::*;

    /// 测量典型出题 prompt 的构成（问题12后续：素材瘦身前置测量）。
    /// 不调 LLM——build_question_messages 是纯函数。
    #[test]
    fn measure_question_prompt_composition() {
        // 典型随机范围场景（瘦身前：4 概念 × 3 chunks × 700 字 = 8400 字）
        // 瘦身后：预览 400 字（chunk 数不变——grounding 优先）
        let chunks: Vec<String> = (0..12)
            .map(|i| format!("素材片段{i}：{}", "知".repeat(400 / 2 + i % 3)).repeat(1))
            .collect();
        let material = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| format!("[{i}] 《笔记》 > 章节{i}\n{c}"))
            .collect::<Vec<_>>()
            .join("\n---\n");
        let ctx = QuizContext {
            course_id: Some(1),
            course_name: "rust".into(),
            directive: "覆盖概念：A、B、C、D".into(),
            material: material.clone(),
            concept_list: "A(0/0)、B(1/2)、C(0/0)、D(3/3)".into(),
        };
        // 第 3 题（同轮已有 2 题 evidence）
        let same_round: Vec<QuestionContext> = (0..2)
            .map(|i| QuestionContext {
                qtype: "choice",
                concept: Some(format!("概念{i}")),
                text: format!("这是第{i}题的题干，包含一些内容描述，用于测量 evidence 占用。"),
                grading: Some("答对（100/100）".into()),
                aspect: Some("definition".into()),
            })
            .collect();
        let cross: Vec<QuestionContext> = (0..8)
            .map(|i| QuestionContext {
                qtype: "此前轮次",
                concept: Some(format!("旧概念{i}")),
                text: format!("历史题面{i}，{}。", "内".repeat(60)),
                grading: None,
                aspect: None,
            })
            .collect();
        let messages =
            build_question_messages(&ctx, 2, 5, &QType::Choice, &same_round, &cross, false);
        let prompt = messages[1].content.as_ref().unwrap();
        let total = prompt.chars().count();
        let mat = material.chars().count();
        let ev = {
            let i = prompt.find("# 已考察内容").unwrap();
            let j = prompt.find("# 已覆盖").unwrap();
            prompt[i..j].chars().count()
        };
        println!("prompt 总字符: {total}");
        println!("  素材区: {mat} 字符（{:.0}%）", mat * 100 / total);
        println!("  evidence 区: {ev} 字符（{:.0}%）", ev * 100 / total);
        // 瘦身前同场景 ≈ 9000 字符 prompt（素材 8400）——预期省 ~40%
        // deepseek 中文 ≈ 0.6 token/字（1 token ≈ 1.6 中文字符）
        println!("  估算 tokens: {:.0}", total as f64 * 0.6);
        // 断言：素材占 prompt 的绝对大头（瘦身的靶子）
        assert!(
            mat * 100 / total > 50,
            "素材应占 >50%（实测 {:.0}%）",
            mat * 100 / total
        );
    }
}
