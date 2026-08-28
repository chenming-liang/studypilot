//! 导入流水线编排：walkdir 收集 → 解析 → LLM 抽取 → 入库（幂等 + 容错 + 可中断）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_providers::{OpenAiClient, ProviderConfig};
use storage::{InsertOutcome, NewChunk, NewNote, Store};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

use crate::events::ImportEvent;
use crate::extract;
use crate::parser;

pub struct ImportConfig {
    pub dir: PathBuf,
    /// 已指定课程名（跳过 LLM 归类）；None = 让 LLM 判断
    pub course: Option<String>,
    /// 预算熔断阈值（R6）
    pub max_cost: f64,
}

/// 导入一个目录。逐文件串行处理（LLM 有速率/费用约束），进度经 mpsc 上报，
/// CancellationToken 随时可停。
pub async fn import_directory(
    store: Arc<Store>,
    provider: Arc<OpenAiClient>,
    provider_cfg: ProviderConfig,
    config: ImportConfig,
    tx: UnboundedSender<ImportEvent>,
    cancel: CancellationToken,
) {
    let files = collect_files(&config.dir);
    let total = files.len();
    let _ = tx.send(ImportEvent::Started {
        total,
        course: config.course.clone(),
    });

    if total == 0 {
        let _ = tx.send(ImportEvent::Finished {
            ok: 0,
            skipped: 0,
            fail: 0,
        });
        return;
    }

    let mut ok = 0;
    let mut skipped = 0;
    let mut fail = 0;

    for (i, path) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            let _ = tx.send(ImportEvent::Cancelled);
            break;
        }

        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_owned();
        let _ = tx.send(ImportEvent::FileStart {
            name: name.clone(),
            index: i + 1,
            total,
        });

        match process_one(
            &store,
            &provider,
            &provider_cfg,
            path,
            &config,
            &cancel,
            &tx,
        )
        .await
        {
            Ok(ProcessOutcome::Created(n)) => {
                ok += 1;
                let _ = tx.send(ImportEvent::FileDone {
                    name: name.clone(),
                    concepts: n,
                });
            }
            Ok(ProcessOutcome::Skipped) => {
                skipped += 1;
                let _ = tx.send(ImportEvent::FileSkipped {
                    name: name.clone(),
                    at: None,
                });
            }
            Ok(ProcessOutcome::SkippedWith(at)) => {
                skipped += 1;
                let _ = tx.send(ImportEvent::FileSkipped {
                    name: name.clone(),
                    at: Some(at),
                });
            }
            Err(e) => {
                fail += 1;
                tracing::warn!(file = %name, "导入失败: {e}");
                let _ = tx.send(ImportEvent::FileFail {
                    name: name.clone(),
                    err: e.to_string(),
                });
            }
        }
    }

    let _ = tx.send(ImportEvent::Finished { ok, skipped, fail });
}

enum ProcessOutcome {
    Created(usize),
    Skipped,
    /// 去重跳过，附存活位置描述
    SkippedWith(String),
}

