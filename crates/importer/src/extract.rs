//! LLM 概念抽取（决策 D4：JSON mode 降级链）。
//!
//! 降级链：response_format json_object → 不支持/报错则 prompt 约束 + 正则提取 →
//! 解析失败重试一次 → 再失败跳过（只存原文）。

use std::sync::Arc;

use agent_core::{Message, Provider, Usage};
use agent_providers::{OpenAiClient, ProviderConfig, estimate_cost};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::parser::RawDoc;

/// LLM 抽取结果。
#[derive(Debug, Clone, Default)]
pub struct ExtractResult {
    pub title: Option<String>,
    pub course: Option<String>,
    #[allow(dead_code)]
    pub summary: Option<String>,
    pub concepts: Vec<String>,
}

#[derive(Deserialize)]
struct ExtractResponse {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    course: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    concepts: Vec<ConceptDef>,
}

#[derive(Deserialize)]
struct ConceptDef {
    name: String,
}

/// 抽取概念。`fixed_course` 非空时跳过 LLM 归类（直接用该课程名）。
/// 返回 None 表示彻底失败（调用方只存原文）。
#[allow(clippy::too_many_arguments)]
pub async fn extract(
    provider: &OpenAiClient,
    provider_cfg: &ProviderConfig,
    store: Arc<storage::Store>,
    accumulated_cost: &mut f64,
    max_cost: f64,
    doc: &RawDoc,
    fixed_course: Option<&str>,
    cancel: &CancellationToken,
) -> Option<ExtractResult> {
    // R6 + D2：读累计成本经 spawn_blocking
    let store_for_cost = Arc::clone(&store);
    let recorded = tokio::task::spawn_blocking(move || store_for_cost.total_recorded_cost())
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(0.0);
    *accumulated_cost = recorded;

    // R6：发起请求前检查预算
    if *accumulated_cost >= max_cost {
        tracing::warn!(
            cost = *accumulated_cost,
            max_cost,
            "已达预算上限，跳过概念抽取"
        );
        return None;
    }

    let prompt = build_prompt(doc, fixed_course);
    let messages = [
        Message::system(
            "你是知识库助手。从笔记内容中提取结构化信息。\
             只输出 JSON，不要 markdown 代码块、不要多余文字。",
        ),
        Message::user(&prompt),
    ];

    // 第一跳：尝试 JSON mode；记录 usage 落库（R6）。等待期间观察取消（Ctrl+C 立即停）。
    let content = match agent_providers::with_cancel(provider.chat_json(&messages), cancel).await {
        Some(Ok(resp)) => {
            log_usage(&store, provider_cfg, &resp.usage).await;
            *accumulated_cost += estimate_cost(provider_cfg, &resp.usage);
            Some(resp.content)
        }
        Some(Err(e)) if e.is_json_mode_unsupported() => {
            // endpoint 不支持 json_object → 降级：prompt 约束
            // （认证失败/限流等错误不降级——换普通 chat 同样会失败）
            tracing::info!("JSON mode 不支持，降级为 prompt 约束");
            match agent_providers::with_cancel(provider.chat(&messages, &[]), cancel).await {
                Some(Ok(resp)) => {
                    log_usage(&store, provider_cfg, &resp.usage).await;
                    *accumulated_cost += estimate_cost(provider_cfg, &resp.usage);
                    Some(resp.content)
                }
                Some(Err(e)) => {
                    tracing::warn!("LLM 调用失败: {e}");
                    None
                }
                None => None,
            }
        }
        Some(Err(e)) => {
            tracing::warn!("LLM 调用失败: {e}");
            None
        }
        None => None,
    };

    let content = content?;

    if cancel.is_cancelled() {
        return None;
    }

    // 解析：直接 parse → 正则提取 → 重试一次
    match parse_json(&content) {
        Some(mut result) => {
            if let Some(fc) = fixed_course {
                result.course = Some(fc.to_owned());
            }
            Some(result)
        }
        None => {
            tracing::warn!("首次 JSON 解析失败，重试一次");
            let retry_messages = [
                Message::system(
                    "只输出 JSON，不要 markdown 代码块、不要多余文字。上次输出不合法，请修正。",
                ),
                Message::user(&prompt),
            ];
            let retry = agent_providers::with_cancel(provider.chat(&retry_messages, &[]), cancel)
                .await
                .and_then(|r| r.ok());
            if let Some(r) = &retry {
                log_usage(&store, provider_cfg, &r.usage).await;
                *accumulated_cost += estimate_cost(provider_cfg, &r.usage);
            }
            retry
                .as_ref()
                .and_then(|r| parse_json(&r.content))
                .map(|mut result| {
                    if let Some(fc) = fixed_course {
                        result.course = Some(fc.to_owned());
                    }
                    result
                })
        }
    }
}

