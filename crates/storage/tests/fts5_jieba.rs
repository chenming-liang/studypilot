//! M0 风险验证实验：rusqlite `bundled` feature 下 FTS5 + jieba 中文检索。
//!
//! 验证点（对应决策 D1，机制见 _personal/核心代码逻辑.md）：
//! 1. bundled 构建确实带 FTS5；
//! 2. 默认 unicode61 tokenizer 把连续中文当一个 token，直接 MATCH 子词不可用；
//! 3. 入库/查询都过同一条 jieba 分词管线后，中文检索可命中；
//! 4. MATCH 词加双引号可规避特殊字符导致的语法错误；
//! 5. AND 零命中时降级 OR，按「命中词数 + bm25」排序可行。
//!
//! 说明：jieba 自身的切词质量不是被测对象；测试中凡依赖具体切词结果的断言，
//! 要么手工给出预分词文本（等价于管线输出），要么从同一条管线的输出取查询词。

use jieba_rs::Jieba;
use rusqlite::Connection;

/// 分词管线：入库与查询共用（决策 D1：必须同一条管线）。
fn segment(jieba: &Jieba, text: &str) -> String {
    jieba
        .cut(text, true)
        .iter()
        .map(|t| t.word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 拼 MATCH 查询串：每个词加双引号（决策 D1 实现细则）。
fn match_query<S: AsRef<str>>(terms: &[S]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.as_ref().replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// OR 降级用的 MATCH 串（FTS5 多词默认 AND，OR 必须显式连接）。
fn match_query_or<S: AsRef<str>>(terms: &[S]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.as_ref().replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[test]
fn bundled_build_has_fts5() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE VIRTUAL TABLE t USING fts5(x)", [])
        .expect("bundled rusqlite 应自带 FTS5");
}

#[test]
fn unicode61_treats_continuous_chinese_as_one_token() {
    // 不分词直接入库：连续中文成为单个 token
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE VIRTUAL TABLE t USING fts5(content)", [])
        .unwrap();
    conn.execute("INSERT INTO t VALUES ('虚拟内存管理')", [])
        .unwrap();

    // 高频 2 字子词查不到 —— 证明预分词是必需的（D1 理由）
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM t WHERE t MATCH '\"内存\"'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 0, "unicode61 下连续中文应为一个 token，子词不可命中");

    // 整串匹配才命中
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM t WHERE t MATCH '\"虚拟内存管理\"'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1);
}

/// 预分词入库后，多字/双字词均可命中，且含特殊字符的词不炸语法。
#[test]
fn presegmented_ingest_and_quoted_query() {
    // 手工预分词文本 ≡ 管线输出形态
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE VIRTUAL TABLE t USING fts5(content)", [])
        .unwrap();
    conn.execute(
        "INSERT INTO t VALUES ('虚拟 内存 页面置换 c++ move 语义')",
        [],
    )
    .unwrap();

    for term in ["虚拟", "内存", "页面置换", "c++", "move"] {
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM t WHERE t MATCH ?1",
                [match_query(&[term])],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("term {term:?} 应正常匹配: {e}"));
        assert_eq!(hits, 1, "term {term:?} 应回命中 1 条");
    }
}

/// 入库与查询过同一条 jieba 管线：文档自身切出的词反查必命中。
#[test]
fn same_pipeline_on_both_sides() {
    let jieba = Jieba::new();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE VIRTUAL TABLE t USING fts5(content)", [])
        .unwrap();

    let doc = "操作系统的虚拟内存通过页面置换算法管理有限的物理内存";
    conn.execute(
        "INSERT INTO t(rowid, content) VALUES (1, ?1)",
        [&segment(&jieba, doc)],
    )
    .unwrap();

    // 查询词取自同一条管线的输出（过滤单字与纯标点），逐个反查都应命中
    let tokens: Vec<String> = jieba
        .cut(doc, true)
        .iter()
        .map(|t| t.word)
        .filter(|t| t.chars().count() > 1 && t.chars().any(char::is_alphanumeric))
        .map(String::from)
        .collect();
    assert!(
        tokens.len() >= 3,
        "jieba 切出的多字词应不少于 3 个，实际: {tokens:?}"
    );
    for tok in &tokens {
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM t WHERE t MATCH ?1",
                [match_query(std::slice::from_ref(tok))],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("token {tok:?} 反查失败: {e}"));
        assert_eq!(hits, 1, "token {tok:?} 应命中原文档");
    }
}

