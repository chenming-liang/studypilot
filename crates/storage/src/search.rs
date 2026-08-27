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

    // 第一跳：AND（隐式多词 AND）
    let mut hits = query_fts(conn, &match_query(&terms), course_id, Some(top_k))?;
    if hits.len() >= top_k {
        for hit in &mut hits {
            hit.hit_terms = terms.len();
        }
        return Ok(hits);
    }

    // 降级 OR：全量取回后按命中词数+bm25 合并排序截断
    let mut or_hits = query_fts(conn, &match_query_or(&terms), course_id, None)?;
    let mut counts = hit_counts_per_term(conn, &terms, course_id)?;
    for hit in &mut or_hits {
        hit.hit_terms = counts.remove(&hit.note_id).unwrap_or(0);
    }
    or_hits.sort_by(|a, b| {
        b.hit_terms
            .cmp(&a.hit_terms)
            .then(a.rank.total_cmp(&b.rank))
    });
    or_hits.truncate(top_k);
    Ok(or_hits)
}

/// 执行一次 FTS 查询；`limit = None` 表示不限制。
fn query_fts(
    conn: &Mutex<Connection>,
    match_expr: &str,
    course_id: Option<i64>,
    limit: Option<usize>,
) -> Result<Vec<SearchHit>> {
    let sql = build_query_sql(course_id.is_some());
    let conn = conn.lock().unwrap();
    let mut stmt = conn.prepare(&sql)?;

    let mut values = vec![Value::Text(match_expr.to_owned())];
    if let Some(cid) = course_id {
        values.push(Value::Integer(cid));
    }

    let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |r| {
        Ok(SearchHit {
            note_id: r.get(0)?,
            title: r.get(1)?,
            course_id: r.get(2)?,
            rank: r.get(3)?,
            hit_terms: 0,
            content: r.get(4)?,
        })
    })?;

    let mut hits: Vec<SearchHit> = rows.flatten().collect();
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
        "SELECT n.id, n.title, n.course_id, bm25(notes_fts), n.content
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

/// chunk 级搜索：AND → OR 降级，纯 bm25 排序（去掉 N 次 COUNT 查询）。
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

    // 第一跳：AND
    let hits = query_chunks(conn, &match_query(&terms), course_id, top_k)?;
    if hits.len() >= top_k {
        return Ok(hits);
    }

    // 降级 OR：纯 bm25 排序（FTS5 OR 模式下匹配多词的文档 bm25 天然更高）
    let or_hits = query_chunks(conn, &match_query_or(&terms), course_id, top_k)?;
    if or_hits.len() > hits.len() {
        return Ok(or_hits);
    }
    Ok(hits)
}

fn query_chunks(
    conn: &Mutex<Connection>,
    match_expr: &str,
    course_id: Option<i64>,
    limit: usize,
) -> Result<Vec<ChunkHit>> {
    let course_clause = course_id
        .is_some()
        .then(|| " AND (n.course_id = ?2 OR n.course_id IS NULL)".to_string());
    let sql = format!(
        "SELECT c.id, c.note_id, n.title, c.heading, c.content, bm25(note_chunks_fts)
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
        Ok(ChunkHit {
            chunk_id: r.get(0)?,
            note_id: r.get(1)?,
            note_title: r.get(2)?,
            heading: r.get(3)?,
            content: r.get(4)?,
            rank: r.get(5)?,
        })
    })?;
    Ok(rows.flatten().take(limit).collect())
}
