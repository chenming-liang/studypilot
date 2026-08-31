# AGENTS.md — AI 会话约定（每次会话开始前必读）

**关键技术决策的唯一事实源是 `docs/核心代码逻辑.md`**（与代码同步维护）。`规划.md` 为早期历史规划，roadmap 部分已过时，仅作背景参考，不再约束开发。

## 项目一句话

StudyPilot（仓库目录 agent）：Rust TUI 个人学习/笔记管理 Agent（导入 → 学习 → 复习三模式），ratatui + SQLite，本地优先。

> **方向回退（2026-08-30，用户决策）**：Web 论文阅读转向（Learning Session + 诊断引擎，66 个提交）经评估判定转向失败，main 已整体回退到 `archive/course-agent-v1`（收口会话⑪ 完整态）。Web 转向的全部工作保留在分支 **`archive/web-pivot`**（tag `web-pivot-final`），需要参考（诊断动作设计/页级 chunk/tool_trace 等经验）时切分支查看，不并入主线。本文件与 docs/核心代码逻辑.md 均已随回退还原为 TUI 版本。注意：`data/mynotes.db` 的 schema user_version 曾被 Web 版升到 5（sessions.document_id 列、diagnosis_stats 表），TUI 版（SCHEMA_VERSION=3）打开会把它写回 3，多余列/表对旧代码无害；若要彻底干净可删掉该文件让 schema 重建（会丢历史数据，删前自行备份）。

## Workspace 布局