/// AND 零命中 → 降级 OR，按「命中词数 desc」排序，未命中任何词的文档不出现（D1 细则）。
#[test]
fn and_zero_hits_falls_back_to_or_ranking() {
    // 预分词文本，「命中词数」= 文档命中的查询词个数（非词频）
    let docs: [&str; 6] = [
        "rust 所有权 移动语义 借用检查",       // 所有权
        "操作系统 虚拟内存 页面置换 物理内存", // 虚拟内存
        "rust 所有权 保证 内存安全",           // 所有权
        "csapp virtual memory 第 9 章",        // 无命中
        "结构体 与 所有权 的 关系",            // 所有权
        "虚拟内存 分区 与 所有权 模型 对比",   // 两词都命中
    ];
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE VIRTUAL TABLE notes_fts USING fts5(content)", [])
        .unwrap();
    for (i, doc) in docs.iter().enumerate() {
        conn.execute(
            "INSERT INTO notes_fts(rowid, content) VALUES (?1, ?2)",
            rusqlite::params![(i + 1) as i64, doc],
        )
        .unwrap();
    }

    let terms = ["所有权", "虚拟内存", "页面置换"];

    // 先 AND：没有任何文档同时含三词，零命中触发降级
    let mut stmt = conn
        .prepare("SELECT rowid FROM notes_fts WHERE notes_fts MATCH ?")
        .unwrap();
    let and_hits: Vec<i64> = stmt
        .query_map([match_query(&terms)], |r| r.get(0))
        .unwrap()
        .flatten()
        .collect();
    assert!(and_hits.is_empty(), "跨主题多词 AND 应零命中");

    // OR 降级：命中词数 desc，bm25 asc 作次级排序
    let ranked = search_or_ranked(&conn, &terms);
    assert_eq!(ranked.len(), 5, "doc4 未命中任何词，不应出现在结果里");
    assert!(
        ranked[..2].iter().all(|(_, cnt)| *cnt == 2),
        "命中两个词的 doc2/doc6 应排前二，实际: {ranked:?}"
    );
    assert!(
        ranked[2..].iter().all(|(_, cnt)| *cnt == 1),
        "其余文档命中词数均为 1"
    );
    // 同命中数内按 bm25 升序（bm25 越小越相关）
    let bm25 = fetch_bm25(&conn, &terms);
    for w in ranked[1..].windows(2) {
        assert!(bm25[&w[0].0] <= bm25[&w[1].0], "同级内应按 bm25 升序");
    }
}

/// OR 降级检索：对每词单独统计命中数（FTS5 不直接提供，见 D1「工作量勿低估」），
/// 再全量取回 bm25 合并排序：命中词数 desc → bm25 asc。
fn search_or_ranked(conn: &Connection, terms: &[&str]) -> Vec<(i64, usize)> {
    let mut hit_counts: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    for term in terms {
        let sql = format!(
            "SELECT rowid FROM notes_fts WHERE notes_fts MATCH '{}'",
            match_query(std::slice::from_ref(term))
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
        for id in rows.flatten() {
            *hit_counts.entry(id).or_default() += 1;
        }
    }

    let bm25 = fetch_bm25(conn, terms);
    let mut ranked: Vec<(i64, usize)> = bm25
        .keys()
        .filter_map(|id| Some((*id, *hit_counts.get(id)?)))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(bm25[&a.0].total_cmp(&bm25[&b.0])));
    ranked
}

fn fetch_bm25(conn: &Connection, terms: &[&str]) -> std::collections::HashMap<i64, f64> {
    let mut stmt = conn
        .prepare("SELECT rowid, bm25(notes_fts) FROM notes_fts WHERE notes_fts MATCH ?")
        .unwrap();
    let rows = stmt
        .query_map([match_query_or(terms)], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })
        .unwrap();
    rows.flatten().collect()
}
