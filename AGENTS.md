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

## 当前进度（每会话收工时更新；详细历史与决策见 docs/核心代码逻辑.md）

**完成态**：M0-M8 里程碑 + 收口会话①~㉞ 全数合入 main + **收口会话㉟（Command 体系产品化 + all 剥离 + Import 路径语义 + 产品化 Phase 3-7，分支 refactor/command-integration 进行中）**。核心能力——导入流水线（md/pdf/pptx + 幂等去重）、FTS5+jieba 中文检索（噪声降权）、RAG 学习、Review 复习闭环（逐题生成/预取/evidence 多样性/Review Map/进度监控/素材瘦身）、Outline concept-driven 持久缓存、Agent Trace 可见化、预算熔断与持久化、三级 workspace（Home/Course/Session）、onboarding 卡片与 Home Launchpad 视觉；**产品化**（pricing 可选化、Runtime Config ~/.studypilot、Model Registry 内置预设、Home AI 待配置引导、/test 连接测试、binary=studypilot、GitHub Release workflow、README Installation 优先）。workspace 282 测试全绿。

**关键架构决策（后续开发必须遵守，细节见 docs）**：
- **UI/颜色重构（纯 token）**：正文=终端默认前景、主视觉=Primary 蓝、移除紫色（SECONDARY=PRIMARY）、新增 WARNING 黄；Tool 活动行 ⟳muted/✓绿/✗红+正文默认；Review Map Section 恒蓝+mark 语义色；◌ 活动 header=muted、● Ready=绿；Color::Gray 统一 MUTED
- **问题.md 13-17（追加）**：⑬a 保存链路端到端验证（3 模型 Setup→落盘→重读全保留）+ split_custom_models 兼容中文分隔符（含空格模型名不误拆）；⑬b 模型选择器 PageUp/PgDn/Home/End + ListState 自动滚动；⑭ flashcard prompt 禁开放问法 + 强制 Reveal 后自评；⑮ flashcard 用 render_markdown 渲染（代码块高亮）；⑯ review_scope：focus 全带 + ✓ 按 2:1 补、全过也抽 ≤n 概念；⑰ 修复 Entry::Tool/闪卡双勾（文本去 ✓ 前缀）
- **问题.md 十二项 UI 收口**：palette 直开动作（Model/Budget/Reset/Sessions 不填输入框）、会话分组（Load/Recent/Rename/Export）、移除 Help 总览、scope 后缀删除、Flashcard s 键跳过、Setup Done 折行、setup_test_lines 宽度-4、对话框调大、Home Switch Course 直进 Course
- **Learning Map + Flashcard Warm-up（v3）**：Review Map = 三层（Course→Section→Concept）+ `organize` 按 concept_id 去重 + `ReviewMapPicker` 树形选择器（section 头行聚合 ✓/△/○、概念行缩进，Enter 概念=复习概念/section=复习整节）；Flashcard 是正式 Review 前置 recall 暖场（一次 LLM 调用产 **3~8 张** `WARMUP_CARD_MIN/MAX`——LLM 自主数量但 `.take(8)` 硬上限不无限生成、范围=选定概念素材、question/answer 按弹窗宽度 wrap 折行永不溢出、Space 翻开/1·2·3 自评/Enter 下一张/Esc 退出；**正式题数=用户指定**——Review Map 路径经题数向导（默认 5 可改），`/review --n N` 保持，引擎不写死 5、focus/卡数/section 数均不影响题数），**自评绝不写 mastery**（纯内存 WarmupState，focus_concepts 汇总 △/○ 作为 scope 传现有 run_review），Focus 只存当前 session 用完即丢；Only Formal Review 更新 mastery
- Review 出题是**逐题生成 + 后台预取**；判重=同轮 too_similar(≥0.75)，Concept 是主题不是去重单位
- Review Map 用 `importer::review_map`（lib 共用）；Outline 缓存签名自愈（data/outline/{id}.json）
- 课程上下文**单一事实源** = `app.course`，`home_cursor`/侧栏/session 全部从它派生（`sync_home_cursor_to_course`）；New Course 对话框直接叠 Home，建完直达 Course 页（`pending_course_enter`）
- **per-course 分区（§32）**：`Partition{history,entries,session_state,session_cost,scroll_up}` 按课缓存，`swap_course_partition` 五处切换点换入换出——聊天/错误/花费不串课
- **命令入口统一 Ctrl+K**（Home/Course 不再任意键入开面板，普通键入不弹窗）
- **AI Setup + Home 常驻入口**：`App::setup` 覆盖层状态机（Provider→Model→Credentials→Test→Done）；**Home 首项恒为 AI 入口**（`setup_offset` 恒 1，与 home_cursor_count/home_activate/home_cursor_course/sync_home_cursor_to_course 一致）——未配置=「Set up AI」引导、已配置=「AI Models」继续接入，Enter 恒进 Setup（修复"配完第一个就接不了第二个"）；API key 遮罩输入、Test 成功才保存——config.toml 写**全量 all_providers**（追加不覆盖）、auth.toml **合并**已有 key；Esc 逐级回退；连接测试复用 `/test` canonical `run_connection_test`（普通 chat 非 chat_json——DeepSeek 无 json 字样 prompt 强制 400）；Test 成功自动进 Done（test_passed 分流），失败回 Credentials 输入框清空 + 旧 key 渲染提示、重新输入直接替换；**Custom OpenAI-compatible 四步 UI 输入**（Name/Base URL/Model ID + API key，不再必须手改 config.toml；provider 名保留用户输入 `custom_display_name`、多模型逗号拆分 `custom_provider_multi` 独立 ModelConfig、Test 用 pending cfg 原样报错；预设多选 Test 步渲染全部 selected_models 逐行，不再"多选似单选"；连接测试结果经 setup_test_lines 按 \n 拆行 + wrap 渲染，失败提示不再挤成一行；Setup 对话框 68×24、Custom 输入经 setup_input_lines 折行（Model ID 多模型逗号输入不溢出、光标落末行行尾）；**多模型逐测**（setup_test 逐个 model 连接测试、setup_test_all_passed 任一 ✗ 整体失败）；命令面板高度 32）
- **Provider/Model 解耦（v2）**：`ProviderConfig.models[]` + `Config.default_model`（`provider/model` canonical，serde default 零迁移——`models_or_legacy`/`resolve_default_model` 回退旧单 model）；运行期 `provider_cfg.model` = 活动模型（下游零改动）；`/model` 全量分组选择（`apply_model_switch(provider,model)` 重建 client 不要求重输 key，落盘 default_model）；Setup Model 步空格多选（单一 ✓ 勾 + ▸ 光标，不再混用 ☑/✓）；config.toml 保持 `[[providers]]` 数组形态（**不**嵌套表，用户确认）；**凭证分离（auth.toml）**——`ProviderConfig.api_key` `skip_serializing` 只读（解析兼容旧 config、序列化永不写回），`AuthConfig` 存 `~/.studypilot/auth.toml`（0600）；启动 `migrate_from` 迁移旧明文 key + `apply_to` 合并进内存，Setup 成功存 key 到 auth.toml、config.toml 只写 Provider/Model
- **Provider incomplete ≠ config invalid**：`OpenAiClient::new` 容忍缺 key（请求时才报可操作错误），缺 key 的独立 binary 正常启动进 Home + AI 引导（E2E 复现）
- **产品化（productization，Phase 3-6）**：pricing 可选化（`ProviderConfig.price_*` → `Option<f64>`、`known_pricing()`，未知模型 Cost tracking unavailable 可正常运行）；Runtime Config（`Config::save`/`runtime_path`，main 优先 ~/.studypilot/config.toml 回退 cwd、缺失/损坏不 crash 用 default，/model 切换落盘）；Model Registry（`providers::registry`：DeepSeek/GLM/OpenAI 预设 + custom_provider）；AI Setup（`App::ai_needs_setup()` Home 引导 + `/test` 连接测试）
- **同一能力三入口一个实现**：Home/Course UI、Ctrl+K Palette、/slash CLI 都调 canonical action（`open_import_wizard`/`switch_course`/`rename_course`/`ListChoiceAction`），UI 不拼命令字符串
- Home/Course/Session 是纯 UI state 不入 session history；启动一律进 Home
- **all 不是 Course，是 Global scope**：Switch Course picker/侧栏只列真实课程、header/Ask/Continue 显示 `Global` 而非 `all`、`/review` `/new` 在 Global 下拒绝引导选课；backend（`/course all` CLI、course_id=NULL、Global 分区）保留
- **Tool Trace 持久化**：messages 表已存 assistant tool_calls + tool 结果（DB 是长期事实源），`history_to_entries` 恢复时重建紧凑 `✓ search_notes → 3 条`（同一 renderer、不重执行、不存 raw result、不渲染 CoT）
- **Import 路径统一 resolver（import_path.rs）**：`resolve_import_path` 统一处理绝对/相对路径（`~` 前缀展开 `$HOME`、相对 cwd、canonicalize 归一化）、单文件与目录、存在性/类型/空目录前置校验；CLI `/import <path>` 与 `/import --dir <path>`、Wizard、Palette、Course 页全部走同一 resolver + `run_import(ImportTarget)`；不依赖 project root，不动 importer 核心
- 会话标题无意义时 fallback `Recent conversation`；`SessionMeta.created_at` 供 Last studied；/sessions 默认当前课；/rename context-aware（Course→课、Session→会话）；all 降格为跨课检索范围

**待办**：
- [ ] M9 网页导入（已排期）：`/add-url <url> [--course x]`——reqwest 抓取 → readability/htmd 正文提取（保留标题层级与代码块）→ 转 RawDoc 复用现有管线；notes 表加 `source_url` 列，schema.sql 先改 DDL 再写迁移（SCHEMA_VERSION 3→4，迁移需兼容曾被 Web 版升到 5 的库）；内容 hash 幂等去重；本地快照优先（存提取正文，防链接失效）；只抓单页不爬站；抓取失败明确报错不静默丢；SPA 页面提取差为已知局限，文档如实标注。schema 变更，走分支。
- [ ] 收口会话㉟ 收尾待办：`refactor/command-integration` 分支跑真实 TUI 回归（headless 无法截图）→ 合回 main；palette fuzzy search（当前 substring）可后续增强；Home/Course 鼠标点击仍未支持
- [x] 收口会话㉟ 附：Home 首屏对齐 Agent项目改进建议.md mockup——区块分隔线（`──────`）、Continue learning 内课程名/会话标题分行、`[ Continue ]` 按钮化、`[ + New Course ]` 括弧样式（纯 home_lines 渲染，光标/导航/分区逻辑不变）
