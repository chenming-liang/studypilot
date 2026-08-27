//! 笔记检索：FTS5 MATCH，AND 不足 top_k 时降级 OR，
//! 按「命中词数 desc + bm25 asc」排序（决策 D1 实现细则，机制经 M0 实验验证）。
//!
//! 范围语义 = 当前课程 ∪ all 区：`course_id` 精确匹配 OR `course_id IS NULL`；
//! `course_id = None` 表示 all 全局区（不加课程过滤）。

use std::collections::HashMap;
use std::sync::Mutex;

use rusqlite::{Connection, types::Value};

use crate::ChunkHit;
use crate::error::Result;
use crate::segment::{cut_tokens, match_query, match_query_or};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub note_id: i64,
    pub title: String,
    pub course_id: Option<i64>,
    /// bm25 分数，越小越相关
    pub rank: f64,
    /// 命中的查询词个数（OR 降级排序主键；AND 直命中时 = 词数）
    pub hit_terms: usize,
    /// 原文全文（引用展示由上层截取片段）
    pub content: String,
}

// ---- 噪声笔记降权（评测基线结论：plan/README/索引类笔记污染排序）----

/// 噪声惩罚量：bm25 为负值域（越小越相关），+PENALTY 把噪声压到正常命中之后。
const NOISE_PENALTY: f64 = 5.0;
/// SQL 候选池放宽倍数：给惩罚重排留出"被噪声压住的正常命中"。
const FETCH_MULTIPLIER: usize = 3;

/// 大纲/计划/索引类文件判定（按 source_path 的文件名，大小写不敏感）。
fn is_noise_source(source_path: Option<&str>) -> bool {
    let Some(p) = source_path else {
        return false;
    };
    let stem = std::path::Path::new(p)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    ["readme", "plan", "index", "syllabus", "agenda"]
        .iter()
        .any(|k| stem.contains(k))
        || p.contains("索引")
        || p.contains("目录")
}

/// 惩罚重排：噪声 rank 加罚，按调整后 bm25 升序截断。
fn penalize_and_sort<T>(hits: &mut Vec<(T, Option<String>)>, top_k: usize) -> Vec<T>
where
    T: HasRank,
{
    for (h, src) in hits.iter_mut() {
        if is_noise_source(src.as_deref()) {
            *h.rank_mut() += NOISE_PENALTY;
        }
    }
    hits.sort_by(|a, b| a.0.rank().total_cmp(&b.0.rank()));
    hits.truncate(top_k);
    std::mem::take(hits).into_iter().map(|(h, _)| h).collect()
}

trait HasRank {
    fn rank(&self) -> f64;
    fn rank_mut(&mut self) -> &mut f64;
}

impl HasRank for ChunkHit {
    fn rank(&self) -> f64 {
        self.rank
    }
    fn rank_mut(&mut self) -> &mut f64 {
        &mut self.rank
    }
}

impl HasRank for SearchHit {
    fn rank(&self) -> f64 {
        self.rank
    }
    fn rank_mut(&mut self) -> &mut f64 {
        &mut self.rank
    }
}

pub fn run(
    conn: &Mutex<Connection>,
    query: &str,
    course_id: Option<i64>,
    top_k: usize,
) -> Result<Vec<SearchHit>> {
    let terms = cut_tokens(query);
    if terms.is_empty() || top_k == 0 {
        return Ok(Vec::new());
    }

    // 候选池放宽（FETCH_MULTIPLIER），惩罚重排在 Rust 层做
    let fetch_limit = top_k * FETCH_MULTIPLIER;
    let mut hits = query_fts(conn, &match_query(&terms), course_id, Some(fetch_limit))?;

    if hits.len() >= top_k {
        for hit in &mut hits {
            hit.0.hit_terms = terms.len();
        }
        return Ok(penalize_and_sort(&mut hits, top_k));
    }

    // 降级 OR：全量取回后按「命中词数 desc → 非噪声优先 → bm25 asc」排序截断
    let mut or_hits = query_fts(conn, &match_query_or(&terms), course_id, None)?;
    let mut counts = hit_counts_per_term(conn, &terms, course_id)?;
    for (hit, src) in &mut or_hits {
        hit.hit_terms = counts.remove(&hit.note_id).unwrap_or(0);
        if is_noise_source(src.as_deref()) {
            hit.rank += NOISE_PENALTY;
        }
    }
    or_hits.sort_by(|a, b| {
        b.0.hit_terms
            .cmp(&a.0.hit_terms)
            .then(b.1.is_none().cmp(&a.1.is_none()))
            .then(a.0.rank.total_cmp(&b.0.rank))
    });
    or_hits.truncate(top_k);
    Ok(or_hits.into_iter().map(|(h, _)| h).collect())
}

