//! Docket: track GitHub PRs and produce Claude-assisted reviews to edit and post.

mod agent;
mod config;
mod github;
mod markdown;
mod preflight;
mod repo;
mod service;
mod state;
mod store;
mod ui;

use dioxus::desktop::tao::dpi::{LogicalPosition, LogicalSize};
use dioxus::desktop::tao::platform::macos::WindowBuilderExtMacOS;
use dioxus::desktop::{Config, WindowBuilder};

fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = config::paths::log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let appender = tracing_appender::rolling::daily(&dir, "docket.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "docket=info,warn".into()),
        )
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Some(guard)
}

fn main() {
    let _guard = init_logging();
    let store = match store::Store::open(&config::paths::db_file()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("can't open the database: {e:#}");
            std::process::exit(1);
        }
    };

    let mut wb = WindowBuilder::new()
        .with_title("Docket")
        .with_min_inner_size(LogicalSize::new(900.0, 560.0))
        .with_inner_size(LogicalSize::new(1320.0, 860.0))
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
        .with_transparent(true);
    if let Some(frame) = store.ui_get("window") {
        let v: Vec<f64> = frame.split(',').filter_map(|x| x.parse().ok()).collect();
        if let [x, y, w, h] = v[..] {
            wb = wb.with_position(LogicalPosition::new(x, y)).with_inner_size(LogicalSize::new(w, h));
        }
    }
    service::init_svc(store);

    let cfg = Config::new()
        .with_window(wb)
        .with_menu(ui::menus::build())
        .with_custom_head(ui::styles::head())
        .with_background_color((0, 0, 0, 0));

    dioxus::LaunchBuilder::desktop().with_cfg(cfg).launch(ui::App);
}
