//! 导入任务启动与进度事件。

use std::sync::Arc;

use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, Entry};

impl App {
    /// canonical：打开导入向导（palette / Course 页 / Home 共用）。
    /// 课程预填当前分区；向导完成后经 `finish_wizard` 调 `run_import`。
    pub(crate) fn open_import_wizard(&mut self) {
        self.enter_session_workspace();
        if self.import_cancel.is_some() {
            self.push_entry(Entry::Error("导入任务进行中，Ctrl+C 可中断".into()));
            return;
        }
        self.take_input_for_overlay();
        self.wizard = Some(crate::wizard::Wizard::new_import(self.course.clone()));
        self.enter_wizard_step();
    }

    /// `/import <路径>` 或 `/import --dir <路径> [--course <名>]`：
    /// 统一路径解析（绝对/相对任意位置，文档 import-path），无参 → 向导。
    /// 单文件与目录都支持（核心 walkdir 兼容两者）。
    pub(crate) fn handle_import_command(&mut self, arg: &str) {
        // 导入输出进聊天流：从 Course/Home 发起时切到 Session workspace
        self.enter_session_workspace();
        if self.import_cancel.is_some() {
            self.push_entry(Entry::Error("导入任务进行中，Ctrl+C 可中断".into()));
            return;
        }

        // 无参 → canonical 向导（UI/CLI 同一入口，消除"拼命令→解析→handler"路径）
        if arg.trim().is_empty() {
            self.open_import_wizard();
            return;
        }

        // 全旗标解析：--dir 与 --course 的值可含空格（到下一个 -- 或串尾）；
        // 兼容 `/import <裸路径>`（无旗标 = 首 token 起为路径）
        let mut dir: Option<String> = None;
        let mut course: Option<String> = None;
        let mut saw_flag = false;
        crate::course_cmd::collect_flags(arg, &mut |flag, value| match flag {
            "--dir" => {
                dir = Some(value);
                saw_flag = true;
            }
            "--course" => course = Some(value),
            _ => {}
        });
        // 裸路径：整段（含空格）作为路径；带 --dir 时取其值
        let path_input = if saw_flag {
            dir.clone().unwrap_or_default()
        } else {
            arg.trim().trim_matches('"').to_owned()
        };

        // 统一解析（绝对/相对、存在性、类型、空目录）
        let target = match crate::import_path::resolve_import_path(&path_input) {
            Ok(t) => t,
            Err(e) => {
                self.push_entry(Entry::Error(e.to_string()));
                return;
            }
        };
        self.run_import(target, course);
    }

    /// 导入执行（结构化入口：手输解析与向导直连共用）。
    /// `target` 已经过 `resolve_import_path` 校验（存在/类型/空目录）。
    pub(crate) fn run_import(
        &mut self,
        target: crate::import_path::ImportTarget,
        course: Option<String>,
    ) {
        if self.import_cancel.is_some() {
            self.push_entry(Entry::Error("导入任务进行中，Ctrl+C 可中断".into()));
            return;
        }
        let dir = target.path;
        let kind = if target.is_file { "文件" } else { "目录" };
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
            "开始导入{}: {}{}",
            kind,
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