/// 执行一次 FTS 查询；`limit = None` 表示不限制。
/// 返回 (hit, source_path)——source 供噪声降权判定，不外泄。
fn query_fts(
    conn: &Mutex<Connection>,
    match_expr: &str,
    course_id: Option<i64>,
    limit: Option<usize>,
) -> Result<Vec<(SearchHit, Option<String>)>> {
    let sql = build_query_sql(course_id.is_some());
    let conn = conn.lock().unwrap();
    let mut stmt = conn.prepare(&sql)?;

    let mut values = vec![Value::Text(match_expr.to_owned())];
    if let Some(cid) = course_id {
        values.push(Value::Integer(cid));
    }

    let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |r| {
        Ok((
            SearchHit {
                note_id: r.get(0)?,
                title: r.get(1)?,
                course_id: r.get(2)?,
                rank: r.get(3)?,
                hit_terms: 0,
                content: r.get(4)?,
            },
            r.get(5)?,
        ))
    })?;

    let mut hits: Vec<(SearchHit, Option<String>)> = rows.flatten().collect();
    if let Some(limit) = limit {
        hits.truncate(limit);
    }
    Ok(hits)
}

/// AND 与 OR 共用查询骨架；带课程过滤时追加占位符 ?2。
fn build_query_sql(with_course: bool) -> String {
    let course_clause =
        with_course.then(|| " AND (n.course_id = ?2 OR n.course_id IS NULL)".to_string());
    format!(
        "SELECT n.id, n.title, n.course_id, bm25(notes_fts), n.content, n.source_path
         FROM notes_fts JOIN notes n ON n.id = notes_fts.rowid
         WHERE notes_fts MATCH ?1{course}
         ORDER BY bm25(notes_fts)",
        course = course_clause.as_deref().unwrap_or("")
    )
}

/// 对每个词单独 COUNT（FTS5 不直接提供命中词数，D1「工作量勿低估」处）。
fn hit_counts_per_term(
    conn: &Mutex<Connection>,
    terms: &[String],
    course_id: Option<i64>,
) -> Result<HashMap<i64, usize>> {
    let conn = conn.lock().unwrap();
    let mut counts = HashMap::new();
    for term in terms {
        let expr = format!("\"{}\"", term.replace('"', "\"\""));
        let clause = course_id
            .is_some()
            .then(|| " AND (n.course_id = ?2 OR n.course_id IS NULL)".to_string());
        let sql = format!(
            "SELECT n.id FROM notes_fts JOIN notes n ON n.id = notes_fts.rowid
             WHERE notes_fts MATCH ?1{clause}",
            clause = clause.as_deref().unwrap_or("")
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut values = vec![Value::Text(expr)];
        if let Some(cid) = course_id {
            values.push(Value::Integer(cid));
        }
        let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |r| {
            r.get::<_, i64>(0)
        })?;
        for id in rows.flatten() {
            *counts.entry(id).or_default() += 1;
        }
    }
    Ok(counts)
}

// ---- chunk 级检索（M7+ 改进：FTS 降到 chunk 粒度，纯 bm25 排序）----

/// chunk 级搜索：AND → OR 降级，噪声降权后按 bm25 排序。
pub fn run_chunks(
    conn: &Mutex<Connection>,
    query: &str,
    course_id: Option<i64>,
    top_k: usize,
) -> Result<Vec<ChunkHit>> {
    let terms = cut_tokens(query);
    if terms.is_empty() || top_k == 0 {
        return Ok(Vec::new());
    }

    // 候选池放宽 3 倍：AND 池装满才跳过 OR（保证惩罚重排后仍有足够正常命中）
    let fetch_limit = top_k * FETCH_MULTIPLIER;
    let mut hits = query_chunks(conn, &match_query(&terms), course_id, fetch_limit)?;
    if hits.len() < fetch_limit {
        let or_hits = query_chunks(conn, &match_query_or(&terms), course_id, fetch_limit)?;
        if or_hits.len() > hits.len() {
            hits = or_hits;
        }
    }
    Ok(penalize_and_sort(&mut hits, top_k))
}

fn query_chunks(
    conn: &Mutex<Connection>,
    match_expr: &str,
    course_id: Option<i64>,
    limit: usize,
) -> Result<Vec<(ChunkHit, Option<String>)>> {
    let course_clause = course_id
        .is_some()
        .then(|| " AND (n.course_id = ?2 OR n.course_id IS NULL)".to_string());
    let sql = format!(
        "SELECT c.id, c.note_id, n.title, c.heading, c.content, bm25(note_chunks_fts), n.source_path
         FROM note_chunks_fts
         JOIN note_chunks c ON c.id = note_chunks_fts.rowid
         JOIN notes n ON n.id = c.note_id
         WHERE note_chunks_fts MATCH ?1{course}
         ORDER BY bm25(note_chunks_fts)",
        course = course_clause.as_deref().unwrap_or("")
    );
    let conn = conn.lock().unwrap();
    let mut stmt = conn.prepare(&sql)?;
    let mut values = vec![Value::Text(match_expr.to_owned())];
    if let Some(cid) = course_id {
        values.push(Value::Integer(cid));
    }
    let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |r| {
        Ok((
            ChunkHit {
                chunk_id: r.get(0)?,
                note_id: r.get(1)?,
                note_title: r.get(2)?,
                heading: r.get(3)?,
                content: r.get(4)?,
                rank: r.get(5)?,
            },
            r.get(6)?,
        ))
    })?;
    Ok(rows.flatten().take(limit).collect())
}
