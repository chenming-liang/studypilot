//! 检索评测器。
//!
//! 离线模式（默认，不调 LLM、零成本）：只测检索命中率。
//! `cargo run --release -p tui --example eval -- --out eval/report.md`
//!
//! LLM 模式（--llm，真调 API 花钱）：完整 RAG 链路回答 + judge 评分，
//! 测"检索找到之后回答对不对"——检索层测不到的那一半。
//! `cargo run --release -p tui --example eval -- --llm [--limit 5] --out eval/report-llm.md`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use agent_core::{Message, Provider};
use agent_providers::{Config, OpenAiClient, estimate_cost};
use serde::Deserialize;
use storage::Store;

#[derive(Deserialize)]
struct Question {
    id: usize,
    q: String,
    /// 期望命中的笔记标题关键词（子串匹配）
    expect: Vec<String>,
}

struct Row {
    id: usize,
    q: String,
    hit: Option<usize>, // Some(rank) / None
    top_titles: Vec<String>,
    elapsed_ms: u128,
    // ---- --llm 模式字段 ----
    answer: Option<String>,
    cited_sources: Vec<String>, // 回答中 [n] 引用对应的来源
    score: Option<i64>,
    hallucination: Option<bool>,
    judge_reason: Option<String>,
    cost: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut db = PathBuf::from("data/mynotes.db");
    let mut questions_path = PathBuf::from("eval/questions.jsonl");
    let mut out_path = PathBuf::from("eval/report-baseline.md");
    let mut topk = 5usize;
    let mut with_llm = false;
    let mut limit: Option<usize> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--db" => db = PathBuf::from(args.next().expect("--db 需要参数")),
            "--questions" => {
                questions_path = PathBuf::from(args.next().expect("--questions 需要参数"))
            }
            "--out" => out_path = PathBuf::from(args.next().expect("--out 需要参数")),
            "--topk" => topk = args.next().expect("--topk 需要参数").parse()?,
            "--llm" => with_llm = true,
            "--limit" => limit = Some(args.next().expect("--limit 需要参数").parse()?),
            other => anyhow::bail!(
                "未知参数 {other}（支持 --db/--questions/--topk/--out/--llm/--limit）"
            ),
        }
    }

    let store = Store::open(&db)?;
    let text = std::fs::read_to_string(&questions_path)?;
    let mut questions: Vec<Question> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    if let Some(n) = limit {
        questions.truncate(n);
    }

    // --llm 模式：config.toml 取 default provider（真调 API，花钱）
    let llm: Option<(Arc<OpenAiClient>, agent_providers::ProviderConfig)> = if with_llm {
        let cfg = Config::load(std::path::Path::new("config.toml"))?;
        let pc = cfg.default_provider()?.clone();
        println!(
            "⚠ LLM 模式：真调 API（{} / {}），每题 2 次调用（回答 + judge）",
            pc.name, pc.model
        );
        Some((Arc::new(OpenAiClient::new(pc.clone())?), pc))
    } else {
        None
    };

    // 判定基准用 source_path 文件名而非 note 标题：标题是导入时 LLM 重写的
    // （不稳定），source_path 才是稳定可复现的锚点。
    let note2src: std::collections::HashMap<i64, String> = store
        .list_notes(None, usize::MAX)?
        .into_iter()
        .filter_map(|n| {
            let src = store.get_note(n.id).ok().flatten()?.source_path?;
            let file = std::path::Path::new(&src).file_stem()?.to_str()?.to_owned();
            Some((n.id, file))
        })
        .collect();

    let mut rows: Vec<Row> = Vec::new();
    for q in &questions {
        let t0 = Instant::now();
        let hits = store.search_chunks(&q.q, None, topk)?;
        let elapsed = t0.elapsed().as_millis();
        let hit = hits.iter().position(|h| {
            let anchor = note2src.get(&h.note_id).map(String::as_str).unwrap_or("");
            q.expect.iter().any(|e| anchor.contains(e))
        });
        let top_titles: Vec<String> = hits
            .iter()
            .map(|h| {
                let anchor = note2src
                    .get(&h.note_id)
                    .map(String::as_str)
                    .unwrap_or(&h.note_title);
                let title: String = anchor.chars().take(28).collect();
                format!(
                    "{}({})",
                    title,
                    h.heading.chars().take(14).collect::<String>()
                )
            })
            .collect();

        let mut row = Row {
            id: q.id,
            q: q.q.clone(),
            hit,
            top_titles,
            elapsed_ms: elapsed,
            answer: None,
            cited_sources: Vec::new(),
            score: None,
            hallucination: None,
            judge_reason: None,
            cost: 0.0,
        };

        // LLM 模式：完整 RAG 回答 + judge 评分
        if let Some((client, pc)) = &llm {
            eprintln!("[{}/{}] {} …", rows.len() + 1, questions.len(), q.q);
            match run_llm_eval(client, pc, q, &hits).await {
                Ok((answer, cited, usage)) => {
                    row.answer = Some(answer.clone());
                    row.cited_sources = cited;
                    row.cost += estimate_cost(pc, &usage);
                    match judge_answer(client, pc, q, &answer).await {
                        Ok((score, hallucination, reason, usage)) => {
                            row.score = Some(score);
                            row.hallucination = Some(hallucination);
                            row.judge_reason = Some(reason);
                            row.cost += estimate_cost(pc, &usage);
                        }
                        Err(e) => row.judge_reason = Some(format!("judge 失败: {e}")),
                    }
                }
                Err(e) => row.judge_reason = Some(format!("LLM 调用失败: {e}")),
            }
        }
        rows.push(row);
    }

    // 汇总
    let total = rows.len();
    let rank1 = rows.iter().filter(|r| r.hit == Some(0)).count();
    let top3 = rows.iter().filter(|r| matches!(r.hit, Some(0..=2))).count();
    let topk_hits = rows.iter().filter(|r| r.hit.is_some()).count();
    let avg_ms = rows.iter().map(|r| r.elapsed_ms).sum::<u128>() / total.max(1) as u128;

    let mut md = String::new();
    md.push_str(&format!(
        "# 检索评测报告\n\n\
         - 题集: {}（{} 题）｜ 检索范围: 全库 chunk 级 FTS（top{topk}）\n\
         - 汇总: **rank1 命中 {rank1}/{total}** ｜ top3 命中 {top3}/{total} ｜ \
         top{topk} 命中 {topk_hits}/{total} ｜ 平均耗时 {avg_ms}ms\n\n\
         | # | 问题 | 结果 | 命中 rank |\n|---|---|---|---|\n",
        questions_path.display(),
        total
    ));
    for r in &rows {
        let (result, rank): (&str, String) = match r.hit {
            Some(0) => ("✓", "1".to_owned()),
            Some(n) => ("△", (n + 1).to_string()),
            None => ("✗", "-".to_owned()),
        };
        let llm_col = if llm.is_some() {
            match (r.score, r.hallucination) {
                (Some(s), Some(h)) => format!(" | {s}/10{}", if h { " ⚠幻觉" } else { "" }),
                _ => " | -".to_owned(),
            }
        } else {
            String::new()
        };
        md.push_str(&format!(
            "| {} | {} | {result} | {rank}{llm_col} |\n",
            r.id, r.q
        ));
    }

    md.push_str("\n## Miss / 低 rank 题目分析\n\n");
    for r in rows.iter().filter(|r| !matches!(r.hit, Some(0..=2))) {
        md.push_str(&format!("- **#{id} {}** — 实际 top:\n", r.q, id = r.id));
        for (i, t) in r.top_titles.iter().enumerate() {
            md.push_str(&format!("  - [{}] {t}\n", i + 1));
        }
    }

    // LLM 模式汇总与明细
    if llm.is_some() {
        let scored: Vec<_> = rows.iter().filter(|r| r.score.is_some()).collect();
        let avg_score = scored.iter().map(|r| r.score.unwrap()).sum::<i64>() as f64
            / scored.len().max(1) as f64;
        let halluc = rows
            .iter()
            .filter(|r| r.hallucination == Some(true))
            .count();
        let total_cost: f64 = rows.iter().map(|r| r.cost).sum();
        let no_retrieval_hit = scored
            .iter()
            .filter(|r| !matches!(r.hit, Some(0..=2)))
            .filter(|r| r.score.unwrap() >= 6)
            .count();
        md.push_str(&format!(
            "\n## LLM 评测汇总\n\n\
             - 平均 judge 评分: **{avg_score:.1}/10**（{}/{} 题）\n\
             - 幻觉判定: {halluc} 题\n\
             - 检索 miss（top3 外）仍答对（≥6 分）: {no_retrieval_hit} 题——\
             说明部分问题模型可凭自身知识兜底，检索价值需按题分析\n\
             - 总成本: ¥{total_cost:.4}\n\n\
             ### 每题明细\n\n",
            scored.len(),
            rows.len()
        ));
        for r in rows
            .iter()
            .filter(|r| r.score.is_some() || r.judge_reason.is_some())
        {
            md.push_str(&format!(
                "#### #{} {}（检索 {}，{:.1} 分，¥{:.4}）\n\n",
                r.id,
                r.q,
                match r.hit {
                    Some(0) => "rank1".into(),
                    Some(n) => format!("rank{}", n + 1),
                    None => "miss".into(),
                },
                r.score.map(|s| s as f64).unwrap_or(0.0),
                r.cost
            ));
            if let Some(reason) = &r.judge_reason {
                md.push_str(&format!("- judge: {reason}\n"));
            }
            if let Some(a) = &r.answer {
                let preview: String = a.chars().take(300).collect();
                md.push_str(&format!(
                    "- 回答摘录: {preview}{}\n",
                    if a.chars().count() > 300 { "…" } else { "" }
                ));
            }
            if !r.cited_sources.is_empty() {
                md.push_str(&format!("- 引用: {}\n", r.cited_sources.join("、")));
            }
            md.push('\n');
        }
    }

    std::fs::create_dir_all(out_path.parent().unwrap_or(std::path::Path::new(".")))?;
    std::fs::write(&out_path, &md)?;
    println!(
        "评测完成: rank1 {rank1}/{total}, top3 {top3}/{total}, top{topk} {topk_hits}/{total}, 平均 {avg_ms}ms"
    );
    println!("报告已写入 {}", out_path.display());
    Ok(())
}

