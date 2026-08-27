//! jieba 分词管线——入库与查询必须共用这一条（决策 D1）。
//!
//! jieba 自身的切词质量不是被测对象；这里保证的是"同一条管线"。

use jieba_rs::Jieba;
use std::sync::OnceLock;

static JIEBA: OnceLock<Jieba> = OnceLock::new();

fn jieba() -> &'static Jieba {
    JIEBA.get_or_init(Jieba::new)
}

/// 切词并过滤：保留含字母/数字的 token（纯标点剔除）。
/// 单字汉字（堆/栈/值/类）保留——技术概念常用单字词。
/// 入库与查询两侧都用它，保证 FTS 可命中。
pub fn segment(text: &str) -> String {
    cut_tokens(text).join(" ")
}

/// 同一条管线的 token 化形式（去重保序）。
pub fn cut_tokens(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    jieba()
        .cut(text, true)
        .iter()
        .map(|t| t.word)
        // 保留：含字母或数字的 token；剔除纯标点/空白
        // 单字汉字（如"堆""栈""值"）也保留——技术概念常用单字
        .filter(|w| {
            w.chars()
                .any(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&c))
                && !w.chars().all(|c| c.is_whitespace())
        })
        .filter(|w| seen.insert(*w))
        .map(String::from)
        .collect()
}

/// MATCH 查询串：每个词加双引号（D1 细则，防特殊字符语法错误），多词隐式 AND。
pub fn match_query<S: AsRef<str>>(terms: &[S]) -> String {
    quoted_terms(terms).join(" ")
}

/// OR 降级用的 MATCH 串（FTS5 多词默认 AND，OR 必须显式连接，见 M0 实验）。
pub fn match_query_or<S: AsRef<str>>(terms: &[S]) -> String {
    quoted_terms(terms).join(" OR ")
}

fn quoted_terms<S: AsRef<str>>(terms: &[S]) -> Vec<String> {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.as_ref().replace('"', "\"\"")))
        .collect()
}
