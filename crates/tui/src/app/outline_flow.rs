//! 复习地图（Review Map）生成流程：概念驱动的大纲 + 状态渲染 + 复习入口。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, Entry};
use crate::outline_render::OutlinePayload;

impl App {
    /// `/outline [课程] [--export]`：无参默认当前课程。
    /// 概念驱动：LLM 只做「组织既有概念」，point 逐字取自概念清单并 resolve 回主键。
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
            let result = importer::review_map::build_review_map(
                store,
                provider,
                provider_cfg,
                course_id,
                course_name,
                &cancel,
            )
            .await;
            let _ = tx.send(AppEvent::OutlineReady(
                result.map(|map| OutlinePayload { map, export }),
            ));
        });
    }

    /// 复习地图选择器（方案 A：复用 ListPicker，Enter = 对该概念出题）。
    pub(crate) fn open_review_map_picker(&mut self) {
        if self.review_map.is_some() {
            self.open_list_picker(crate::palette::PickKind::ReviewMap);
        }
    }

    pub(crate) fn on_outline_ready(&mut self, result: Result<OutlinePayload, String>) {
        self.inflight = None;
        self.request_cost_sync();
        match result {
            Ok(payload) => {
                self.review_map = Some(payload.map.clone());
                if payload.export {
                    let md = payload.map.markdown();
                    let dir = std::path::Path::new("data/exports");
                    let _ = std::fs::create_dir_all(dir);
                    let path = dir.join(format!("{}-outline.md", payload.map.course));
                    match std::fs::write(&path, &md) {
                        Ok(()) => self.push_entry(Entry::Info(format!(
                            "复习地图已导出到 {}（{} 字节）",
                            path.display(),
                            md.len()
                        ))),
                        Err(e) => self.push_entry(Entry::Error(format!("导出失败: {e}"))),
                    }
                } else {
                    self.push_entry(Entry::Markdown(payload.map.markdown()));
                    self.push_entry(Entry::Info(
                        "· 选中一个知识点开始复习（选择器中 Enter = 对该概念出题）".into(),
                    ));
                    self.open_review_map_picker();
                }
            }
            Err(e) => {
                self.push_entry(Entry::Error(format!("复习地图生成失败: {e}")));
            }
        }
    }
}
