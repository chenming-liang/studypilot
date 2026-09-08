# StudyPilot

StudyPilot 是一个运行在终端里的 AI 学习 Agent，面向"把课程资料真正学懂"这个场景。它把笔记组织成个人知识库：导入课程资料后，可以基于自己的笔记提问、按知识点自动出题复习，并记录每个知识点的掌握情况。

通用聊天助手能回答"所有权是什么"，但不记得你学没学会。StudyPilot 把"导入 → 学习 → 复习"串成闭环，数据保存在本地，模型可接入多家服务商。

---

## Screenshots

![提问界面](image-1.png)

![复习界面](image-2.png)

![复习地图](image-3.png)

---

## Why StudyPilot?

通用对话助手能回答大部分问题，但它并不为"学习"设计：回答不基于你的资料、不会检测你掌握与否、更不会针对薄弱点安排复习。StudyPilot 是一个围绕学习流程设计的 Agent，把三个环节固化成了产品：

- **回答有依据**——基于你的笔记回答并标注来源，笔记未覆盖的部分会明确说明
- **结构可追踪**——导入时自动抽取知识点、组织成章节结构，并记录每个知识点的掌握状态
- **复习成闭环**——先闪卡 recall、再正式出题、即时批改，薄弱知识点自动优先重考

这些能力在 Features 中逐项展开。

---

## Features

### Material-grounded Q&A

基于你导入的笔记回答，而不是通用知识库。提问后，回答中的关键事实会标注来源编号（如 `[1]`），对应到具体的笔记片段，方便你回看原文；笔记没有覆盖的内容会标明「（笔记外补充）」，区分"笔记讲的"和"模型自己补的"。

### Review Map（知识地图）

导入的资料会自动抽取为知识点，并按主题组织成「章节 → 概念」的结构，形成一张可浏览的知识地图。每个知识点标记掌握状态：已掌握（✓）、需巩固（△）、未复习（○）。地图可导出为 Markdown 文件（`Outline → Export`），作为复习提纲带走。

### Review（复习闭环）

复习按"先检查、再训练、后沉淀"的顺序组织：

1. **Flashcard Warm-up**：先出几张 recall 卡片快速自检。你自评（会了 / 模糊 / 不会）后，评为"模糊/不会"的概念会进入正式复习的重点范围
2. **Formal Review**：逐题生成选择题 / 简答题。选择题即时判分；简答题由模型批改打分，并指出缺失的要点
3. **掌握度追踪**：每次作答记入学习历史，复习后给出小结（已掌握 / 需巩固 / 下一步建议）。答错的概念在后续复习中优先出现

### Model Roles（角色与模型配置）

StudyPilot 把任务分成几类角色（fast / balanced / reasoning），每类角色可以**单独指定一个模型**。系统按任务所属的角色使用你配置的模型，而不是为每次调用自行挑选——这样既能让高频、机械的任务用便宜快速的模型省钱，也能让高难度的推理任务用更强的模型保证质量。

**如何配置**

在命令面板（`Ctrl+K` → Model）顶部有三行角色：`fast`、`balanced`、`reasoning`。选中角色 → 选择该角色的模型 → 回车绑定并保存。也可以在 `config.toml` 的 `[roles]` 里手动指定（如 `fast = "deepseek/deepseek-v4-flash"`）。未配置的角色自动使用当前模型。

**任务分工**

| 角色 | 负责的任务 | 特点 |
|---|---|---|
| `fast` | 导入、概念抽取 | 大量、结构化、低难度，追求速度，不开启思考 |
| `balanced` | 普通问答、复习追问、正式出题、复习小结、闪卡、大纲、复习地图 | 默认学习，追求响应快，不开启思考 |
| `reasoning` | 复杂简答批改 | 高难度推理，开启思考保证质量 |

> 导入、概念抽取这类机械、高频的流程，建议配给 `fast` 角色一个速度快、价格低的模型（如 `deepseek-v4-flash`），能明显提升体验。

