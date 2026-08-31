use std::path::Path;
use std::sync::Mutex;

use agent_core::Message;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;
use crate::schema_sql;
use crate::search::{self, SearchHit};
use crate::segment::segment;
use crate::{Error, SCHEMA_VERSION};

/// 一条笔记（notes 表行；content 为清洗后原文，非分词文本）。
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: i64,
    pub course_id: Option<i64>,
    pub title: String,
    pub source_path: Option<String>,
    pub content: String,
    pub content_hash: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct NewChunk<'a> {
    pub heading: &'a str,
    /// chunk 正文（clean_for_fts 级别：已去元信息+目录段）——干净文本，供展示与引用
    pub content: &'a str,
    /// 概念名等 boosting 后缀——只进 FTS 索引，不进 content
    pub fts_extra: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ConceptMastery {
    pub concept_id: i64,
    pub name: String,
    pub attempts: i64,
    pub correct: i64,
}

#[derive(Debug, Clone)]
pub struct QuestionRecord {
    pub id: i64,
    pub q_type: String,
    pub question: String,
    pub options_json: Option<String>,
    pub answer: Option<i64>,
    pub key_points_json: Option<String>,
    pub explanation: Option<String>,
    pub concept_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHit {
    pub chunk_id: i64,
    pub note_id: i64,
    pub note_title: String,
    pub heading: String,
    /// chunk 正文原文（未分词）
    pub content: String,
    /// bm25 分数，越小越相关
    pub rank: f64,
}

#[derive(Debug, Clone)]
pub struct NoteSummary {
    pub id: i64,
    pub course_id: Option<i64>,
    pub title: String,
    /// content_hash 前 4 位（短 id 兜底，TUI 列表展示用）
    pub short_id: String,
}

#[derive(Debug, Clone)]
pub struct NewNote<'a> {
    pub course_id: Option<i64>,
    pub title: &'a str,
    pub source_path: Option<&'a str>,
    /// 清洗后原文（wikilink 已剥离，保留原文结构——供展示/引用）
    pub content: &'a str,
    /// FTS 专用文本（已去目录段+元信息噪声）；None 时回退到 content
    pub fts_content: Option<&'a str>,
}

/// 幂等去重结果：同 content_hash 重复导入返回已存在笔记信息
/// （含位置，供上层提示"已存在：课程#id 标题"——全局去重下用户需要知道去哪删）。
#[derive(Debug)]
pub enum InsertOutcome {
    Created(Note),
    Duplicate {
        existing_id: i64,
        existing_title: String,
        existing_course_id: Option<i64>,
    },
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: i64,
    pub title: Option<String>,
    pub course_id: Option<i64>,
}

