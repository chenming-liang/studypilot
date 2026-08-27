-- mynotes-agent 数据库 schema —— 本文件即唯一事实源（改表先改这里再写迁移）。
-- 本文件与其保持一致；改表先改规划再改这里，禁止各处散落 CREATE TABLE。

-- all 区不是一行记录：course_id IS NULL 即未归类/all，查询不过滤课程即为 all
CREATE TABLE courses(
  id   INTEGER PRIMARY KEY,
  name TEXT UNIQUE NOT NULL               -- 如 rust / csapp
);

CREATE TABLE notes(
  id           INTEGER PRIMARY KEY,
  course_id    INTEGER REFERENCES courses(id) ON DELETE SET NULL,
  title        TEXT NOT NULL,
  source_path  TEXT,                      -- 原始文件路径（只读参考，绝不回写）
  content      TEXT NOT NULL,             -- 清洗后原文（wikilink 已剥离）
  content_hash TEXT UNIQUE NOT NULL,      -- sha256 前 16 位：幂等去重 + 短 id
  created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

-- rowid = notes.id；content 存 jieba 预分词文本（决策 D1）
CREATE VIRTUAL TABLE notes_fts USING fts5(content);

CREATE TABLE concepts(
  id        INTEGER PRIMARY KEY,
  name      TEXT NOT NULL,
  course_id INTEGER REFERENCES courses(id) ON DELETE SET NULL,
  UNIQUE(name, course_id)
);

CREATE TABLE note_concepts(               -- 笔记↔概念多对多：共用概念不重复存
  note_id    INTEGER REFERENCES notes(id)    ON DELETE CASCADE,
  concept_id INTEGER REFERENCES concepts(id) ON DELETE CASCADE,
  PRIMARY KEY(note_id, concept_id)
);

CREATE TABLE note_chunks(                  -- 笔记按标题拆分的 chunk（检索粒度）
  id           INTEGER PRIMARY KEY,
  note_id      INTEGER REFERENCES notes(id) ON DELETE CASCADE,
  heading      TEXT NOT NULL DEFAULT '',    -- chunk 所属的 ## 标题
  content      TEXT NOT NULL,               -- chunk 原文（clean_for_fts 级别）
  position     INTEGER NOT NULL,            -- 在笔记中的顺序（0-based）
  content_hash TEXT NOT NULL                -- chunk 级去重
);

-- chunk 级 FTS 索引（rowid = note_chunks.id；content 存 jieba 预分词文本，决策 D1）
CREATE VIRTUAL TABLE note_chunks_fts USING fts5(content);

CREATE TABLE quizzes(
  id         INTEGER PRIMARY KEY,
  course_id  INTEGER REFERENCES courses(id),
  scope      TEXT NOT NULL,               -- 出题范围描述，如 "rust 所有权"
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE questions(
  id              INTEGER PRIMARY KEY,
  quiz_id         INTEGER REFERENCES quizzes(id),
  type            TEXT NOT NULL CHECK(type IN ('choice','short_answer')),
  question        TEXT NOT NULL,
  options_json    TEXT,                 -- 选择题选项 JSON 数组
  answer          INTEGER,              -- 选择题正确下标
  key_points_json TEXT,                 -- 简答题要点 JSON 数组
  explanation     TEXT,
  concept_id      INTEGER REFERENCES concepts(id)
);

CREATE TABLE attempts(                    -- 每次作答记录（可重做）
  id           INTEGER PRIMARY KEY,
  question_id  INTEGER REFERENCES questions(id),
  answer_text  TEXT,
  score        INTEGER,                   -- 仅简答题有
  missing_json TEXT,
  created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE concept_mastery(
  concept_id    INTEGER PRIMARY KEY REFERENCES concepts(id),
  attempts      INTEGER NOT NULL DEFAULT 0,
  correct       INTEGER NOT NULL DEFAULT 0,
  last_reviewed TEXT
);

CREATE TABLE sessions(
  id         INTEGER PRIMARY KEY,
  title      TEXT,
  course_id  INTEGER REFERENCES courses(id),  -- 会话发生时的分区，恢复时还原
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE messages(
  id         INTEGER PRIMARY KEY,
  session_id INTEGER REFERENCES sessions(id) ON DELETE CASCADE,
  role       TEXT NOT NULL CHECK(role IN ('system','user','assistant','tool')),
  content    TEXT NOT NULL,               -- 原样 JSON 字符串（含 tool_calls / tool_call_id）
  seq        INTEGER NOT NULL             -- 会话内顺序
);

CREATE TABLE usage_log(                   -- R6：覆盖 chat/vision/embedding 全部外部调用
  id                INTEGER PRIMARY KEY,
  provider          TEXT NOT NULL,
  model             TEXT NOT NULL,
  kind              TEXT NOT NULL,        -- chat | vision | embedding | review | outline ...
  prompt_tokens     INTEGER NOT NULL,
  completion_tokens INTEGER NOT NULL,
  cost              REAL NOT NULL,        -- 按配置单价折算
  created_at        TEXT NOT NULL DEFAULT (datetime('now'))
);
