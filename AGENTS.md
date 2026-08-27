# AGENTS.md — AI 会话约定（每次会话开始前必读）

完整规划见 `规划.md`；**关键技术决策的唯一事实源是 `规划.md` 附录 A**，与本文件冲突时以规划为准，发现冲突应提出而非自行取舍。

## 项目一句话

mynotes-agent：Rust TUI 个人知识库 Agent（导入 → 学习 → 复习三模式），ratatui + SQLite，本地优先。

## Workspace 布局

```
crates/core        Agent Loop、Tool trait、Provider trait、事件总线（包名 agent-core：`core` 与 Rust 内建 crate 冲突，会劫持集成测试与宏展开的 ::core:: 路径）
crates/providers   OpenAI-compatible LLM 客户端（reqwest + SSE）
crates/tools       agent 工具：search_notes / list_courses / generate_outline
crates/storage     rusqlite 存储（同步 API）+ FTS5(jieba 预分词)
crates/tui         bin crate：ratatui App shell + Import/Study/Review mode view
scripts/pdf_extract.py   pymupdf 提取脚本（子进程调用，stdout 输出 JSON）
config.toml        provider 配置（示例见 规划.md 第四章）
data/              运行时生成的 sqlite 文件（gitignore）
```

## 编码约定

- **文档同步（强制）**：每次修改代码后，必须同步更新 `docs/核心代码逻辑.md` 中受影响的章节——改了哪条数据流/机制/决策落点，就更新对应小节；新增机制补新小节。禁止出现"代码已改、文档还是旧逻辑"的状态。
- **错误处理**：库 crate 用 `thiserror` 定义错误枚举；bin/TUI 层用 `anyhow` 透传
- **异步纪律**：禁止在 async 上下文直接调 rusqlite 或等待子进程——一律 `tokio::task::spawn_blocking`（决策 D2）
- **LLM 调用**：全部经 `Provider` trait；测试一律用 `MockProvider` + 录制的 JSON fixture，**绝不真调外部 API**
- **中文检索**：入库与查询必须过同一条 jieba 分词管线（决策 D1）；FTS5 MATCH 的每个词加双引号
- **结构化输出**：所有 LLM JSON 调用共用一条降级链（决策 D4）：json_object → prompt 约束+正则提取 `{...}` → 重试一次 → 跳过
- **命令 vs 工具**：`/import` `/review` `/delete` `/move` 直连内部函数不进 agent loop；只有只读检索型工具暴露给 LLM（决策 D6）
- **日志**：用 `tracing`；TUI 代码里禁止 `println!`
- **安全红线**：删除笔记只删知识库数据，绝不修改/删除磁盘原文件
- **数据库变更**：schema 以 `规划.md` 第四章 DDL 为准；需要改表先更新 DDL 再写迁移，不允许各处散落 CREATE TABLE

## 每会话收工必跑

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

三项全绿后，更新本文件底部进度表，再结束会话。

## 会话开场模板

每次新会话第一句固定为：

> 读 AGENTS.md 和 规划.md。本次会话只做 M{n}：<目标一句话>。以下功能明确不做（缓冲区可砍项，实现了也算越界）：闪卡、大纲交互视图 V2、embedding 向量检索、主动学习引擎完整版。完成后跑收工检查并更新进度表。

不要单会话连做多个里程碑。

## 真实测试语料（fixtures）

本仓库自带两门课的真实资料，是 M0/M6/M7/Demo 的标准测试集：

- `CSAPP/notes/`：17 篇章节/实验笔记（中文 Markdown）
- `程序设计实践-rust/note/`：3 讲 Rust 课程笔记
- `程序设计实践-rust/resources/`：3 个课件 PDF（M0 pymupdf 验证用，如 `01-basic.pdf`）
- `CSAPP/resources/`：约 17 讲课件 PPTX（pptx 抽取测试用）

解析器必须处理的实际特征（详见 `规划.md` "真实语料"一节）：Obsidian wikilink `[[#…]]` 需剥离、头部元信息引用块、"目录"小节属噪声、文件名含空格与 `(1)` 重复副本（测 hash 幂等去重）。**不要把这两个目录的内容复制进 crates**，运行时按路径导入即可。

## 当前进度（每会话收工时更新）

- [x] M0 风险前置验证（pymupdf 子进程 / FTS5+jieba 实验 / 流式响应字段实测）——结论见规划.md 附录 A D1/D3/D7
- [x] M1 项目骨架：workspace + config 解析 + Provider trait + 客户端
- [x] M2 Agent Loop：Tool trait + tool call 循环
- [x] M3 Storage：SQLite schema + FTS5(jieba)
- [x] M4 TUI v1 + mpsc 事件通道 + 中断
- [x] M5 基础要求 R1-R6 收口
- [x] M6 定制点①：导入流水线 + 课程归类
- [x] M7 检索 RAG + generate_outline
- [x] M8 定制点②：复习模式 + 掌握度闭环
- [x] 打磨：集成串测、README、Windows 抽查、Demo 演练
