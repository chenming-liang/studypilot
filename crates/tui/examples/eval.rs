//! 检索评测器（离线，不调 LLM、零 API 成本）。
//!
//! 用法：`cargo run --release -p tui --example eval -- \
//!        --db data/mynotes.db --questions eval/questions.jsonl \
//!        --topk 5 --out eval/report-baseline.md`
//!
//! 对每题跑 chunk 级 FTS 检索（course=None 全库），判定 expect 笔记是否出现在
//! top 结果里；输出 markdown 报告：汇总命中率 + 每题明细 + miss 题的实际命中
//! （供人工分析 miss 原因：词面盲区 / chunk 切分 / 关键词选择）。

use std::path::PathBuf;
use std::time::Instant;

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
}

fn main() -> anyhow::Result<()> {
    let mut db = PathBuf::from("data/mynotes.db");
    let mut questions_path = PathBuf::from("eval/questions.jsonl");
    let mut out_path = PathBuf::from("eval/report-baseline.md");
    let mut topk = 5usize;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--db" => db = PathBuf::from(args.next().expect("--db 需要参数")),
            "--questions" => {
                questions_path = PathBuf::from(args.next().expect("--questions 需要参数"))
            }
            "--out" => out_path = PathBuf::from(args.next().expect("--out 需要参数")),
            "--topk" => topk = args.next().expect("--topk 需要参数").parse()?,
            other => anyhow::bail!("未知参数 {other}（支持 --db/--questions/--topk/--out）"),
        }
    }

    let store = Store::open(&db)?;
    let text = std::fs::read_to_string(&questions_path)?;
    let questions: Vec<Question> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

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
        rows.push(Row {
            id: q.id,
            q: q.q.clone(),
            hit,
            top_titles: hits
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
                .collect(),
            elapsed_ms: elapsed,
        });
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
        md.push_str(&format!("| {} | {} | {result} | {rank} |\n", r.id, r.q));
    }

    md.push_str("\n## Miss / 低 rank 题目分析\n\n");
    for r in rows.iter().filter(|r| !matches!(r.hit, Some(0..=2))) {
        md.push_str(&format!("- **#{id} {}** — 实际 top:\n", r.q, id = r.id));
        for (i, t) in r.top_titles.iter().enumerate() {
            md.push_str(&format!("  - [{}] {t}\n", i + 1));
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