/// R6：导入管线的 LLM 调用也记入 usage_log。
/// D2：spawn_blocking 执行；同步 await 完成以保证预算检查读到最新累计值（防竞态突破 max_cost）。
async fn log_usage(store: &Arc<storage::Store>, cfg: &ProviderConfig, usage: &Usage) {
    let cost = estimate_cost(cfg, usage);
    let cfg_name = cfg.name.clone();
    let cfg_model = cfg.model.clone();
    let prompt = usage.prompt_tokens;
    let completion = usage.completion_tokens;
    let store = Arc::clone(store);
    if let Err(e) = tokio::task::spawn_blocking(move || {
        store.append_usage(&cfg_name, &cfg_model, "import", prompt, completion, cost)
    })
    .await
    {
        tracing::warn!("import usage_log 写入失败: {e:?}");
    }
}

fn build_prompt(doc: &crate::parser::RawDoc, fixed_course: Option<&str>) -> String {
    let preview: String = doc.text.chars().take(2000).collect();
    let course_hint = fixed_course
        .map(|c| format!("（课程已指定为 {c}，course 字段填 {c}）"))
        .unwrap_or_default();
    format!(
        "从以下笔记内容中提取结构化信息，输出 JSON：\n\
         {{\"title\": \"标题\", \"course\": {course} 或 null, \"summary\": \"一句话摘要\", \
         \"concepts\": [{{\"name\": \"概念名\"}}]}}\n\n\
         笔记标题: {title}\n{course_hint}\n\n笔记内容:\n{preview}",
        course = if fixed_course.is_some() {
            "课程名"
        } else {
            "null"
        },
        title = doc.title,
        course_hint = course_hint,
        preview = preview,
    )
}

/// 解析 JSON：直接 from_str → 正则提取第一个完整 {...} → None。
fn parse_json(content: &str) -> Option<ExtractResult> {
    // 去除可能的 markdown 代码块包裹
    let trimmed = agent_core::trim_code_fence(content);

    if let Ok(resp) = serde_json::from_str::<ExtractResponse>(trimmed) {
        return Some(resp.into());
    }

    // 提取第一个完整的 {...} 块再解
    if let Some(json_str) = agent_core::first_json_block(trimmed)
        && let Ok(resp) = serde_json::from_str::<ExtractResponse>(json_str)
    {
        return Some(resp.into());
    }
    None
}

impl From<ExtractResponse> for ExtractResult {
    fn from(r: ExtractResponse) -> Self {
        Self {
            title: r.title,
            course: r.course,
            summary: r.summary,
            concepts: r.concepts.into_iter().map(|c| c.name).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_json() {
        let json = r#"{"title":"所有权","course":"rust","summary":"Rust所有权","concepts":[{"name":"所有权"},{"name":"移动语义"}]}"#;
        let r = parse_json(json).unwrap();
        assert_eq!(r.title.as_deref(), Some("所有权"));
        assert_eq!(r.course.as_deref(), Some("rust"));
        assert_eq!(r.concepts, vec!["所有权", "移动语义"]);
    }

    #[test]
    fn parse_markdown_wrapped_json() {
        let json = "```json\n{\"title\":\"T\",\"concepts\":[]}\n```";
        let r = parse_json(json).unwrap();
        assert_eq!(r.title.as_deref(), Some("T"));
    }

    #[test]
    fn parse_json_with_noise() {
        let json = "好的，这是结果：\n{\"title\":\"T\",\"concepts\":[{\"name\":\"C\"}]}\n以上。";
        let r = parse_json(json).unwrap();
        assert_eq!(r.concepts, vec!["C"]);
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert!(parse_json("这不是JSON").is_none());
    }
}