async fn process_one(
    store: &Arc<Store>,
    provider: &Arc<OpenAiClient>,
    provider_cfg: &ProviderConfig,
    path: &Path,
    config: &ImportConfig,
    cancel: &CancellationToken,
    tx: &UnboundedSender<ImportEvent>,
) -> Result<ProcessOutcome, String> {
    // ① 解析格式（pdf/pptx 走 spawn_blocking）
    let path_owned = path.to_owned();
    let raw = tokio::task::spawn_blocking(move || parser::parse_file(&path_owned))
        .await
        .map_err(|e| format!("任务错误: {e}"))?
        .map_err(|e| e.to_string())?;

    if cancel.is_cancelled() {
        return Ok(ProcessOutcome::Skipped);
    }

    // ② LLM 概念抽取（异步）；D2：Store 调用经 spawn_blocking
    let mut import_cost = 0.0f64;
    let extracted = extract::extract(
        provider,
        provider_cfg,
        Arc::clone(store),
        &mut import_cost,
        config.max_cost,
        &raw,
        config.course.as_deref(),
        cancel,
    )
    .await;

    // ③ 入库（spawn_blocking，D2）
    let store = Arc::clone(store);
    let path_owned = path.to_owned();
    let tx_warn = tx.clone();
    tokio::task::spawn_blocking(move || -> Result<ProcessOutcome, String> {
        let (course_id, concepts, title) =
            match &extracted {
                Some(ext) => {
                    // 归课失败要有痕：查询/创建任何一环出错，笔记将静默落 all 区
                    let cid =
                        ext.course.as_deref().and_then(|c| {
                            match store.find_course(c) {
                    Ok(Some(id)) => Some(id),
                    Ok(None) => store.get_or_create_course(c).map_err(|e| {
                        tracing::warn!(course = %c, "课程创建失败，笔记将落入 all 区: {e}");
                        e
                    }).ok(),
                    Err(e) => {
                        tracing::warn!(course = %c, "课程查询失败，笔记将落入 all 区: {e}");
                        None
                    }
                }
                        });
                    (cid, ext.concepts.clone(), ext.title.clone())
                }
                None => (None, Vec::new(), None),
            };

        let title = title.unwrap_or_else(|| raw.title.clone());
        let source_path = path_owned.to_str().map(|s| s.to_owned());

        let outcome = store
            .insert_note(NewNote {
                course_id,
                title: &title,
                source_path: source_path.as_deref(),
                content: &raw.text,
                fts_content: Some(&raw.fts_text),
            })
            .map_err(|e| e.to_string())?;

        match outcome {
            InsertOutcome::Duplicate {
                existing_id,
                existing_title,
                existing_course_id,
            } => {
                // 透明化：全局 content_hash 去重下，用户需要知道存活位置才能删干净再导
                let at = existing_course_id.and_then(|cid| {
                    store
                        .list_courses()
                        .ok()?
                        .into_iter()
                        .find(|(i, _)| *i == cid)
                        .map(|(_, name)| name)
                });
                let detail = match at {
                    Some(name) => format!("已存在：{name}#{existing_id} {existing_title}"),
                    None => format!("已存在：all#{existing_id} {existing_title}"),
                };
                Ok(ProcessOutcome::SkippedWith(detail))
            }
            InsertOutcome::Created(note) => {
                // 概念关联（失败要有痕：该概念将游离于检索 boosting/掌握度闭环之外）
                for concept_name in &concepts {
                    match store.get_or_create_concept(concept_name, course_id) {
                        Ok(cid) => {
                            if let Err(e) = store.link_note_concept(note.id, cid) {
                                tracing::error!(
                                    note_id = note.id,
                                    concept = %concept_name,
                                    "笔记-概念关联失败: {e}"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                note_id = note.id,
                                concept = %concept_name,
                                "概念入库失败: {e}"
                            );
                        }
                    }
                }

                // chunk 拆分 + 入库：概念名只进 FTS 索引（fts_extra），不污染正文显示
                let chunks_raw = crate::clean::split_into_chunks(&raw.fts_text);
                let concept_suffix = if concepts.is_empty() {
                    None
                } else {
                    Some(concepts.join(" "))
                };
                let chunks: Vec<NewChunk> = chunks_raw
                    .iter()
                    .map(|(heading, body)| NewChunk {
                        heading: heading.as_str(),
                        content: body.as_str(),
                        fts_extra: concept_suffix.as_deref(),
                    })
                    .collect();
                if let Err(e) = store.insert_chunks(note.id, &chunks)
                    && !chunks.is_empty()
                {
                    // chunk 失败 = 笔记无法被检索/出题，必须留痕诊断并向用户可见
                    tracing::error!(
                        note_id = note.id,
                        title = %title,
                        "chunk 入库失败（该笔记将不可检索）: {e}"
                    );
                    let _ = tx_warn.send(ImportEvent::ChunkFail {
                        name: title.clone(),
                        err: e.to_string(),
                    });
                }
                Ok(ProcessOutcome::Created(concepts.len()))
            }
        }
    })
    .await
    .map_err(|e| format!("任务错误: {e}"))?
}

/// walkdir 递归收集 md/txt/pdf/pptx 文件（排序后确定性顺序）。
fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| {
                    matches!(
                        x.to_ascii_lowercase().as_str(),
                        "md" | "txt" | "pdf" | "pptx"
                    )
                })
                .unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect();
    files.sort();
    files
}
