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
