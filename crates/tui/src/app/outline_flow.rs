//! 复习地图（Review Map）流程：持久缓存 + 签名自愈 + 出题入口。
//!
//! `/outline` 与 `/review-map` 共用 `ensure_review_map`：
//! - 概念签名匹配 → 缓存命中（零 LLM），重读掌握度保证状态新鲜
//! - 概念签名变化（导入/刷新/删除）→ 自动重跑 LLM 重组章节并覆盖缓存
//!
//! 两者只差渲染后行为：/outline 只渲染（看地图），/review-map 渲染 + 弹选择器（复习）。
//! `--regen`（仅 /outline）无视缓存强制重跑。

use std::sync::Arc;

use agent_providers::{OpenAiClient, ProviderConfig};
use storage::Store;
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use super::{App, AppEvent, Entry};
use crate::outline_render::{OutlinePayload, ReviewMap};

/// 统一入口：签名自愈的复习地图获取。
/// 返回 (map, regenerated)——regenerated = 本次实际跑了 LLM（缓存未命中/签名变化/强制）。
#[allow(clippy::too_many_arguments)]
async fn ensure_review_map(
    store: Arc<Store>,
    provider: Arc<OpenAiClient>,
    provider_cfg: ProviderConfig,
    course_id: i64,
    course_name: String,
    force_regen: bool,
    cancel: &CancellationToken,
) -> Result<(ReviewMap, bool), String> {
    // ① 当前概念快照 + 签名 + 今日作答（spawn_blocking，D2）
    let store_g = Arc::clone(&store);
    let (concepts, today, today_ids) =
        spawn_blocking(move || -> storage::Result<(_, usize, Vec<i64>)> {
            let concepts = store_g.list_concepts_with_mastery(Some(course_id))?;
            let today = store_g.attempts_today_by_course(course_id)?;
            let ids = store_g.concept_ids_reviewed_today(course_id)?;
            Ok((concepts, today, ids))
        })
        .await
        .map_err(|e| format!("任务错误: {e}"))?
        .map_err(|e| e.to_string())?;

    let signature =
        importer::review_map::signature_of(concepts.iter().map(|c| c.name.clone()).collect());

    // ② 缓存命中：签名一致 → 重读状态直接返回（零 LLM）
    if !force_regen
        && let Some(cached) = importer::review_map::load_outline_cache(course_id)?
        && cached.signature == signature
    {
        let map = cached
            .map
            .with_refreshed_status(&concepts)
            .with_today(today)
            .with_today_reviewed(today_ids.clone())
            .clone();
        return Ok((map, false));
    }

    // ③ 缓存未命中/签名变化/强制：LLM 重组 + 写缓存
    let map = importer::review_map::build_review_map(
        Arc::clone(&store),
        provider,
        provider_cfg,
        course_id,
        course_name,
        cancel,
    )
    .await?;
    importer::review_map::save_outline_cache(course_id, signature, &map)?;
    let map = map
        .with_refreshed_status(&concepts)
        .with_today(today)
        .clone();
    Ok((map, true))
}

impl App {
    /// `/outline [课程] [--export] [--regen]`：看地图。
    /// 概念驱动 + 持久缓存 + 签名自愈（详见 ensure_review_map）。
    pub(crate) fn handle_outline_command(&mut self, arg: &str) {
        let force_regen = arg.contains("--regen");
        let export = arg.contains("--export");
        let name = arg
            .replace("--export", "")
            .replace("--regen", "")
            .trim()
            .to_owned();
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
        // 生成期间挂 inflight：header 显示进行中，Ctrl+C 可中断
        let cancel = CancellationToken::new();
        self.inflight = Some(cancel.clone());
        if force_regen {
            self.push_entry(Entry::Info("强制重新组织章节…".into()));
        }

        tokio::spawn(async move {
            let result = ensure_review_map(
                store,
                provider,
                provider_cfg,
                course_id,
                course_name,
                force_regen,
                &cancel,
            )
            .await;
            let _ = tx.send(AppEvent::OutlineReady(result.map(|(map, regenerated)| {
                OutlinePayload {
                    map,
                    export,
                    regenerated,
                }
            })));
        });
    }

    /// 复习地图选择器（复用 ListPicker，Enter = 对该概念出题）。
    pub(crate) fn open_review_map_picker(&mut self) {
        if self.review_map.is_some() {
            self.open_list_picker(crate::palette::PickKind::ReviewMap);
        }
    }

    /// `/review-map [数量]` —— 自愈式出题入口：缓存命中秒开；签名变化自动重跑 LLM。
    /// 数量 = 选择器普通行出题的 --n（默认 5；「优先巩固」批量项数量 = 待巩固概念数）。
    /// 参数支持 `3` 或 `--n 3` 两种写法。
    pub(crate) fn handle_review_map_command(&mut self, arg: &str) {
        let trimmed = arg.trim();
        let n = trimmed
            .strip_prefix("--n")
            .map(str::trim)
            .unwrap_or(trimmed)
            .parse::<usize>()
            .unwrap_or(0);
        self.review_map_n = n;
        let name = self.course.clone();
        if name == "all" {
            self.push_entry(Entry::Error("复习地图需指定具体课程，不能为 all".into()));
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
        let cancel = CancellationToken::new();
        self.inflight = Some(cancel.clone());

        tokio::spawn(async move {
            let result = ensure_review_map(
                store,
                provider,
                provider_cfg,
                course_id,
                name,
                false,
                &cancel,
            )
            .await;
            let _ = tx.send(AppEvent::ReviewMapReady(
                result.map(|(map, _)| map).map_err(|e| e.to_string()),
            ));
        });
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
                    let source = if payload.regenerated {
                        "· 已按最新概念重新组织章节"
                    } else {
                        "· 缓存命中（概念未变化）· 输入 /review-map 选择知识点开始复习"
                    };
                    self.push_entry(Entry::Info(source.into()));
                }
            }
            Err(e) => {
                self.push_entry(Entry::Error(format!("复习地图生成失败: {e}")));
            }
        }
    }
}
