//! `tm`, the travel monitor app (DECISIONS.md 0017, 0040, 0044).
//!
//! Reads the daemon's SQLite file and shows deals on a night map, the
//! routes, their price calendars, the daemon's activity and the
//! settings. Writes only configuration and requests for the daemon.
//!
//! - `--db <path>` (or `TM_DB`): the database; by default
//!   `$XDG_DATA_HOME/travel-monitor/tm.db`.
//! - `--fake`: sample data shaped like the designs, no database.
//! - `--profile-dir`, `--chromium` (or `TM_PROFILE_DIR`, `TM_CHROMIUM`):
//!   the daemon's Chromium profile and binary, for miles logins.

mod data;
mod format;
mod geo;
mod grid;
mod land_dots;
mod login;
mod notify;
mod theme;
mod ui;

use std::{path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use clap::Parser;
use gpui::{
    App,
    AppContext,
    Bounds,
    TitlebarOptions,
    WindowBounds,
    WindowOptions,
    px,
    size,
};

use crate::{
    data::SharedBackend,
    login::LoginConfig,
    notify::{Desktop, Notifier, Recorder},
    ui::{AppEnv, TravelMonitor},
};

#[derive(Debug, Parser)]
#[command(name = "tm", about = "The travel monitor app")]
struct Args {
    /// The daemon's database.
    #[arg(long, env = "TM_DB")]
    db: Option<PathBuf>,
    /// Show sample data instead of a database.
    #[arg(long)]
    fake: bool,
    /// The daemon's Chromium profile (miles logins).
    #[arg(long)]
    profile_dir: Option<PathBuf>,
    /// The Chromium binary for miles logins.
    #[arg(long)]
    chromium: Option<PathBuf>,
    /// Seconds between refreshes.
    #[arg(long, default_value_t = 5)]
    refresh: u64,
    /// No desktop notifications.
    #[arg(long)]
    quiet: bool,
}

fn default_db() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("travel-monitor")
        .join("tm.db")
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tm=info,warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    // sqlx and the login window's process run on tokio; the interface
    // only awaits their results.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("tm-io")
        .enable_all()
        .build()?;
    let backend: SharedBackend = if args.fake {
        Arc::new(data::fake::FakeBackend::new())
    } else {
        let path = args.db.clone().unwrap_or_else(default_db);
        tracing::info!("database: {}", path.display());
        data::store::backend(path, runtime.handle().clone())
    };
    let notifier: Rc<dyn Notifier> = if args.quiet {
        Rc::new(Recorder::default())
    } else {
        Rc::new(Desktop)
    };
    let env = AppEnv {
        backend,
        notifier,
        login: LoginConfig::resolve(
            args.profile_dir.clone(),
            args.chromium.clone(),
        ),
        tokio: Some(runtime.handle().clone()),
        refresh_every: Duration::from_secs(args.refresh.max(1)),
    };
    gpui_platform::application().run(move |cx: &mut App| {
        theme::Fonts::install(cx);
        ui::text_field::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("travel monitor".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(960.), px(640.))),
            app_id: Some("travel-monitor".into()),
            ..Default::default()
        };
        if let Err(error) = cx.open_window(options, |_, cx| {
            cx.new(|cx| TravelMonitor::new(env, cx))
        }) {
            tracing::error!("cannot open a window: {error:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
    drop(runtime);
    Ok(())
}
