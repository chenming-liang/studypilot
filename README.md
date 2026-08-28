# StudyPilot

> **Your AI study agent for course materials.**

一个用 Rust 实现的本地优先学习 Agent——导入课程资料、基于笔记辅导问答、主动出题复习并追踪掌握度。（仓库目录沿用 agent）



## 核心功能

### 三种模式（对应知识生命周期：输入 → 消化 → 巩固）

| 模式 | 功能 |
|-|-|
| **导入 (Import)** | 批量导入 md/pdf/pptx，LLM 自动归类到课程、提取概念、chunk 化入库；进度条可 Ctrl+C 中断 |
| **学习 (Study)** | 知识问答（RAG 检索 + agent loop 自动调度 + `[n]` 引用标注）；课程大纲生成（缩进树 + Markdown 导出） |
| **复习 (Review)** | LLM 基于笔记出题（选择题+简答题）；选择题本地判分、简答题 LLM 批改；掌握度闭环（薄弱概念优先出题） |

### 技术亮点

- **Chunk 级 RAG**：笔记按 `##` 标题拆分为 300-800 token 的 chunk，FTS5 检索降到 chunk 粒度——搜"借用"只命中"借用"章节，不被其他章节词频稀释
- **jieba 中文分词 + FTS5**：入库与查询共用同一条分词管线（决策 D1），AND → OR 降级 + bm25 排序
- **Agent Loop**：LLM 自主决定是否检索、检索几次（最多 3 次不同关键词），而非固定管道
- **D4 JSON 降级链**：所有 LLM 结构化输出共用 `json_object → prompt 约束 + 正则提取 → 重试一次 → 跳过`
- **R6 成本统计**：覆盖 chat/import/review/outline/grade 全部外部 AI 调用，预算熔断
- **D2 异步纪律**：rusqlite 同步 API + `spawn_blocking`，不阻塞 TUI 渲染

## 安装

### 前置依赖

```bash
# Rust 工具链
rustup default stable

# Python 3 + pymupdf（PDF 提取）
pip install pymupdf
# Debian/Ubuntu (PEP 668): pip install --break-system-packages pymupdf
# 或用 venv: python3 -m venv .venv && source .venv/bin/activate && pip install pymupdf

# 剪贴板工具（可选，拖选复制用）
sudo apt install xclip   # X11
# 或 wl-clipboard         # Wayland
```

### 构建

```bash
git clone <repo-url> && cd agent
cargo build --release
```

### 配置

创建 `config.toml`（已被 .gitignore 忽略，不会提交）：

```toml
default_provider = "deepseek"
max_cost = 5.0

[[providers]]
name = "deepseek"
endpoint = "https://api.deepseek.com/v1"
api_key = "sk-..."              # 或用 api_key_env = "DEEPSEEK_API_KEY"
model = "deepseek-reasoner"     # 思考模式
price_prompt = 4.0              # 元/百万 token
price_completion = 16.0
context_length = 65536
thinking = true

[[providers]]
name = "deepseek-chat"
endpoint = "https://api.deepseek.com/v1"
api_key = "sk-..."
model = "deepseek-chat"         # 非思考模式，更快更便宜
price_prompt = 0.27
price_completion = 1.1
context_length = 65536
thinking = false
```

## 使用

```bash
cargo run --release -- -p tui
# 或指定配置文件
cargo run --release -- -p tui -- /path/to/config.toml
```

### 命令一览

| 命令 | 说明 |
|-|-|
| `/help` | 命令列表 |
| `/course` | 列出课程 |
| `/course <名>` | 切换分区 |
| `/course -new <名>` | 新建课程 |
| `/course -delete <名>` | 删除课程（笔记回落 all） |
| `/model` | 弹窗选择模型 |
| `/new` | 开启新会话 |
| `/sessions` | 列出历史会话 |
| `/open <id>` | 恢复历史会话 |
| `/rename <标题>` | 重命名当前会话 |
| `/export` | 导出当前会话 JSON |
| `/load <文件>` | 加载导出的会话 |
| `/import <目录> [--course <名>]` | 导入语料（md/pdf/pptx） |
| `/notes` | 列出当前课程笔记 |
| `/delete <id>` | 删除笔记 |
| `/delete --course <名>` | 批量删除课程笔记 |
| `/move <id> <课程>` | 迁移笔记 |
| `/outline [课程] [--export]` | 生成课程大纲 |
| `/review <课程> [概念] [--n N]` | 生成复习题 |
| `/budget` | 查看预算 |
| `/budget <金额>` | 设置上限 |
| `/budget reset` | 清零累计花费 |
| `v` | 鼠标拖选复制聊天内容 |
| `Ctrl+C` / `Esc` | 中断请求 / 退出 |
| `PageUp/Down` | 滚动聊天流 |

### 快速开始

```
# 1. 导入课程语料
/import 程序设计实践-rust --course rust
/import CSAPP --course csapp

# 2. 切换课程并提问
/course rust
> 什么是所有权？              # AI 检索笔记后回答，带 [1] 引用

# 3. 跨课程提问
/course all
> ownership vs 虚拟内存       # 检索两门课的笔记

# 4. 生成大纲
/outline rust
/outline csapp --export       # 导出 Markdown

# 5. 复习
/review rust 所有权           # 出 5 道题
# 选择题按 1-4 作答，简答题输入后 Enter

# 6. 查看费用
/budget
```

## 架构

```
crates/
├── core/        Agent Loop、Tool trait、Provider trait、MockProvider
├── providers/   OpenAI-compatible LLM 客户端（reqwest + chat_json）
├── storage/     rusqlite（同步 API + spawn_blocking）+ FTS5(jieba) + chunk 级检索
├── importer/    导入流水线：walkdir → 解析(md/pdf/pptx) → LLM 概念抽取 → chunk 入库
├── tools/       search_notes（chunk 级 FTS5）/ list_courses
└── tui/         ratatui App shell + mpsc 事件通道 + 复习模式 + 鼠标拖选复制
```

### 技术决策（机制详见 docs/核心代码逻辑.md）

| 编号 | 决策 |
|-|-|
| D1 | jieba 预分词 + FTS5（chunk 级，AND→OR 降级 + bm25） |
| D2 | rusqlite 同步 API + spawn_blocking |
| D3 | pymupdf 子进程提取 PDF |
| D4 | JSON mode 统一降级链 |
| D5 | 课程唯一主分类 |
| D6 | 命令直连流水线，仅检索工具进 agent loop |
| D7 | reasoning_content/reasoning 双变体兼容 |

## 项目结构

- `config.toml` — provider 配置（gitignore）
- `scripts/pdf_extract.py` — pymupdf PDF 提取脚本
- `data/` — SQLite 数据库 + 日志 + 导出文件（gitignore）
- `CSAPP/` — 真实测试语料（17 篇笔记 + 课件）
- `程序设计实践-rust/` — 真实测试语料（3 讲笔记 + 3 个 PDF 课件）

## 技术栈

Rust · ratatui · crossterm · rusqlite (FTS5) · jieba-rs · reqwest · tokio · walkdir · zip · quick-xml · pymupdf

## License

MIT
