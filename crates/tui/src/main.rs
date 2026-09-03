//! StudyPilot TUI 入口。

mod app;
mod clipboard;
mod course_cmd;
mod events;
mod highlight;
mod import_path;
mod input_edit;
mod markdown;
mod note_browser;
mod outline_render;
mod palette;
mod review;
mod session_browser;
mod theme;
mod ui;
mod wizard;

use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging()?;

    // 运行时配置（产品形态）：优先 ~/.studypilot/config.toml，回退 cwd config.toml。
    // 缺失/损坏 → 不 crash，进入 App 后由 Settings/Setup 引导配置（文档 §十七）。
    let config_path = agent_providers::Config::runtime_path()?;
    let mut config_file = config_path.clone();
    let mut cfg = match agent_providers::Config::load(&config_path) {
        Ok(c) => c,
        Err(_)
            if config_path.file_name() == Some("config.toml".as_ref())
                && config_path
                    .parent()
                    .map(|p| p.ends_with(".studypilot"))
                    .unwrap_or(false) =>
        {
            // ~/.studypilot 不存在 → 回退 cwd（开发者兼容）
            match agent_providers::Config::load("config.toml") {
                Ok(c) => {
                    config_file = std::path::PathBuf::from("config.toml");
                    c
                }
                Err(_) => agent_providers::Config::default(),
            }
        }
        Err(_) => agent_providers::Config::default(),
    };
    // 无有效 provider 时，用 Model Registry 内置预设填充（每 provider 一个条目，含全部 models）
    if cfg.providers.is_empty() {
        cfg.providers = agent_providers::PRESETS
            .iter()
            .map(|p| {
                let first = p.models.first().map(|m| m.id).unwrap_or("");
                agent_providers::provider_from_preset(p.id, first)
            })
            .collect();
    }
    // 无有效 provider 时，用空占位（Setup/Model 面板可配）；不 crash
    let pc = cfg.default_provider().cloned().unwrap_or_default();

    // D2：DB 操作走 spawn_blocking；这里启动时同步读一次课程列表+统计与累计成本
    let store = Arc::new(storage::Store::open("data/mynotes.db")?);
    // 预算上限：config.toml 的 max_cost 为默认，data/budget.json（/budget 设置）覆盖
    let mut max_cost = cfg.max_cost;
    if let Ok(raw) = std::fs::read_to_string("data/budget.json")
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(m) = v.get("max_cost").and_then(serde_json::Value::as_f64)
        && m > 0.0
    {
        max_cost = m;
    }
    let courses = store.list_courses()?;
    let mut course_stats = std::collections::HashMap::new();
    for (id, _) in &courses {
        if let Ok(stats) = store.course_stats(*id) {
            course_stats.insert(*id, stats);
        }
    }
    let recorded_cost = store.total_recorded_cost().unwrap_or(0.0);
    tracing::info!(count = courses.len(), recorded_cost, "已加载课程");

    let client = Arc::new(agent_providers::OpenAiClient::new(pc.clone())?);

    install_panic_hook();
    let terminal = ratatui::init();
    // 鼠标滚轮需要显式捕获；光标强制块状（终端默认常为竖线，不易对位）
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture,
        crossterm::cursor::SetCursorStyle::SteadyBlock
    )?;
    let result = {
        let mut app = app::App::new(client, store, pc, cfg.providers.clone(), max_cost, courses);
        app.config_file = config_file;
        app.sidebar_course_stats = course_stats;
        // R6：状态栏显示含历史的累计成本，预算熔断按累计值判断
        app.total_cost = recorded_cost;
        // 恢复上次所在课程 → 分发首屏卡（空库 Welcome / 已有课程轻量 context 卡）
        app.restore_last_course();
        app.push_startup_cards();
        app::run(terminal, app).await
    };
    restore_terminal();
    result
}

/// 恢复终端：关鼠标捕获 + 光标还原 + ratatui 恢复 raw mode/alternate screen。
fn restore_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::cursor::SetCursorStyle::DefaultUserShape
    );
    ratatui::restore();
}

/// TUI 里禁止 println!：日志写文件（data/tui.log，RUST_LOG 控制）。
fn init_logging() -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("data/tui.log")?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(Mutex::new(log))
        .with_ansi(false)
        .init();
    Ok(())
}

/// panic 时也必须恢复终端状态，并把错误记录到日志。
fn install_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("PANIC: {info}");
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::SetCursorStyle::DefaultUserShape
        );
        ratatui::restore();
        eprintln!("发生内部错误已退出，详情见 data/tui.log");
        hook(info);
    }));
}