/// LLM 模式单题：RAG 回答（top 片段为上下文）→ 解析回答中的 [n] 引用。
async fn run_llm_eval(
    client: &OpenAiClient,
    pc: &agent_providers::ProviderConfig,
    q: &Question,
    hits: &[storage::ChunkHit],
) -> anyhow::Result<(String, Vec<String>, agent_core::Usage)> {
    let context: String = hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let preview: String = h.content.chars().take(800).collect();
            format!("[{}] {} > {}\n{preview}", i + 1, h.note_title, h.heading)
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    let messages = [
        Message::system(
            "你是学习导师。仅依据提供的资料回答问题，在关键事实后用 [n] 标注引用；\
             资料不足时明确说明，不要编造。",
        ),
        Message::user(format!("问题：{}\n\n资料：\n{context}", q.q)),
    ];
    let resp = client.chat(&messages, &[]).await?;

    // 解析回答中的 [n] 引用 → 对应来源
    let cited: Vec<String> = extract_ref_numbers(&resp.content)
        .into_iter()
        .filter_map(|n| hits.get(n.saturating_sub(1)))
        .map(|h| format!("{}({})", h.note_title, h.heading))
        .collect();

    Ok((resp.content, cited, resp.usage))
}

/// judge：问题 + 回答 → score 0-10 / 幻觉判定 / 一句话理由。
async fn judge_answer(
    client: &OpenAiClient,
    pc: &agent_providers::ProviderConfig,
    q: &Question,
    answer: &str,
) -> anyhow::Result<(i64, bool, String, agent_core::Usage)> {
    let prompt = format!(
        "判定以下学习问答的质量。\n问题：{}\nAI 回答：{}\n\n\
         评分标准（0-10）：直接且正确回答了问题给 8-10；方向正确但有遗漏或\
         部分错误给 5-7；答非所问、明显错误给 0-4。编造资料中不存在的具体事实\
         记 hallucination=true。\n\
         输出 JSON：{{\"score\": 0, \"hallucination\": false, \"reason\": \"一句话\"}}",
        q.q,
        answer.chars().take(1500).collect::<String>()
    );
    let messages = [
        Message::system("只输出 JSON，不要 markdown 代码块。"),
        Message::user(&prompt),
    ];
    let resp = client.chat(&messages, &[]).await?;
    let text = resp.content.trim().to_owned();
    let v: serde_json::Value = serde_json::from_str(&text).or_else(|_| {
        agent_core::first_json_block(&text)
            .and_then(|b| serde_json::from_str(b).ok())
            .ok_or_else(|| anyhow::anyhow!("judge JSON 解析失败"))
    })?;
    Ok((
        v.get("score").and_then(|s| s.as_i64()).unwrap_or(0),
        v.get("hallucination")
            .and_then(|s| s.as_bool())
            .unwrap_or(false),
        v.get("reason")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_owned(),
        resp.usage,
    ))
}

/// 从回答文本提取 [n] 引用编号（复用 [数字] 形态）。
fn extract_ref_numbers(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1
                && j < bytes.len()
                && bytes[j] == b']'
                && let Ok(n) = text[i + 1..j].parse::<usize>()
                && n > 0
            // 资料编号从 1 起，arr[0] 这类下标不算引用
            {
                out.push(n);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::extract_ref_numbers;

    #[test]
    fn refs_extracted_and_deduped() {
        assert_eq!(extract_ref_numbers("根据[1]所述[2][1]结束"), vec![1, 2]);
        assert!(extract_ref_numbers("没有引用").is_empty());
        assert!(extract_ref_numbers("数组下标 arr[0] 不算引用").is_empty());
    }
}