### 本地优先

笔记、会话、学习历史全部保存在本地数据库，不依赖云端存储。重复导入同一份资料不会产生重复内容（按内容去重）。数据留存在本机，这与"调用你配置的 LLM 服务"是两个独立概念——你的资料不会上传到任何服务商。

---

## Supported Providers & Models

以下为当前内置的服务商及常用模型（均可通过命令行面板选择，也可以在配置文件中指定）：

| Provider | Models |
|---|---|
| DeepSeek | `deepseek-v4-flash` · `deepseek-v4-pro` · `deepseek-v4-flash-vision-exp` |
| Qwen (通义千问) | `qwen3.8-max` · `qwen3.7-max` · `qwen3.8-flash` · `qwen3.7-flash` |
| GLM (智谱) | `glm-5.3` · `glm-5.3-flash` · `glm-5.2` |
| Kimi (Moonshot) | `kimi-k3` · `kimi-k2.7-code` · `kimi-k2.6` |
| MiniMax | `MiniMax-M3` · `MiniMax-M2.7` · `MiniMax-M2.7-highspeed` · `MiniMax-M2.5` |
| Doubao (火山方舟) | `doubao-seed-2.1-pro` · `doubao-seed-2.1-turbo` |
| Hunyuan (腾讯混元) | `Hy4-preview` · `Hy3` |
| ERNIE (百度千帆) | `ernie-5.1` · `ernie-5.0` |
| OpenAI | `gpt-6-astra` · `gpt-5.6-sol` · `gpt-5.6-terra` · `gpt-5.6-luna` |
| Anthropic (Claude) | `claude-fable-5.1` · `claude-opus-5` · `claude-sonnet-5` · `claude-haiku-4.5` |
| Gemini (Google) | `gemini-3.8-flash` · `gemini-3.7-flash` · `gemini-3.6-flash` |
| Grok (xAI) | `grok-4.6` · `grok-4.5` · `grok-4.3` |

此外支持任意 **Custom OpenAI-compatible** 服务（自定义 Base URL 与模型名），可接入 OpenRouter、本地 Ollama 等动态模型目录。

> 模型为各厂商当前主流选项，随厂商更新演进；未列出模型可通过自定义方式接入。

---

## Installation

### 使用预编译版本

