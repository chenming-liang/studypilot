//! mynotes-agent TUI 入口。

mod app;
mod clipboard;
mod course_cmd;
mod events;
mod input_edit;
mod markdown;
mod outline_render;
mod palette;
mod review;
mod ui;
mod wizard;

use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging()?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let config_path = args.first().map(String::as_str).unwrap_or("config.toml");
    let cfg = agent_providers::Config::load(config_path)?;
    let pc = cfg.default_provider()?.clone();

    // D2：DB 操作走 spawn_blocking；这里启动时同步读一次课程列表+统计与累计成本
    let store = Arc::new(storage::Store::open("data/mynotes.db")?);
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
        let mut app = app::App::new(
            client,
            store,
            pc,
            cfg.providers.clone(),
            cfg.max_cost,
            courses,
        );
        app.sidebar_course_stats = course_stats;
        // R6：状态栏显示含历史的累计成本，预算熔断按累计值判断
        app.total_cost = recorded_cost;
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
