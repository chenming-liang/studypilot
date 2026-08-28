//! 大纲生成流程与渲染。

use std::sync::Arc;

use agent_core::{Message, Provider};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, Entry};
use crate::outline_render::{render_outline_markdown, render_outline_tree};

impl App {
    /// `/outline [课程] [--export]`：无参默认当前课程。
    pub(crate) fn handle_outline_command(&mut self, arg: &str) {
        let export = arg.contains("--export");
        let name = arg.replace("--export", "").trim().to_owned();
        let name = if name.is_empty() {
            self.course.clone()
        } else {
            name
        };
        if name == "all" {
            self.push_entry(Entry::Error("大纲需指定具体课程，不能为 all".into()));
            return;
        }
        let Some(course_id) = self
            .courses
            .iter()
            .find(|(_, n)| *n == name.as_str())
            .map(|(id, _)| *id)
        else {
            self.push_entry(Entry::Error(format!("课程 `{name}` 不存在")));
            return;
        };

        let store = Arc::clone(&self.store);
        let provider = self.provider.clone();
        let provider_cfg = self.provider_cfg.clone();
        let tx = self.tx.clone();
        let course_name = name.clone();
        // 大纲期间挂 inflight：header 显示进行中，Ctrl+C 可中断（否则会落入空闲→退出）
        let cancel = CancellationToken::new();
        self.inflight = Some(cancel.clone());

        tokio::spawn(async move {
            let store_clone = Arc::clone(&store);
            let gathered = spawn_blocking(move || {
                let titles = store_clone.list_note_titles_by_course(course_id)?;
                let concepts = store_clone.list_concept_names_by_course(course_id)?;
                Ok::<_, storage::Error>((titles, concepts))
            })
            .await;

            let (titles, concepts) = match gathered {
                Ok(Ok((t, c))) if !t.is_empty() => (t, c),
                _ => {
                    let _ = tx.send(AppEvent::OutlineGenerated(
                        None,
                        course_name,
                        export,
                        Vec::new(),
                    ));
                    return;
                }
            };

            let note_list: String = titles
                .iter()
                .enumerate()
                .map(|(i, (_, t))| format!("[{}] {}", i + 1, t))
                .collect::<Vec<_>>()
                .join("\n");
            let concept_list = concepts.join("、");
            let prompt = format!(
                "以下是《{course_name}》课程的笔记列表和已提取的概念。\n\
                 请生成结构化大纲，输出 JSON：\n\
                 {{\"sections\": [{{\"title\": \"章节\", \"points\": [\"知识点\"], \"refs\": [笔记编号]}}]}}\n\n\
                 笔记列表:\n{note_list}\n\n已提取概念: {concept_list}\n\n\
                 重要要求：\n\
                 1. 每个 section 的 refs 必须包含至少一个笔记编号\n\
                 2. refs 里的数字是上面笔记列表中的 [编号]\n\
                 3. 按知识逻辑组织章节，不要只罗列笔记标题"
            );
            let messages = [
                Message::system("只输出 JSON，不要 markdown 代码块。"),
                Message::user(&prompt),
            ];
            // R6：outline 的 LLM 调用也记 usage + cost（等待期间观察取消）
            let pc = provider_cfg.clone();
            let content = match agent_providers::with_cancel(provider.chat_json(&messages), &cancel)
                .await
            {
                Some(Ok(resp)) => {
                    Self::log_llm_usage(&store, &pc, "outline", &resp.usage);
                    Some(resp.content)
                }
                Some(Err(e)) if e.is_json_mode_unsupported() => {
                    match agent_providers::with_cancel(provider.chat(&messages, &[]), &cancel).await
                    {
                        Some(Ok(resp)) => {
                            Self::log_llm_usage(&store, &pc, "outline", &resp.usage);
                            Some(resp.content)
                        }
                        Some(Err(e)) => {
                            tracing::warn!("大纲生成失败: {e}");
                            None
                        }
                        None => None,
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!("大纲生成失败: {e}");
                    None
                }
                None => None,
            };
            let _ = tx.send(AppEvent::OutlineGenerated(
                content,
                course_name,
                export,
                titles,
            ));
        });
    }

    pub(crate) fn on_outline_generated(
        &mut self,
        content: Option<String>,
        course: String,
        export: bool,
        titles: Vec<(i64, String)>,
    ) {
        self.inflight = None;
        self.request_cost_sync();
        let Some(json_str) = content else {
            self.push_entry(Entry::Error("大纲生成失败（LLM 无响应或无笔记）".into()));
            return;
        };
        // D4 兜底：直解失败 → 提取第一个 {...} 块再解
        let outline: serde_json::Value = match serde_json::from_str(json_str.trim()) {
            Ok(v) => v,
            Err(_) => {
                let Some(block) = agent_core::first_json_block(&json_str)
                    .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                else {
                    self.push_entry(Entry::Error("大纲 JSON 解析失败".into()));
                    return;
                };
                block
            }
        };

        if export {
            let md = render_outline_markdown(&outline, &course, &titles);
            let dir = std::path::Path::new("data/exports");
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join(format!("{course}-outline.md"));
            match std::fs::write(&path, &md) {
                Ok(()) => self.push_entry(Entry::Info(format!(
                    "大纲已导出到 {}（{} 字节）",
                    path.display(),
                    md.len()
                ))),
                Err(e) => self.push_entry(Entry::Error(format!("导出失败: {e}"))),
            }
        } else {
            self.push_entry(Entry::Info(format!("《{course}》课程大纲:")));
            for line in render_outline_tree(&outline, &titles) {
                self.push_entry(Entry::Info(line));
            }
        }
    }

    // ---- M6：笔记管理 ----
}
