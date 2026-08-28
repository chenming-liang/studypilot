//! M3 storage 验收测试：schema、幂等去重、中文检索（范围/降级排序）、
//! session roundtrip（含 tool_calls 原样还原）、删除级联。

use agent_core::{Message, Role, ToolCall};
use storage::{InsertOutcome, NewChunk, NewNote, Store};

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn insert(store: &Store, course: Option<i64>, title: &str, content: &str) -> i64 {
    match store
        .insert_note(NewNote {
            course_id: course,
            title,
            source_path: Some(&format!("/fake/{title}.md")),
            content,
            fts_content: None,
        })
        .unwrap()
    {
        InsertOutcome::Created(n) => n.id,
        InsertOutcome::Duplicate { existing_id, .. } => existing_id,
    }
}

#[test]
fn schema_creates_all_tables() {
    let path = std::env::temp_dir().join(format!(
        "m3-schema-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let _store = Store::open(&path).unwrap();
        // 迁移在 open 时执行；drop 释放连接后用独立只读连接检查 sqlite_master
    }
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','virtual table')")
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .flatten()
        .collect();

    for expected in [
        "courses",
        "notes",
        "notes_fts",
        "concepts",
        "note_concepts",
        "quizzes",
        "questions",
        "attempts",
        "concept_mastery",
        "sessions",
        "messages",
        "usage_log",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "表 {expected} 应存在，实际: {names:?}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn note_dedup_by_content_hash() {
    let store = store();
    let rust = store.get_or_create_course("rust").unwrap();

    let n1 = insert(&store, Some(rust), "第一篇", "虚拟内存是操作系统的核心机制");
    match store
        .insert_note(NewNote {
            course_id: None,
            title: "不同标题但相同内容",
            source_path: None,
            content: "虚拟内存是操作系统的核心机制",
            fts_content: None,
        })
        .unwrap()
    {
        InsertOutcome::Duplicate {
            existing_id,
            existing_title,
            ..
        } => {
            assert_eq!(existing_id, n1);
            // 现有标题应是第一篇的（内容相同、标题不同也命中全局去重）
            assert_eq!(existing_title, "第一篇");
        }
        InsertOutcome::Created(_) => panic!("相同内容应被去重"),
    }

    let got = store.get_note(n1).unwrap().unwrap();
    assert_eq!(got.content_hash.len(), 16);
    assert_eq!(got.course_id, Some(rust));
}

#[test]
fn search_scope_is_course_union_all() {
    let store = store();
    let rust = store.get_or_create_course("rust").unwrap();
    let os = store.get_or_create_course("os").unwrap();

    insert(
        &store,
        Some(rust),
        "r1",
        "Rust 的所有权与借用检查保证内存安全",
    );
    insert(
        &store,
        Some(os),
        "o1",
        "操作系统虚拟内存通过页面置换管理物理内存",
    );
    insert(&store, None, "unclassified", "内存安全随笔：悬垂指针的成因");

    // rust 分区：命中课程内 + all 区未归类，不命中 os
    let hits = store
        .search_notes("内存安全 所有权", Some(rust), 10)
        .unwrap();
    let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
    assert!(
        titles.contains(&"r1") && titles.contains(&"unclassified"),
        "{titles:?}"
    );
    assert!(!titles.contains(&"o1"), "{titles:?}");

    // all 区：不过滤，三门全可命中
    let hits = store.search_notes("内存", None, 10).unwrap();
    assert_eq!(hits.len(), 3);
}

#[test]
fn and_falls_back_to_or_ranked_by_hit_terms() {
    let store = store();
    insert(&store, None, "a", "所有权 移动语义 借用检查");
    insert(&store, None, "b", "虚拟内存 页面置换 物理内存");
    insert(&store, None, "c", "虚拟内存 与 所有权 模型对比"); // 两词都命中
    insert(&store, None, "d", "完全无关的内容");

    // AND 零命中（无文档同时含两词）→ OR 降级
    let hits = store.search_notes("所有权 虚拟内存", None, 10).unwrap();

    assert_eq!(hits[0].title, "c");
    assert_eq!(hits[0].hit_terms, 2);
    assert_eq!(hits.len(), 3, "无关文档不应出现");
    assert!(hits[1..].iter().all(|h| h.hit_terms == 1));
    // 同命中数按 bm25 升序
    for w in hits[1..].windows(2) {
        assert!(w[0].rank <= w[1].rank);
    }
}

#[test]
fn session_roundtrip_preserves_tool_calls_verbatim() {
    let store = store();
    let rust = store.get_or_create_course("rust").unwrap();
    let session = store.create_session(Some("测试会话"), Some(rust)).unwrap();

    let msgs = vec![
        Message::system("你是知识库助手"),
        Message::user("什么是所有权？"),
        Message::assistant_tool_calls(vec![ToolCall::function(
            "call_1",
            "search_notes",
            r#"{"query":"所有权"}"#,
        )]),
        Message::tool_result("call_1", r#"{"hits":[{"note":"02-所有权"}]}"#),
        Message::assistant("所有权是……[1]"),
    ];
    for m in &msgs {
        store.append_message(session, m).unwrap();
    }
    // seq 自动递增且有序；乱序 append 也应按写入顺序还原
    let loaded = store.load_session_messages(session).unwrap();
    assert_eq!(loaded, msgs, "含 tool_calls/tool_call_id 的消息应原样还原");

    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].course_id, Some(rust));
}

#[test]
fn role_values_satisfy_db_check_constraint() {
    let store = store();
    let session = store.create_session(None, None).unwrap();
    for role in [Role::System, Role::User, Role::Assistant, Role::Tool] {
        let msg = match role {
            Role::System => Message::system("s"),
            Role::User => Message::user("u"),
            Role::Assistant => Message::assistant("a"),
            Role::Tool => Message::tool_result("t", "r"),
        };
        store.append_message(session, &msg).unwrap(); // CHECK 违反会在此报错
    }
    assert_eq!(store.load_session_messages(session).unwrap().len(), 4);
}

#[test]
fn delete_note_cascades_fts_index() {
    let store = store();
    let id = insert(&store, None, "待删", "页面置换算法详解 LRU 与 Clock");
    assert!(store.search_notes("页面置换", None, 5).unwrap().len() == 1);

    assert!(store.delete_note(id).unwrap());
    assert!(!store.delete_note(id).unwrap(), "重复删除应返回 false");
    assert!(store.get_note(id).unwrap().is_none());
    assert!(
        store.search_notes("页面置换", None, 5).unwrap().is_empty(),
        "FTS 索引行必须随笔记一起清理"
    );
}

#[test]
fn usage_log_records_and_sums_cost() {
    let store = store();
    store
        .append_usage("deepseek", "deepseek-reasoner", "chat", 98, 188, 0.001_092)
        .unwrap();
    store
        .append_usage("deepseek", "deepseek-chat", "vision", 11, 1, 0.000_012)
        .unwrap();

    // 累计值近似相等（f64 求和）
    let total = store.total_recorded_cost().unwrap();
    assert!((total - 0.001_104).abs() < 1e-9, "{total}");
}

#[test]
fn session_rename_persists() {
    let store = store();
    let id = store.create_session(Some("旧标题"), None).unwrap();
    assert!(store.set_session_title(id, "新标题").unwrap());
    assert!(
        !store.set_session_title(999, "x").unwrap(),
        "不存在应返回 false"
    );
    let title: Option<String> = store
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
        .and_then(|s| s.title);
    assert_eq!(title.as_deref(), Some("新标题"));
}

/// 噪声笔记（plan/README/索引类）降权：内容同质时正常笔记必须排在前面。
/// 对应评测基线的 miss 归因（plan/README 污染排序）。
#[test]
fn noise_sources_ranked_below_real_notes() {
    let store = Store::open_in_memory().unwrap();
    let cid = store.get_or_create_course("c").unwrap();

    // 同样的关键内容 + 各自差异化文字（否则 content_hash 幂等去重会吞掉第二篇）
    for (path, title, extra) in [
        ("course/plan/ch6-plan.md", "课程计划", "本周学习安排与目标"),
        (
            "course/notes/第6章-存储器层级.md",
            "第6章-存储器层级",
            "存储器山与缓存映射机制详解",
        ),
    ] {
        let outcome = store
            .insert_note(NewNote {
                course_id: Some(cid),
                title,
                source_path: Some(path),
                content: &format!("cache 命中率 由局部性决定 {extra}"),
                fts_content: None,
            })
            .unwrap();
        let note = match outcome {
            InsertOutcome::Created(n) => n,
            InsertOutcome::Duplicate { .. } => panic!("不应去重"),
        };
        store
            .insert_chunks(
                note.id,
                &[NewChunk {
                    heading: "cache",
                    content: &format!("cache 命中率 由局部性决定 时间局部性 空间局部性 {extra}"),
                    fts_extra: None,
                }],
            )
            .unwrap();
    }

    let hits = store.search_chunks("cache 命中率 局部性", None, 2).unwrap();
    assert_eq!(hits.len(), 2);
    assert!(
        hits[0].note_title.contains("第6章"),
        "正常笔记应排第一，实际: {}",
        hits[0].note_title
    );
    assert!(
        hits[1].note_title.contains("课程计划"),
        "噪声应被降权到第二"
    );
}

/// NoteBrowser 数据层：标题搜索 + 按 id 批量删除（FTS/chunk 同步清理）。
#[test]
fn title_search_and_batch_delete() {
    let store = store();
    let cid = store.get_or_create_course("rust").unwrap();
    let _ = store.get_or_create_course("csapp").unwrap();
    let mut ids = Vec::new();
    for (title, course) in [
        ("所有权基础", Some(cid)),
        ("所有权与借用", Some(cid)),
        ("cache 基础", None),
    ] {
        match store
            .insert_note(NewNote {
                course_id: course,
                title,
                source_path: None,
                content: &format!("{title} 的内容"),
                fts_content: None,
            })
            .unwrap()
        {
            InsertOutcome::Created(n) => ids.push(n.id),
            InsertOutcome::Duplicate { .. } => panic!("不应去重"),
        }
    }

    // 标题搜索：范围=该课程（不含 all 区笔记）
    let hits = store.search_note_titles("所有权", Some(cid), 50).unwrap();
    assert_eq!(hits.len(), 2);
    // 范围=全部
    let all = store.search_note_titles("", None, 50).unwrap();
    assert_eq!(all.len(), 3);

    // 批量删除前两篇（rust 课的）——FTS/chunk 行同步清理
    let deleted = store.delete_notes_by_ids(&ids[..2]).unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(
        store
            .search_note_titles("所有权", Some(cid), 50)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        store.search_note_titles("cache", None, 50).unwrap().len(),
        1
    );
    // 空列表安全
    assert_eq!(store.delete_notes_by_ids(&[]).unwrap(), 0);
}

/// 回归（收编统计口径）：move_note 收编后概念归属仍留旧课/all，
/// 课程统计与出题概念清单必须按"笔记关联的概念"计数，否则显示 0c
/// 且出题时概念清单为空。
#[test]
fn course_stats_counts_concepts_by_note_association() {
    let store = store();

    // 还原真实时序：无课导入 → 笔记与概念都归 NULL（all 区）
    let n1 = match store
        .insert_note(NewNote {
            course_id: None,
            title: "所有权",
            source_path: None,
            content: "所有权内容",
            fts_content: None,
        })
        .unwrap()
    {
        InsertOutcome::Created(n) => n,
        InsertOutcome::Duplicate { .. } => panic!(),
    };
    let concept = store.get_or_create_concept("所有权", None).unwrap();
    store.link_note_concept(n1.id, concept).unwrap();

    // 新建 test 课并收编笔记（概念归属仍留 NULL——即用户遇到的 0c 场景）
    let test = store.get_or_create_course("test").unwrap();
    store.move_note(n1.id, Some(test)).unwrap();

    // 统计口径：test 课 1 篇笔记、1 个关联概念（旧口径为 0c）
    assert_eq!(store.course_stats(test).unwrap(), (1, 1));
    // 出题概念清单同样按关联取
    let concepts = store.list_concepts_with_mastery(Some(test)).unwrap();
    assert_eq!(concepts.len(), 1);
    assert_eq!(concepts[0].name, "所有权");
}

/// 出题兜底取材：概念检索零命中时按课取全部 chunk。
#[test]
fn chunks_by_course_returns_all_material() {
    let store = store();
    let cid = store.get_or_create_course("rust").unwrap();
    let n = match store
        .insert_note(NewNote {
            course_id: Some(cid),
            title: "所有权",
            source_path: None,
            content: "move 语义 与 借用规则",
            fts_content: None,
        })
        .unwrap()
    {
        InsertOutcome::Created(n) => n,
        InsertOutcome::Duplicate { .. } => panic!(),
    };
    store
        .insert_chunks(
            n.id,
            &[
                NewChunk {
                    heading: "move",
                    content: "move 语义 转移所有权",
                    fts_extra: None,
                },
                NewChunk {
                    heading: "borrow",
                    content: "借用规则 &mut 独占",
                    fts_extra: None,
                },
            ],
        )
        .unwrap();

    // 概念词（"test" 这类无关词）检索零命中
    assert!(
        store
            .search_chunks("无关词xyz", Some(cid), 10)
            .unwrap()
            .is_empty()
    );
    // 兜底取材：全课 chunk
    let all = store.chunks_by_course(Some(cid), 24).unwrap();
    assert_eq!(all.len(), 2, "应取到该课全部 chunk");
}
