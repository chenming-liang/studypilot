# StudyPilot

**本地优先的 AI 学习助手**——导入课程资料、基于笔记问答、主动出题复习，帮你在终端里把一门课真正学明白。

一个 TUI 应用，支持 **14 家 LLM Provider**，数据全部保存在本地。

---

## 为什么用 StudyPilot

- **不搬运笔记，讲给你听**：基于你自己的课程笔记做问答，AI 像老师一样重新讲解，关键事实标注 `[1]` 引用来源；笔记没覆盖的部分明确标注「（笔记外补充）」
- **学了会忘？让它考你**：根据笔记自动出题（选择题 + 简答题），逐题作答、即时批改，掌握薄弱的概念自动优先再考
- **知识有结构**：自动把散落的概念组织成「章节 → 概念」的复习地图，一眼看到哪些已掌握（✓）、哪些需巩固（△）、哪些还没碰（○）
- **本地优先**：笔记、会话、掌握度都存在本地 SQLite；API key 与配置分离，可放心提交代码
- **能用你已有的模型**：DeepSeek、Qwen、GLM、Kimi、MiniMax、Doubao、Hunyuan、ERNIE、OpenAI、Anthropic、Gemini、Grok、OpenRouter、Ollama 及任意 OpenAI-compatible 服务

---

## 快速开始

### 1. 安装

下载最新 release 即可，无需安装 Rust：

| 平台 | 下载 |
|---|---|
| Windows x64 | `StudyPilot-windows-x64.zip`（解压双击 `studypilot.exe`） |
| Linux x64 | `StudyPilot-linux-x64.tar.gz`（解压运行 `./studypilot`） |

### 2. 首次启动

第一次打开会自动进入 **AI Setup**，跟着向导走：

1. 选择 Provider（如 DeepSeek）
2. 选择 Model
3. 输入 API Key
4. Test Connection → Ready

> 之后随时按 `Ctrl+K` 重新配置，或输入 `/test` 测试连接。

### 3. 导入资料并开始学习

```
/import ~/我的课程资料 --course 数据库原理    ← 导入 md/pdf/pptx，自动归类、提取概念
/course 数据库原理                          ← 进入课程
> 什么是 B+ 树索引？                         ← 直接提问，AI 基于笔记回答
/outline                                    ← 生成复习地图
/review                                     ← 出题复习
```

---

## 功能一览

### 导入（把资料变成知识库）

- 批量导入 **md / pdf / pptx**，按目录或文件路径均可
- 自动提取概念、按章节拆分为检索单元，重复导入自动去重
- 进度可实时查看，`Ctrl+C` 中断

### 学习（理解知识）

- **RAG 问答**：AI 检索你的笔记后回答，自动判断要不要查、查几次
- **引用标注**：关键事实后带 `[1]`，对应来源笔记；笔记外的补充明确标注
- **课程大纲**：一键生成「章节 → 概念」结构，可导出 Markdown

### 复习（巩固知识）

- **自动出题**：选择题（本地判分）+ 简答题（AI 批改打分）
- **复习地图**：Course → Section → Concept 三层，按掌握度着色
- **闪卡暖场**：正式复习前先过一遍 recall 卡片，自评哪里不熟再重点考
- **掌握度追踪**：每道题记入学习历史，薄弱概念自动优先

### 模型管理

- **14 家 Provider** 预设 + 自定义 OpenAI-compatible（任意 Base URL / Model）
- **Model Role**：把不同任务分配到不同模型——便宜快的跑导入/出题，强的跑批改/推理，日常问答用均衡模型
- **角色配置**：`/model` 面板里给 fast / balanced / reasoning 各选一个模型，未配置的角色自动用当前模型

---

## 命令参考

| 命令 | 说明 |
|---|---|
| `/course` | 列出 / 切换课程 |
| `/course -new <名>` | 新建课程 |
| `/course -delete <名>` | 删除课程 |
| `/import <路径> [--course <名>]` | 导入资料（md/pdf/pptx，文件或目录） |
| `/notes` | 浏览当前课程笔记（可搜索、多选、移动/删除） |
| `/outline [--export]` | 生成课程复习地图 / 导出 Markdown |
| `/review-map` | 打开复习地图选择器 |
| `/review [概念] [--n N]` | 出题复习（默认 5 题） |
| `/refresh-concepts` | 重新抽取课程概念 |
| `/model` | 切换模型 / 配置角色 |
| `/test` | 测试当前 Provider 连接 |
| `/new` | 开启新会话 |
| `/sessions` | 历史会话 |
| `/open <id>` | 恢复会话 |
| `/rename <标题>` | 重命名当前会话 |
| `/export` · `/load <文件>` | 导出 / 加载会话 |
| `/budget [金额] [reset]` | 查看 / 设置预算上限 / 清零累计花费 |
| `/help` | 帮助 |

快捷键：`Ctrl+K` 命令面板 · `Ctrl+C` 中断 · `Esc` 退出/返回 · `v` 拖选复制 · `PageUp/Down` 滚动

---

## 配置

配置文件在 `~/.studypilot/`（首次启动自动创建）：

- `config.toml` — Provider / Model 列表与默认模型
- `auth.toml` — API Key（权限 0600，与代码配置分离）

开发者也可以把 `config.toml` 放在仓库根目录（已被 .gitignore 忽略，不会提交）。

---

## 架构

```
crates/
├── core/         Agent Loop、工具与 Provider 抽象
├── providers/    LLM 客户端（14 家 Provider + 统一 OpenAI-compatible 协议）
├── storage/      SQLite + FTS5 全文检索（jieba 中文分词）
├── importer/     导入流水线（md/pdf/pptx 解析 → 概念抽取 → 入库）
├── tools/        检索工具（search_notes / list_courses）
└── tui/          ratatui 终端界面
```

技术细节与设计决策见 [docs/核心代码逻辑.md](docs/核心代码逻辑.md)。

---

## 开发

```bash
# 依赖
rustup default stable
pip install pymupdf            # PDF 提取
sudo apt install xclip         # 剪贴板（可选，拖选复制用）

# 构建 & 运行
cargo build --release
cargo run --release -p tui

# 测试 & 检查
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## License

[MIT](LICENSE)
