//! 导入任务启动与进度事件。

use std::sync::Arc;

use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, Entry};

impl App {
    /// `/import --dir <路径> [--course <名>]`：启动导入任务（逐文件串行，进度经事件通道上报）。
    pub(crate) fn handle_import_command(&mut self, arg: &str) {
        if self.import_cancel.is_some() {
            self.push_entry(Entry::Error("导入任务进行中，Ctrl+C 可中断".into()));
            return;
        }

        // 全旗标解析：--dir 与 --course 的值可含空格（到下一个 -- 或串尾）
        let mut dir: Option<String> = None;
        let mut course: Option<String> = None;
        crate::course_cmd::collect_flags(arg, &mut |flag, value| match flag {
            "--dir" => dir = Some(value),
            "--course" => course = Some(value),
            _ => {}
        });
        let Some(dir) = dir else {
            self.push_entry(Entry::Error(
                "用法: /import --dir <路径> [--course <课程名>]".into(),
            ));
            return;
        };

        self.run_import(std::path::PathBuf::from(dir), course);
    }

    /// 导入执行（结构化入口：手输解析与向导直连共用）。
    pub(crate) fn run_import(&mut self, dir: std::path::PathBuf, course: Option<String>) {
        if self.import_cancel.is_some() {
            self.push_entry(Entry::Error("导入任务进行中，Ctrl+C 可中断".into()));
            return;
        }
        if !dir.exists() {
            self.push_entry(Entry::Error(format!("目录不存在: {}", dir.display())));
            return;
        }

        let cancel = CancellationToken::new();
        self.import_cancel = Some(cancel.clone());

        let store = Arc::clone(&self.store);
        let provider = Arc::clone(&self.provider);
        let provider_cfg = self.provider_cfg.clone();
        let config = importer::ImportConfig {
            dir: dir.clone(),
            course: course.clone(),
            max_cost: self.max_cost,
        };

        self.push_entry(Entry::Info(format!(
            "开始导入: {}{}",
            dir.display(),
            course
                .as_deref()
                .map(|c| format!(" → 课程 {c}"))
                .unwrap_or_default()
        )));

        let (import_tx, mut import_rx) = unbounded_channel::<importer::ImportEvent>();
        let tx_main = self.tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = import_rx.recv().await {
                let _ = tx_main.send(AppEvent::ImportProgress(ev));
            }
        });
        tokio::spawn(async move {
            importer::import_directory(store, provider, provider_cfg, config, import_tx, cancel)
                .await;
        });
    }

    pub(crate) fn handle_import_event(&mut self, ev: importer::ImportEvent) {
        use importer::ImportEvent::*;
        match ev {
            Started { total, course } => {
                self.push_entry(Entry::Info(format!(
                    "导入开始: 共 {total} 个文件{}",
                    course
                        .as_deref()
                        .map(|c| format!(" → 课程 {c}"))
                        .unwrap_or_default()
                )));
            }
            FileStart { name, index, total } => {
                self.push_entry(Entry::Info(format!("[{index}/{total}] 导入: {name}")));
            }
            FileDone { name, concepts } => {
                self.push_entry(Entry::Info(format!("  ✓ {name} → {} 个概念", concepts)));
            }
            FileSkipped { name, at } => match at {
                Some(detail) => {
                    self.push_entry(Entry::Info(format!("  ⊘ {name}（{detail}）")));
                }
                None => {
                    self.push_entry(Entry::Info(format!("  ⊘ {name}（已存在，跳过）")));
                }
            },
            ChunkFail { name, err } => {
                self.push_entry(Entry::Error(format!(
                    "  ⚠ {name}: 切片入库失败，该笔记暂不可检索: {err}"
                )));
            }
            FileFail { name, err } => {
                self.push_entry(Entry::Error(format!("  ✗ {name}: {err}")));
            }
            Finished { ok, skipped, fail } => {
                self.import_cancel = None;
                self.request_sessions_refresh();
                // 流水线可能在 DB 里新建课程（归类/收编），内存列表必须对齐
                self.request_courses_refresh();
                // 导入期 LLM 调用的花费只落 usage_log，对账进内存熔断口径
                self.request_cost_sync();
                self.push_entry(Entry::Info(format!(
                    "导入完成: ✓ {ok} 新建 / ⊘ {skipped} 跳过 / ✗ {fail} 失败"
                )));
            }
            Cancelled => {
                self.import_cancel = None;
                // 中断前可能已建课/入库，统计同样要对齐
                self.request_courses_refresh();
                self.push_entry(Entry::Info("[导入已中断]".into()));
            }
        }
    }
}
