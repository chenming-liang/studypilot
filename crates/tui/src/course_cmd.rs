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
        ["-new", name] => CourseAction::Create((*name).to_owned()),
        ["-new"] => CourseAction::Invalid("用法: /course -new <课程名>".into()),
        ["-new", rest @ ..] => CourseAction::Invalid(format!(
            "课程名不能含空格：『{}』被解析为多段。试试 /course -new {}",
            rest.join(" "),
            rest.join("-")
        )),
        ["-delete", name] => {
            if *name == "all" {
                CourseAction::Invalid("all 是全局分区，不能删除".into())
            } else if known.iter().any(|k| k == name) {
                CourseAction::Delete((*name).to_owned())
            } else {
                CourseAction::Invalid(format!("未知课程 `{name}`，无法删除"))
            }
        }
        ["-delete"] => CourseAction::Invalid("用法: /course -delete <课程名>".into()),
        [name] if *name == "all" => CourseAction::Switch("all".into()),
        [name] => {
            if known.iter().any(|k| k == name) {
                CourseAction::Switch((*name).to_owned())
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
        _ => {
            // 多词参数：区分带旗标与不带旗标的提示
            if tokens[0].starts_with('-') {
                CourseAction::Invalid(format!(
                    "`{}` 后的课程名不能含空格（被解析为多段）",
                    tokens[0]
                ))
            } else {
                CourseAction::Invalid(format!(
                    "无法识别 `/course {arg}`：课程名不能含空格。查看全部用 /course -list"
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

    /// 回归：多词课程名应给出明确用法错误并建议连字符形式。
    #[test]
    fn multi_word_new_gets_clear_usage_error() {
        match parse_course_action("-new machine learning", &known()) {
            CourseAction::Invalid(msg) => {
                assert!(msg.contains("不能含空格"), "{msg}");
                assert!(msg.contains("machine-learning"), "应给出连字符建议: {msg}");
            }
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