/// 同步存储门面。Connection 包在 Mutex 里使其 Sync，
/// async 调用方按决策 D2 经 `tokio::task::spawn_blocking` 使用。
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Self {
            conn: Mutex::new(conn),
        }
        .migrate()
    }

    /// 迁移：以 PRAGMA user_version 为版本号，逐版升级。
    fn migrate(self) -> Result<Self> {
        {
            let mut conn = self.conn.lock().unwrap();
            let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            if version < 1 {
                conn.execute_batch(schema_sql())?;
            }
            // v2：追加 chunk 表（已有库升级时执行）
            if version < 2 {
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS note_chunks(
                       id           INTEGER PRIMARY KEY,
                       note_id      INTEGER REFERENCES notes(id) ON DELETE CASCADE,
                       heading      TEXT NOT NULL DEFAULT '',
                       content      TEXT NOT NULL,
                       position     INTEGER NOT NULL,
                       content_hash TEXT NOT NULL
                     );
                     CREATE VIRTUAL TABLE IF NOT EXISTS note_chunks_fts USING fts5(content);",
                )?;
            }
            // v3：quizzes/sessions.course_id 补 ON DELETE SET NULL（原为 NO ACTION，
            // 出过题或开过会话的删除课程会撞 FOREIGN KEY constraint failed）。
            // SQLite 不能 ALTER 外键，需建新表拷数据。PRAGMA foreign_keys 须在事务外切换。
            if version < 3 {
                conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
                let tx = conn.transaction()?;
                tx.execute_batch(
                    "CREATE TABLE quizzes_new(
                       id         INTEGER PRIMARY KEY,
                       course_id  INTEGER REFERENCES courses(id) ON DELETE SET NULL,
                       scope      TEXT NOT NULL,
                       created_at TEXT NOT NULL DEFAULT (datetime('now'))
                     );
                     INSERT INTO quizzes_new(id, course_id, scope, created_at)
                         SELECT id, course_id, scope, created_at FROM quizzes;
                     DROP TABLE quizzes;
                     ALTER TABLE quizzes_new RENAME TO quizzes;
                     CREATE TABLE sessions_new(
                       id         INTEGER PRIMARY KEY,
                       title      TEXT,
                       course_id  INTEGER REFERENCES courses(id) ON DELETE SET NULL,
                       created_at TEXT NOT NULL DEFAULT (datetime('now'))
                     );
                     INSERT INTO sessions_new(id, title, course_id, created_at)
                         SELECT id, title, course_id, created_at FROM sessions;
                     DROP TABLE sessions;
                     ALTER TABLE sessions_new RENAME TO sessions;",
                )?;
                tx.commit()?;
                conn.execute_batch("PRAGMA foreign_keys = ON;")?;
            }
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(self)
    }

    // ---- courses ----

    pub fn find_course(&self, name: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row("SELECT id FROM courses WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// 不存在则创建（导入时 LLM 归类到封闭课程列表，见 D5）。
    pub fn get_or_create_course(&self, name: &str) -> Result<i64> {
        if let Some(id) = self.find_course(name)? {
            return Ok(id);
        }
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO courses(name) VALUES (?1)", [name])?;
        Ok(conn.last_insert_rowid())
    }
    /// 全部课程（TUI 侧栏用）。
    pub fn list_courses(&self) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM courses ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.flatten().collect())
    }

    /// 课程统计：(笔记数, 概念数)。
    pub fn course_stats(&self, course_id: i64) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        let notes: i64 = conn.query_row(
            "SELECT count(*) FROM notes WHERE course_id = ?1",
            [course_id],
            |r| r.get(0),
        )?;
        // 概念口径 = 该课笔记关联的 DISTINCT 概念数（收编后概念的
        // course_id 可能仍留 all，按归属数会漏——按关联数才符合
        // "这门课覆盖多少概念"的用户预期）
        let concepts: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT nc.concept_id)
             FROM note_concepts nc JOIN notes n ON n.id = nc.note_id
             WHERE n.course_id = ?1",
            [course_id],
            |r| r.get(0),
        )?;
        Ok((notes as usize, concepts as usize))
    }
    /// 删除课程。notes/concepts 的 course_id 由 ON DELETE SET NULL 回落 all 区，
    /// 数据不丢。返回是否真的删了。
    pub fn delete_course(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute("DELETE FROM courses WHERE name = ?1", [name])?;
        Ok(affected > 0)
    }

    /// 按 id 删除课程（UI 选择器用——id 对课程名的任何字符免疫）。
    pub fn delete_course_by_id(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute("DELETE FROM courses WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }

    // ---- notes + FTS ----

    pub fn insert_note(&self, note: NewNote<'_>) -> Result<InsertOutcome> {
        let hash = content_hash(note.content);
        let segmented = segment(note.fts_content.unwrap_or(note.content));

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM notes WHERE content_hash = ?1",
                [&hash],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing_id) = existing {
            let (existing_title, existing_course_id) = tx.query_row(
                "SELECT title, course_id FROM notes WHERE id = ?1",
                [existing_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            return Ok(InsertOutcome::Duplicate {
                existing_id,
                existing_title,
                existing_course_id,
            });
        }

        tx.execute(
            "INSERT INTO notes(course_id, title, source_path, content, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                note.course_id,
                note.title,
                note.source_path,
                note.content,
                hash
            ],
        )?;
        let id = tx.last_insert_rowid();

        // FTS 行与 notes.id 对齐；content 存预分词文本（D1）
        tx.execute(
            "INSERT INTO notes_fts(rowid, content) VALUES (?1, ?2)",
            params![id, segmented],
        )?;
        tx.commit()?;

        Ok(InsertOutcome::Created(Note {
            id,
            course_id: note.course_id,
            title: note.title.to_owned(),
            source_path: note.source_path.map(String::from),
            content: note.content.to_owned(),
            content_hash: hash,
            created_at: String::new(), // 由 DB 默认值生成；需要时再查
        }))
    }

    pub fn get_note(&self, id: i64) -> Result<Option<Note>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, course_id, title, source_path, content, content_hash, created_at
             FROM notes WHERE id = ?1",
            [id],
            row_to_note,
        )
        .optional()
        .map_err(Error::from)
    }

    /// 删除知识库数据（安全红线：绝不碰磁盘原文件——本函数只操作 DB）。
    /// FTS 无外键级联，需显式清理。
    pub fn delete_note(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM notes_fts WHERE rowid = ?1", [id])?;
        tx.execute("DELETE FROM note_chunks_fts WHERE rowid IN (SELECT id FROM note_chunks WHERE note_id = ?1)", [id])?;
        tx.execute("DELETE FROM note_chunks WHERE note_id = ?1", [id])?;
        let affected = tx.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(affected > 0)
    }

    // ---- chunks ----

    /// 批量写入笔记的 chunk + chunk FTS 索引。事务内完成。
    pub fn insert_chunks(&self, note_id: i64, chunks: &[NewChunk]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for (i, ch) in chunks.iter().enumerate() {
            let hash = content_hash(ch.content);
            // FTS 索引文本 = 正文 + 可选概念名后缀（boosting），互不污染
            let fts_source = match ch.fts_extra {
                Some(extra) => format!("{} {}", ch.content, extra),
                None => ch.content.to_owned(),
            };
            let segmented = segment(&fts_source);
            tx.execute(
                "INSERT INTO note_chunks(note_id, heading, content, position, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![note_id, ch.heading, ch.content, i as i64, hash],
            )?;
            let chunk_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO note_chunks_fts(rowid, content) VALUES (?1, ?2)",
                params![chunk_id, segmented],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// chunk 级检索：返回精准片段（天然就是相关段落，无需 snippet 提取）。
    /// 范围 = 当前课程 ∪ all 区。
    pub fn search_chunks(
        &self,
        query: &str,
        course_id: Option<i64>,
        top_k: usize,
    ) -> Result<Vec<ChunkHit>> {
        search::run_chunks(&self.conn, query, course_id, top_k)
    }

    /// 中文检索。范围 = 当前课程 ∪ all 区（course_id 匹配或为 NULL）；
    /// `course_id = None` 即 all 区语义（不过滤）。
    pub fn search_notes(
        &self,
        query: &str,
        course_id: Option<i64>,
        top_k: usize,
    ) -> Result<Vec<SearchHit>> {
        search::run(&self.conn, query, course_id, top_k)
    }

    // ---- sessions / messages ----

    pub fn create_session(&self, title: Option<&str>, course_id: Option<i64>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions(title, course_id) VALUES (?1, ?2)",
            params![title, course_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 追加一条消息，seq 自动递增。content 存 Message 的原样 JSON
    /// （含 tool_calls / tool_call_id），恢复时无损还原。
    pub fn append_message(&self, session_id: i64, msg: &Message) -> Result<()> {
        let json = serde_json::to_string(msg)?;
        let role = serde_json::to_value(msg.role)?
            .as_str()
            .unwrap_or_default()
            .to_owned();

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO messages(session_id, role, content, seq) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role, json, next_seq],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 更新会话归属课程（用户切分区时跟随当前工作上下文；None = all）。
    pub fn update_session_course(&self, session_id: i64, course_id: Option<i64>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET course_id = ?1 WHERE id = ?2",
            params![course_id, session_id],
        )?;
        Ok(())
    }

    /// 按序还原会话消息（R5 历史上下文的基础）。
    pub fn load_session_messages(&self, session_id: i64) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT content FROM messages WHERE session_id = ?1 ORDER BY seq")?;
        let rows = stmt.query_map([session_id], |r| r.get::<_, String>(0))?;
        let mut msgs = Vec::new();
        for json in rows {
            msgs.push(serde_json::from_str(&json?)?);
        }
        Ok(msgs)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, title, course_id FROM sessions ORDER BY id DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(SessionMeta {
                id: r.get(0)?,
                title: r.get(1)?,
                course_id: r.get(2)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    // ---- usage_log（R6：覆盖 chat/vision/embedding 全部外部调用）----

    /// 记录一次外部 AI 调用的 token 用量与折算成本。
    pub fn append_usage(
        &self,
        provider: &str,
        model: &str,
        kind: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
        cost: f64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO usage_log(provider, model, kind, prompt_tokens, completion_tokens, cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                provider,
                model,
                kind,
                prompt_tokens as i64,
                completion_tokens as i64,
                cost
            ],
        )?;
        Ok(())
    }

    /// 全部历史调用的累计成本（元）。
    pub fn total_recorded_cost(&self) -> Result<f64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COALESCE(SUM(cost), 0.0) FROM usage_log", [], |r| {
            r.get(0)
        })
        .map_err(Error::from)
    }

    /// 清空 usage_log（重置累计成本为 0）。
    pub fn clear_usage_log(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM usage_log", [])?;
        Ok(())
    }

    /// 重命名会话；返回会话是否存在。
    pub fn set_session_title(&self, id: i64, title: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(affected > 0)
    }

    // ---- concepts / note_concepts ----

    /// 获取或创建概念（按 name + course_id 去重）。
    pub fn get_or_create_concept(&self, name: &str, course_id: Option<i64>) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM concepts WHERE name = ?1 AND \
                 (course_id IS ?2 OR (course_id IS NOT NULL AND course_id = ?2))",
                params![name, course_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO concepts(name, course_id) VALUES (?1, ?2)",
            params![name, course_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 关联笔记与概念（幂等：重复链接不报错）。
    pub fn link_note_concept(&self, note_id: i64, concept_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO note_concepts(note_id, concept_id) VALUES (?1, ?2)",
            params![note_id, concept_id],
        )?;
        Ok(())
    }

    /// 课程最近出过的题面（跨轮防重：注入出题 prompt + 相似度检测）。
    pub fn recent_question_texts(&self, course_id: i64, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT q.question FROM questions q
             JOIN quizzes z ON z.id = q.quiz_id
             WHERE z.course_id = ?1
             ORDER BY q.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![course_id, limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows.flatten().collect())
    }

    /// 课程今日作答次数（Anki 式"今日/长期"分层统计的今日侧）。
    pub fn attempts_today_by_course(&self, course_id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM attempts a
             JOIN questions q ON q.id = a.question_id
             JOIN quizzes z ON z.id = q.quiz_id
             WHERE z.course_id = ?1 AND date(a.created_at) = date('now','localtime')",
            [course_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// 课程内概念→笔记 id 关联（Review Map 的章节 refs 聚合用）。
    pub fn concept_note_pairs(&self, course_id: i64) -> Result<Vec<(i64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT nc.concept_id, nc.note_id FROM note_concepts nc
             JOIN concepts c ON c.id = nc.concept_id WHERE c.course_id = ?1",
        )?;
        let rows = stmt.query_map([course_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.flatten().collect())
    }

    /// 解除笔记的全部概念关联（概念刷新第一步）。
    pub fn unlink_note_concepts(&self, note_id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM note_concepts WHERE note_id = ?1", [note_id])?;
        Ok(n)
    }

    /// 清理课程内「零关联且零学习历史」的概念（概念刷新收尾）。
    /// 有 mastery 记录的概念绝不删除——学习历史不可丢（孤儿保留，宁可脏不可丢）。
    pub fn prune_unlinked_concepts(&self, course_id: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM concepts WHERE course_id = ?1 \
             AND id NOT IN (SELECT concept_id FROM note_concepts) \
             AND id NOT IN (SELECT concept_id FROM concept_mastery)",
            [course_id],
        )?;
        Ok(n)
    }

    /// 获取课程下所有笔记的 (note_id, title) 列表，按 id 排序。
    pub fn list_note_titles_by_course(&self, course_id: i64) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, title FROM notes WHERE course_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([course_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.flatten().collect())
    }

    /// 获取课程下所有概念名列表。
    pub fn list_concept_names_by_course(&self, course_id: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT DISTINCT name FROM concepts WHERE course_id = ?1 ORDER BY name")?;
        let rows = stmt.query_map([course_id], |r| r.get::<_, String>(0))?;
        Ok(rows.flatten().collect())
    }

    /// 列出指定课程（或 all=不过滤）的笔记摘要（id + title + hash 前4位）。
    pub fn list_notes(&self, course_id: Option<i64>, limit: usize) -> Result<Vec<NoteSummary>> {
        let conn = self.conn.lock().unwrap();
        let sql = match course_id {
            Some(_) => {
                "SELECT id, course_id, title, content_hash FROM notes \
                        WHERE course_id = ?1 ORDER BY id LIMIT ?2"
            }
            None => {
                "SELECT id, course_id, title, content_hash FROM notes \
                     ORDER BY id LIMIT ?1"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<NoteSummary> {
            Ok(NoteSummary {
                id: r.get(0)?,
                course_id: r.get(1)?,
                title: r.get(2)?,
                short_id: r.get::<_, String>(3)?[..4].to_owned(),
            })
        };
        let rows = match course_id {
            Some(cid) => stmt.query_map(params![cid, limit as i64], map)?,
            None => stmt.query_map(params![limit as i64], map)?,
        };
        Ok(rows.flatten().collect())
    }

    /// 按课程批量删除笔记（含 FTS 行）。安全红线：只删 DB，不碰磁盘文件。
    pub fn delete_notes_by_course(&self, course_id: i64) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        // FTS5 无外键级联，必须显式清理 notes_fts + note_chunks_fts
        tx.execute(
            "DELETE FROM notes_fts WHERE rowid IN (SELECT id FROM notes WHERE course_id = ?1)",
            [course_id],
        )?;
        tx.execute(
            "DELETE FROM note_chunks_fts WHERE rowid IN \
             (SELECT c.id FROM note_chunks c JOIN notes n ON n.id = c.note_id WHERE n.course_id = ?1)",
            [course_id],
        )?;
        let affected = tx.execute("DELETE FROM notes WHERE course_id = ?1", [course_id])?;
        tx.commit()?;
        Ok(affected)
    }

    /// 迁移笔记到目标课程（仅更新 course_id，正文不动）。
    pub fn move_note(&self, note_id: i64, target_course_id: Option<i64>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "UPDATE notes SET course_id = ?1 WHERE id = ?2",
            params![target_course_id, note_id],
        )?;
        Ok(affected > 0)
    }
}

fn row_to_note(r: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: r.get(0)?,
        course_id: r.get(1)?,
        title: r.get(2)?,
        source_path: r.get(3)?,
        content: r.get(4)?,
        content_hash: r.get(5)?,
        created_at: r.get(6)?,
    })
}