```
crates/core        Agent Loop、Tool trait、Provider trait、事件总线（包名 agent-core：`core` 与 Rust 内建 crate 冲突，会劫持集成测试与宏展开的 ::core:: 路径）
crates/providers   OpenAI-compatible LLM 客户端（reqwest + SSE）
crates/tools       agent 工具：search_notes / list_courses / generate_outline
crates/storage     rusqlite 存储（同步 API）+ FTS5(jieba 预分词)
crates/tui         bin crate：ratatui App shell + Import/Study/Review mode view
scripts/pdf_extract.py   pymupdf 提取脚本（子进程调用，stdout 输出 JSON）
config.toml        provider 配置（格式见 crates/providers/src/config.rs）
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
- **数据库变更**：schema 以 `crates/storage/src/schema.sql` 为唯一事实源；需要改表先改 DDL 再写迁移，不允许各处散落 CREATE TABLE

## Git 工作流（AI 改代码的默认纪律）

- 会话开始：`git status` + `git log --oneline -5`；工作区有未提交改动先向用户确认处置
- 小改（bug 修复/单模块）：直接在 main 改，收工三项全绿 + 文档同步后提交
- 大改/实验（跨 crate 重构、schema 变更、架构试验）：`git switch -c refactor|experiment/<名>` 分支进行，全绿后合回 main，失败丢弃分支
- 提交前必查 `git status` / `git diff`：config.toml、密钥、data/ 绝不入库
- 提交粒度 = 一个逻辑改动；信息一行中文说清（做什么 + 为什么）
- 代码改动与 `docs/核心代码逻辑.md` 对应更新放**同一个 commit**
- 改崩可回退：小范围 `git restore <file>`，整体 `git reset --hard`（执行前告知用户）
- 不 push、不 force-push、不改写历史（本地仓，保持线性历史）

## 每会话收工必跑

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

三项全绿后，更新本文件底部进度表，再结束会话。

## 会话开场模板

每次新会话第一句固定为：

> 读 AGENTS.md 和 docs/核心代码逻辑.md。本次会话只做一件事：<目标一句话>。完成后跑收工检查并更新进度表。

不要单会话连做多个里程碑。

## 真实测试语料（fixtures）

本仓库自带两门课的真实资料，是 M0/M6/M7/Demo 的标准测试集：

- `CSAPP/notes/`：17 篇章节/实验笔记（中文 Markdown）
- `程序设计实践-rust/note/`：3 讲 Rust 课程笔记
- `程序设计实践-rust/resources/`：3 个课件 PDF（M0 pymupdf 验证用，如 `01-basic.pdf`）
- `CSAPP/resources/`：约 17 讲课件 PPTX（pptx 抽取测试用）

解析器必须处理的实际特征：Obsidian wikilink `[[#…]]` 需剥离、头部元信息引用块、"目录"小节属噪声、文件名含空格与 `(1)` 重复副本（测 hash 幂等去重）。**不要把这两个目录的内容复制进 crates**，运行时按路径导入即可。

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
- [x] 收口会话①：代码审查修复十项（R6 成本对账闭环、markdown 样式链路修复、D4 降级条件收窄、错误可见化、客户端超时等）+ 导出 `docs/核心代码逻辑.md` + Git 纪律入库
- [x] 收口会话②：事实源切换（规划.md 退役）+ ToolResult 错误类别 + ContextManager 历史裁剪 + 检索评测框架（31 题 BM25 基线 top3=84%，结论：噪声降权优先于 hybrid）
- [x] 收口会话③：检索噪声降权（rank1 58%→71%，top3 87%）+ 评测 --llm 模式（回答质量与检索 rank 强相关 7.2 vs 3.6，0 幻觉；低分主因语料缺口 → 下一步补语料优先于 hybrid）
- [x] 收口会话④：回答策略调整为「笔记优先 + 外部补充明示」（prompt/工具描述/评测器三处同步；幻觉重定义为伪装笔记来源或错误事实），评测平均分 6.1→9.3、低分清零、0 幻觉
- [x] 收口会话⑤：TUI 交互打磨（header 状态可见性 status_label、Ctrl+K 命令面板 19 条、/help 能力化文案）
- [x] 收口会话⑥：命令面板三修（动态宽度/显示宽度对齐/选中项详情行）+ 参数向导（/review /import 文本步向导，课程取当前分区，Tab 保留文本模式）+ header 左右分区
- [x] 收口会话⑦：app.rs 拆分重构（14 模块）+ 交互补全（PaletteAction 绑条目、ListPicker 选择器 /course /open、单步向导 /rename /load /course -new、光标编辑、outline --export 直执行）
- [x] 收口会话⑧：Review workspace 半成品修复（GLM 崩溃遗留：编译错误/进度点颜色丢失/废函数清理/解析丢失回补），主区聊天流隔离为独立答题场 + 反馈停留态 + 简答输入区加高；改进建议.md 新设计版作后续基线（Sources 区/workspace 内 Summary/难度标签等已判不必要）
- [x] 收口会话⑨：底部状态栏专属化（Review 时 hint 随答题态切换，简答输入行即答案框、消除双份渲染）+ 选择题答后选项正误染色（ReviewResult::user_choice），纯渲染改动；改进建议.md 中 Sources 区/卡片化/Summary/侧栏降噪/概念色等经判断未做（成本>收益）
- [x] 收口会话⑩：Review 题干/解析改走 markdown 管线（出题允许题干内代码围栏，修复 C 代码无高亮无缩进）+ 中断修复（新增 `providers::with_cancel`，出题/大纲/导入抽取 LLM 等待期间 Ctrl+C 立即生效；/outline 补 inflight 防空闲退出）
- [x] 收口会话⑪：Review 升级（refactor/review-upgrade 分支合入）——随机范围抽题（concepts_with_material 加权随机，治复读）+ 自由文本范围 AI 解析（--concept 注入 prompt）+ prompt 重构（P1 考知识不考材料/P2 认知层级/P3 选项质量/P4 简答质量/P5 针对性 + 反例）+ answer 字母化 + 结束后 LLM 小结建议（已掌握/需巩固/下一步）+ 修 exit_review 部分摘要卡 bug
- [x] 收口会话⑫（回退后）：**复习出题逐题生成 + 预取流水线**——替代一次性批量出题（大 LLM 调用等待久、无进度感）。首题即现（单题 LLM 调用，quiz 行提前建、每题生成即入库）；作答当前题时后台预取下一题（`generate_review_question` → `ReviewQuestionReady` 回流）；推进时下一题未到位 = 等待态（workspace「出题中…」+ header `· Generating…`，Esc 可退且取消在途生成）；prompt 单题化（P1-P5 保留 + 「已出题目」防重复考点 + 题型配额 choice=⌈n/2⌉ 剩余多者优先）；素材一次收集存 `ReviewState.ctx(Arc<QuizContext>)` 保证整轮范围一致；`parse_one_question` 四级容错（裸对象/旧 questions 格式/裸数组/first_json_block）+ normalize_text；生成失败重试 1 次、耗尽优雅收束（已答的出部分摘要）；进度圆点/标题分母改按 planned。workspace 140 测试全绿
- [ ] M9 网页导入（已排期）：`/add-url <url> [--course x]`——reqwest 抓取 → readability/htmd 正文提取（保留标题层级与代码块）→ 转 RawDoc 复用现有管线；notes 表加 `source_url` 列，schema.sql 先改 DDL 再写迁移（SCHEMA_VERSION 3→4，迁移需兼容曾被 Web 版升到 5 的库）；内容 hash 幂等去重；本地快照优先（存提取正文，防链接失效）；只抓单页不爬站；抓取失败明确报错不静默丢；SPA 页面提取差为已知局限，文档如实标注。schema 变更，走分支。
