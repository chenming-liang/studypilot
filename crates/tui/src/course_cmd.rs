//! `/course` 命令解析（纯函数，可离线单测）。
//!
//! 语法：`/course [-list | <课程|all> | -new <名> | -delete <名>]`
/// parse_course_action 的结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CourseAction {
    /// 列出全部课程与当前分区
    List,
    /// 切换到已有课程或 all
    Switch(String),
    /// 新建课程
    Create(String),
    /// 删除课程（已校验存在且非 all）
    Delete(String),
    /// 用法错误（信息即帮助）
    Invalid(String),
}

pub(crate) fn parse_course_action(arg: &str, known: &[String]) -> CourseAction {
    let arg = arg.trim();
    if arg.is_empty() || arg == "-list" {
        return CourseAction::List;
    }

    let tokens: Vec<&str> = arg.split_whitespace().collect();
    match tokens.as_slice() {
        // -new / -delete：旗标后的全部 token 重新拼接为课程名（允许多词）
        ["-new", rest @ ..] if !rest.is_empty() => CourseAction::Create(rest.join(" ")),
        ["-new"] => CourseAction::Invalid("用法: /course -new <课程名>".into()),
        ["-delete", rest @ ..] if !rest.is_empty() => {
            let name = rest.join(" ");
            if name == "all" {
                CourseAction::Invalid("all 是全局分区，不能删除".into())
            } else if known.iter().any(|k| k == &name) {
                CourseAction::Delete(name)
            } else {
                CourseAction::Invalid(format!("未知课程 `{name}`，无法删除"))
            }
        }
        ["-delete"] => CourseAction::Invalid("用法: /course -delete <课程名>".into()),
        // 切换：整段 join 后精确匹配已知列表（或 all）——多词课程名如
        // "程序设计训练（Rust 语言）" 靠 known 列表消歧
        _ => {
            let name = tokens.join(" ");
            if name == "all" {
                CourseAction::Switch("all".into())
            } else if known.iter().any(|k| k == &name) {
                CourseAction::Switch(name)
            } else {
                let names: Vec<&str> = std::iter::once("all")
                    .chain(known.iter().map(String::as_str))
                    .collect();
                CourseAction::Invalid(format!(
                    "未知课程 `{name}`。可用: {}，或 /course -new <名> 新建",
                    names.join(", ")
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        vec!["rust".into(), "csapp".into()]
    }

    #[test]
    fn empty_or_list_flag_lists_all() {
        assert_eq!(parse_course_action("", &known()), CourseAction::List);
        assert_eq!(parse_course_action("   ", &known()), CourseAction::List);
        assert_eq!(parse_course_action("-list", &known()), CourseAction::List);
    }

    #[test]
    fn switches_to_known_or_all() {
        assert_eq!(
            parse_course_action("rust", &known()),
            CourseAction::Switch("rust".into())
        );
        assert_eq!(
            parse_course_action("all", &known()),
            CourseAction::Switch("all".into())
        );
        assert_eq!(
            parse_course_action(" csapp ", &known()),
            CourseAction::Switch("csapp".into())
        );
    }

    #[test]
    fn creates_single_word_course() {
        assert_eq!(
            parse_course_action("-new ml", &known()),
            CourseAction::Create("ml".into())
        );
        assert_eq!(
            parse_course_action("-new machine-learning", &known()),
            CourseAction::Create("machine-learning".into())
        );
    }

    /// 多词课程名合法（真实语料如"程序设计训练（Rust 语言）"含空格）。
    #[test]
    fn multi_word_names_are_accepted() {
        let mut k = known();
        k.push("程序设计训练（Rust 语言）".into());
        // 切换多词课程
        assert_eq!(
            parse_course_action("程序设计训练（Rust 语言）", &k),
            CourseAction::Switch("程序设计训练（Rust 语言）".into())
        );
        // -new 多词
        assert_eq!(
            parse_course_action("-new machine learning", &known()),
            CourseAction::Create("machine learning".into())
        );
        // -delete 多词
        assert_eq!(
            parse_course_action("-delete 程序设计训练（Rust 语言）", &k),
            CourseAction::Delete("程序设计训练（Rust 语言）".into())
        );
        // 多词但不在 known → 明确报"未知课程"（拼错或未建）
        match parse_course_action("程序设计训练 Rust", &k) {
            CourseAction::Invalid(msg) => assert!(msg.contains("未知课程"), "{msg}"),
            other => panic!("应为 Invalid，实际 {other:?}"),
        }
    }

    #[test]
    fn bare_flags_are_usage_errors() {
        assert!(matches!(
            parse_course_action("-new", &known()),
            CourseAction::Invalid(_)
        ));
        assert!(matches!(
            parse_course_action("-delete", &known()),
            CourseAction::Invalid(_)
        ));
    }

    #[test]
    fn delete_validates_target() {
        assert_eq!(
            parse_course_action("-delete rust", &known()),
            CourseAction::Delete("rust".into())
        );
        // all 分区不可删
        match parse_course_action("-delete all", &known()) {
            CourseAction::Invalid(msg) => assert!(msg.contains("不能删除"), "{msg}"),
            other => panic!("应为 Invalid，实际 {other:?}"),
        }
        // 不存在的课程不可删
        match parse_course_action("-delete os", &known()) {
            CourseAction::Invalid(msg) => assert!(msg.contains("未知课程"), "{msg}"),
            other => panic!("应为 Invalid，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_single_word_suggests_new() {
        match parse_course_action("os", &known()) {
            CourseAction::Invalid(msg) => {
                assert!(msg.contains("未知课程 `os`"), "{msg}");
                assert!(msg.contains("/course -new"), "{msg}");
            }
            other => panic!("应为 Invalid，实际 {other:?}"),
        }
    }
}

/// `/review` 参数解析结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReviewSpec {
    pub course: String,
    /// 概念关键词（空 = 课程名本身）
    pub scope: String,
    pub n: usize,
}

/// 解析 `/review <课程> [概念] [--n 数量]`。
///
/// 课程与概念都是自由文本、边界无法靠空格切分，策略：
/// ① 先摘出 `--n <数>` 旗标；② 剩余整段与已知课程做**最长前缀匹配**
/// （`rest == 课程名` 或 `rest` 以 `课程名 + 空格` 开头），匹配到则课程名
/// 之后的部分为概念；无匹配则整段视为课程名（由调用方报"不存在"）。
pub(crate) fn parse_review_action(arg: &str, known: &[String]) -> Result<ReviewSpec, String> {
    let mut n = 5usize;
    let mut rest_parts: Vec<&str> = Vec::new();
    let mut it = arg.split_whitespace().peekable();
    while let Some(tok) = it.next() {
        if tok == "--n" {
            if let Some(v) = it.next()
                && let Ok(k) = v.parse::<usize>()
                && k > 0
            {
                n = k;
            }
        } else if !tok.starts_with("--") {
            rest_parts.push(tok);
        }
    }
    let rest = rest_parts.join(" ");
    if rest.is_empty() {
        return Err("用法: /review <课程名> [概念关键词] [--n 数量]".into());
    }

    // 最长已知课程前缀匹配
    let hit = known
        .iter()
        .filter(|cn| *cn == &rest || rest.starts_with(&format!("{cn} ")))
        .max_by_key(|cn| cn.len())
        .cloned();
    match hit {
        Some(course) => {
            let scope = rest[course.len()..].trim().to_owned();
            let scope = if scope.is_empty() {
                course.clone()
            } else {
                scope
            };
            Ok(ReviewSpec { course, scope, n })
        }
        None => Err(format!(
            "课程 `{rest}` 不存在。可用: {}",
            std::iter::once("all")
                .chain(known.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;

    fn known() -> Vec<String> {
        vec!["rust".into(), "程序设计训练（Rust 语言）".into()]
    }

    #[test]
    fn single_word_course_and_concept() {
        let s = parse_review_action("rust ownership --n 3", &known()).unwrap();
        assert_eq!(s.course, "rust");
        assert_eq!(s.scope, "ownership");
        assert_eq!(s.n, 3);
    }

    #[test]
    fn multi_word_course_consumes_full_name() {
        // 旧实现会劈成课程="程序设计训练（Rust"、概念="语言）"——回归保护
        let s = parse_review_action("程序设计训练（Rust 语言） --n 5", &known()).unwrap();
        assert_eq!(s.course, "程序设计训练（Rust 语言）");
        assert_eq!(s.scope, "程序设计训练（Rust 语言）"); // 无概念时 scope=课程名
    }

    #[test]
    fn multi_word_course_with_concept() {
        let s = parse_review_action("程序设计训练（Rust 语言） ownership", &known()).unwrap();
        assert_eq!(s.course, "程序设计训练（Rust 语言）");
        assert_eq!(s.scope, "ownership");
        assert_eq!(s.n, 5);
    }

    #[test]
    fn longest_prefix_wins() {
        // 两个已知课程互为前缀时取最长
        let k = vec!["os".into(), "os 内存".into()];
        let s = parse_review_action("os 内存 置换", &k).unwrap();
        assert_eq!(s.course, "os 内存");
        assert_eq!(s.scope, "置换");
    }

    #[test]
    fn flag_order_is_flexible() {
        let s = parse_review_action("--n 7 rust", &known()).unwrap();
        assert_eq!(s.course, "rust");
        assert_eq!(s.n, 7);
    }

    #[test]
    fn empty_arg_is_usage_error() {
        assert!(parse_review_action("", &known()).is_err());
        assert!(parse_review_action("--n 5", &known()).is_err());
    }

    #[test]
    fn unknown_course_reports_available() {
        let e = parse_review_action("不存在的课", &known()).unwrap_err();
        assert!(e.contains("不存在"), "{e}");
        assert!(e.contains("程序设计训练"), "{e}");
    }
}