// ---- quizzes / questions / attempts / concept_mastery（M8 复习模式）----

impl Store {
    /// 创建一次测验记录。
    pub fn create_quiz(&self, course_id: Option<i64>, scope: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO quizzes(course_id, scope) VALUES (?1, ?2)",
            params![course_id, scope],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 插入一道题目。
    #[allow(clippy::too_many_arguments)]
    pub fn insert_question(
        &self,
        quiz_id: i64,
        q_type: &str,
        question: &str,
        options_json: Option<&str>,
        answer: Option<i64>,
        key_points_json: Option<&str>,
        explanation: Option<&str>,
        concept_id: Option<i64>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "INSERT INTO questions(quiz_id, type, question, options_json, answer, key_points_json, explanation, concept_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![quiz_id, q_type, question, options_json, answer, key_points_json, explanation, concept_id],
    )?;
        Ok(conn.last_insert_rowid())
    }

    /// 记录一次作答。
    pub fn record_attempt(
        &self,
        question_id: i64,
        answer_text: &str,
        score: Option<i64>,
        missing_json: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "INSERT INTO attempts(question_id, answer_text, score, missing_json) VALUES (?1, ?2, ?3, ?4)",
        params![question_id, answer_text, score, missing_json],
    )?;
        Ok(conn.last_insert_rowid())
    }

    /// 更新概念掌握度（答对 +1 correct，总 +1 attempts）。
    pub fn update_concept_mastery(&self, concept_id: i64, correct: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO concept_mastery(concept_id, attempts, correct, last_reviewed)
         VALUES (?1, 1, ?2, datetime('now'))
         ON CONFLICT(concept_id) DO UPDATE SET
           attempts = attempts + 1,
           correct = correct + ?2,
           last_reviewed = datetime('now')",
            params![concept_id, if correct { 1 } else { 0 }],
        )?;
        Ok(())
    }

    /// 获取概念掌握度（attempts, correct, last_reviewed）。
    pub fn get_concept_mastery(
        &self,
        concept_id: i64,
    ) -> Result<Option<(i64, i64, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT attempts, correct, last_reviewed FROM concept_mastery WHERE concept_id = ?1",
            [concept_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(Error::from)
    }

    /// 列出课程下概念 + 掌握度（掌握度低的排前，用于出题优先）。
    pub fn list_concepts_with_mastery(
        &self,
        course_id: Option<i64>,
    ) -> Result<Vec<ConceptMastery>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.name,
                COALESCE(cm.attempts, 0) as attempts,
                COALESCE(cm.correct, 0) as correct
         FROM concepts c
         JOIN note_concepts nc ON nc.concept_id = c.id
         JOIN notes n ON n.id = nc.note_id
         LEFT JOIN concept_mastery cm ON cm.concept_id = c.id
         WHERE n.course_id IS ?1
         GROUP BY c.id
         ORDER BY attempts ASC, correct DESC",
        )?;
        let rows = stmt.query_map([course_id], |r| {
            Ok(ConceptMastery {
                concept_id: r.get(0)?,
                name: r.get(1)?,
                attempts: r.get(2)?,
                correct: r.get(3)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 获取测验的全部题目。
    /// 按 id 批量查概念名（复习小结/掌握度可视化用）。
    pub fn concept_names(&self, ids: &[i64]) -> Result<std::collections::HashMap<i64, String>> {
        if ids.is_empty() {
            return Ok(Default::default());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, name FROM concepts WHERE id IN ({placeholders})");
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut map = std::collections::HashMap::new();
        for (id, name) in rows.flatten() {
            map.insert(id, name);
        }
        Ok(map)
    }

    /// 只返回「有关联笔记片段」的概念（随机范围出题用——避免抽中无素材概念退回大杂烩）。
    pub fn concepts_with_material(&self, course_id: Option<i64>) -> Result<Vec<ConceptMastery>> {
        let conn = self.conn.lock().unwrap();
        let (sql, cid): (&str, Option<i64>) = match course_id {
            Some(_) => (
                "SELECT c.id, c.name, COALESCE(cm.attempts, 0), COALESCE(cm.correct, 0)
                 FROM concepts c
                 JOIN note_concepts nc ON nc.concept_id = c.id
                 JOIN note_chunks ch ON ch.note_id = nc.note_id
                 LEFT JOIN concept_mastery cm ON cm.concept_id = c.id
                 WHERE c.course_id = ?1
                 GROUP BY c.id",
                course_id,
            ),
            None => (
                "SELECT c.id, c.name, COALESCE(cm.attempts, 0), COALESCE(cm.correct, 0)
                 FROM concepts c
                 JOIN note_concepts nc ON nc.concept_id = c.id
                 JOIN note_chunks ch ON ch.note_id = nc.note_id
                 LEFT JOIN concept_mastery cm ON cm.concept_id = c.id
                 GROUP BY c.id",
                None,
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let map_row = |r: &rusqlite::Row<'_>| {
            Ok(ConceptMastery {
                concept_id: r.get(0)?,
                name: r.get(1)?,
                attempts: r.get(2)?,
                correct: r.get(3)?,
            })
        };
        let rows = match cid {
            Some(id) => stmt.query_map([id], map_row)?,
            None => stmt.query_map([], map_row)?,
        };
        Ok(rows.flatten().collect())
    }

    pub fn get_quiz_questions(&self, quiz_id: i64) -> Result<Vec<QuestionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
        "SELECT id, type, question, options_json, answer, key_points_json, explanation, concept_id
         FROM questions WHERE quiz_id = ?1 ORDER BY id",
    )?;
        let rows = stmt.query_map([quiz_id], |r| {
            Ok(QuestionRecord {
                id: r.get(0)?,
                q_type: r.get(1)?,
                question: r.get(2)?,
                options_json: r.get(3)?,
                answer: r.get(4)?,
                key_points_json: r.get(5)?,
                explanation: r.get(6)?,
                concept_id: r.get(7)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }
}

/// sha256 前 16 个 hex 字符：幂等去重 + 短 id。
pub fn content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(content.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

impl Store {
    /// 笔记标题搜索（NoteBrowser 用）：LIKE %q% 匹配标题，
    /// 范围语义与检索一致——course_id=None 为全部，Some 为该课（不含 all 区）。
    /// 与 chunk 级 FTS 检索是两回事：这里是对象浏览，不是内容检索。
    pub fn search_note_titles(
        &self,
        query: &str,
        course_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<NoteSummary>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", query);
        let sql = match course_id {
            Some(_) => {
                "SELECT id, course_id, title, content_hash FROM notes
                 WHERE title LIKE ?1 AND course_id = ?2 ORDER BY id LIMIT ?3"
            }
            None => {
                "SELECT id, course_id, title, content_hash FROM notes
                 WHERE title LIKE ?1 ORDER BY id LIMIT ?2"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<NoteSummary> {
            Ok(NoteSummary {
                id: r.get(0)?,
                course_id: r.get(1)?,
                title: r.get(2)?,
                short_id: r.get::<_, String>(3)?[..4].to_owned(),
            })
        };
        let rows = match course_id {
            Some(cid) => stmt.query_map(rusqlite::params![pattern, cid, limit as i64], map)?,
            None => stmt.query_map(rusqlite::params![pattern, limit as i64], map)?,
        };
        Ok(rows.flatten().collect())
    }

    /// 取某课程全部笔记的 chunk（出题兜底：概念检索未命中时按课取材）。
    /// 取课程范围（None=全部）的 chunk（出题兜底：概念检索未命中时按范围取材）。
    pub fn chunks_by_course(&self, course_id: Option<i64>, limit: usize) -> Result<Vec<ChunkHit>> {
        let conn = self.conn.lock().unwrap();
        let (sql, course_param): (&str, Option<i64>) = match course_id {
            Some(_) => (
                "SELECT c.id, c.note_id, n.title, c.heading, c.content
                 FROM note_chunks c JOIN notes n ON n.id = c.note_id
                 WHERE n.course_id = ?1
                 ORDER BY c.note_id, c.position LIMIT ?2",
                course_id,
            ),
            None => (
                "SELECT c.id, c.note_id, n.title, c.heading, c.content
                 FROM note_chunks c JOIN notes n ON n.id = c.note_id
                 ORDER BY c.note_id, c.position LIMIT ?1",
                None,
            ),
        };
        let params: Vec<Box<dyn rusqlite::ToSql>> = match course_param {
            Some(cid) => vec![Box::new(cid), Box::new(limit as i64)],
            None => vec![Box::new(limit as i64)],
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            |r| {
                Ok(ChunkHit {
                    chunk_id: r.get(0)?,
                    note_id: r.get(1)?,
                    note_title: r.get(2)?,
                    heading: r.get(3)?,
                    content: r.get(4)?,
                    rank: 0.0,
                })
            },
        )?;
        Ok(rows.flatten().collect())
    }

    /// 按概念取笔记片段（随机范围出题用）：返回 (concept_id, chunk) 对，
    /// 每个概念只取前 `per_concept` 段（同一概念跨笔记按 position 顺序）。
    pub fn chunks_by_concepts(
        &self,
        concept_ids: &[i64],
        per_concept: usize,
    ) -> Result<Vec<(i64, ChunkHit)>> {
        if concept_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = concept_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT nc.concept_id, c.id, c.note_id, n.title, c.heading, c.content
             FROM note_concepts nc
             JOIN note_chunks c ON c.note_id = nc.note_id
             JOIN notes n ON n.id = c.note_id
             WHERE nc.concept_id IN ({placeholders})
             ORDER BY nc.concept_id, c.note_id, c.position"
        );
        let params: Vec<&dyn rusqlite::ToSql> = concept_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                ChunkHit {
                    chunk_id: r.get(1)?,
                    note_id: r.get(2)?,
                    note_title: r.get(3)?,
                    heading: r.get(4)?,
                    content: r.get(5)?,
                    rank: 0.0,
                },
            ))
        })?;
        // 每个概念只留前 per_concept 段
        let mut counts = std::collections::HashMap::<i64, usize>::new();
        let mut out = Vec::new();
        for pair in rows.flatten() {
            let (cid, hit) = pair;
            let c = counts.entry(cid).or_insert(0);
            if *c < per_concept {
                *c += 1;
                out.push((cid, hit));
            }
        }
        Ok(out)
    }

    /// 按笔记 id 列表批量删除（NoteBrowser 批量动作）。事务内完成，
    /// FTS 无外键级联需显式清理。返回实际删除的篇数。
    pub fn delete_notes_by_ids(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut deleted = 0usize;
        for id in ids {
            tx.execute("DELETE FROM notes_fts WHERE rowid = ?1", [id])?;
            tx.execute(
                "DELETE FROM note_chunks_fts WHERE rowid IN
                 (SELECT id FROM note_chunks WHERE note_id = ?1)",
                [id],
            )?;
            tx.execute("DELETE FROM note_chunks WHERE note_id = ?1", [id])?;
            deleted += tx.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(deleted)
    }
}

impl Store {
    /// 会话浏览器：按标题搜索（空串=全部），按 id 倒序（新会话在前）。
    pub fn search_sessions(&self, query: &str, limit: usize) -> Result<Vec<SessionMeta>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT id, title, course_id FROM sessions
             WHERE COALESCE(title, '') LIKE ?1
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern, limit as i64], |r| {
            Ok(SessionMeta {
                id: r.get(0)?,
                title: r.get(1)?,
                course_id: r.get(2)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    /// 删除会话（SessionBrowser）。messages 由外键 ON DELETE CASCADE 级联清理，
    /// 即会话聊天记录一并删除且不可恢复。返回是否真的删了。
    pub fn delete_session(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod schema_v3_tests {
    use super::*;

    /// 回归（schema v3）：出过题/开过会话的课程删除后，quizzes/sessions 记录保留
    /// 且 course_id 回落 NULL——此前 NO ACTION 外键会让删除撞 FOREIGN KEY 约束。
    #[test]
    fn delete_course_set_nulls_quiz_and_session() {
        let store = Store::open_in_memory().unwrap();
        let cid = store.get_or_create_course("rust").unwrap();

        let quiz = store.create_quiz(Some(cid), "rust").unwrap();
        let session = store.create_session(Some("测试会话"), Some(cid)).unwrap();

        store.delete_course("rust").unwrap();

        assert!(store.find_course("rust").unwrap().is_none());
        let conn = store.conn.lock().unwrap();
        let quiz_course: Option<i64> = conn
            .query_row("SELECT course_id FROM quizzes WHERE id = ?1", [quiz], |r| {
                r.get(0)
            })
            .unwrap();
        let session_course: Option<i64> = conn
            .query_row(
                "SELECT course_id FROM sessions WHERE id = ?1",
                [session],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(quiz_course, None);
        assert_eq!(session_course, None);
    }
}