如果不想本地编译，可直接下载预编译包（[GitHub Releases](https://github.com/chenming-liang/studypilot/releases)）：提供 Windows / Linux / macOS 三平台，下载解压后直接运行，无需安装 Rust。

> 首次使用前：若导入的资料包含 PDF，需先在**解压目录**运行一次依赖安装脚本（程序需在解压目录运行，以便找到配套文件）。
>
> - **Windows**（在解压目录打开 PowerShell）：
>   ```
>   Set-ExecutionPolicy -Scope Process Bypass
>   scripts\install-deps.ps1
>   ```
> - **Linux / macOS**（在解压目录打开终端）：
>   ```
>   bash scripts/install-deps.sh
>   ```

### 从源码构建

```bash
git clone https://github.com/chenming-liang/studypilot.git
cd studypilot
cargo build --release
```

构建需要 **Rust**（stable 工具链）。Windows 用户建议使用 **MSVC toolchain**（`stable-x86_64-pc-windows-msvc`，需 Visual Studio Build Tools）——GNU toolchain 在含中文的路径下可能链接失败。

```bash
# 构建产物
./target/release/studypilot
```

---

## Quick Start

1. **配置 AI**：首次启动自动进入 AI Setup。选择 Provider → 选择 Model → 填入 API Key → 点 Test 测试连接，通过后进入主界面。之后随时可用 `Ctrl+K → Model` 重新配置。

2. **导入资料**：`Ctrl+K` → Import Materials → 选择你的资料文件夹（支持 Markdown / PDF / PPTX）。导入时每个文件会显示处理进度；完成后在聊天区看到每个文件抽取出的概念数。

3. **提问**：直接在底部输入问题，例如"什么是所有权？"。回答会基于你的笔记展开，关键事实后带 `[1]` 等编号——它们在回答下方对应具体笔记片段；笔记没覆盖的内容会标「（笔记外补充）」。

4. **复习**：`Ctrl+K` → Review。先出现几张闪卡：`Space` 翻开答案、`1/2/3` 自评（会了 / 模糊 / 不会）、`Enter` 下一张。闪卡结束后进入正式复习——选择题用 `↑↓` 或 `A-D` 作答、`Enter` 提交；简答题在底部输入答案、`Enter` 提交批改，会看到得分和缺失要点。

5. **查看进度**：`Ctrl+K` → Review Map。每个知识点标有状态：✓ 已掌握 / △ 需巩固 / ○ 未复习。用 `↑↓` 浏览，选中某个概念或章节可直接复习。

---

## Usage

所有功能都可以通过 **Command Palette** 完成——按 `Ctrl+K` 打开，按分组浏览，无需记忆命令：

- **学习**：New Conversation · Review · Review Map · Outline（含 Export）· Materials
- **课程**：Switch Course · New Course · Rename Course · Delete Course · Import Materials
- **会话**：Recent Sessions · Rename Session · Export / Load Conversation
- **设置**：Model（切换模型 / 配置角色）· Budget（费用上限）

几个常用操作：

- **切换课程**：`Ctrl+K` → Switch Course，选择目标课程后，之后的提问和复习都在该课程内进行
- **费用控制**：`Ctrl+K` → Budget 查看累计花费、设置上限；达到上限后自动停止调用，避免超出预算
- **恢复进度**：`Ctrl+K` → Recent Sessions 回到之前的对话继续提问；Export Conversation 可把会话保存为文件，换设备后 Load 恢复——学完关掉终端，下次还能接着上次的问题继续

---

## Configuration

配置文件存放在 `~/.studypilot/`（首次启动自动创建）：

| 文件 | 内容 |
|---|---|
| `config.toml` | Provider / Model 列表、默认模型、Model Roles、预算上限 |
| `auth.toml` | API Key（权限 0600，与配置分离，不会写回 config.toml） |

配置项包括：

- **API Key**：在 AI Setup 中填写，或写入 `auth.toml`；也支持通过环境变量引用
- **Endpoint**：每个 Provider 预设了默认端点；Custom Provider 可自定义 Base URL
- **Model**：可通过命令面板切换并持久化为默认模型
- **Custom OpenAI-compatible Provider**：在 Setup 中选择 Custom，填写 Provider 名称、Base URL、Model、API Key
- **Model Roles**：在 `config.toml` 的 `[roles]` 中为 fast / balanced / reasoning 三种任务角色指定模型（如 `fast = "deepseek/deepseek-v4-flash"`）
- **Budget**：在命令面板查看累计花费、设置累计上限，超出后停止调用；可重置累计金额

---

## Architecture

```
                 ┌─────────────┐
                 │     User    │
                 └──────┬──────┘
                        ↓
                 ┌─────────────┐
                 │  StudyPilot │
                 │     TUI     │
                 └──────┬──────┘
                        ↓
          ┌─────────────┴─────────────┐
          ↓                           ↓
    Learning Agent               Review Engine
          ↓                           ↓
        RAG                      Learning Map
          ↓                           ↓
     Materials                Concept / Mastery
```

StudyPilot 由两部分协作：**学习助手**负责基于资料的问答（检索笔记 → 生成带引用的回答），**复习引擎**负责复习闭环（组织知识地图 → 出题 → 批改 → 更新掌握度）。两者都建立在本地数据库之上（存储笔记、会话与学习历史）。

---

## Manual

本 README 覆盖了快速上手的完整路径。更多操作细节（完整快捷键、复习各阶段按键、笔记/会话管理、预算与模型配置）见 [使用手册](docs/使用手册.md)。

---

## License

MIT
