//! `tmd`, the travel monitor daemon. See the library's documentation.

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use tm_browser::{ChromiumConfig, Display};
use tm_daemon::{
    Config,
    Daemon,
    candidates::Only,
    cli::Cli,
    jobs::JobOptions,
    notify::{Desktop, Notifier},
    sources::browser::{BrowserSources, site_config},
};
use tm_domain::FamilyId;
use tm_store::Store;
use tokio::sync::watch;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[allow(
    clippy::disallowed_methods,
    reason = "the runtime's entry point: nothing runs on it yet"
)]
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let db = cli.db_path();
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let store = Store::open(&db)
        .await
        .with_context(|| format!("cannot open {}", db.display()))?;
    info!(db = %db.display(), "database open");

    let mut chromium =
        ChromiumConfig::new(cli.chromium_path(), cli.profile_path());
    chromium.display = if cli.headed {
        Display::Window
    } else {
        Display::Headless
    };
    chromium.port = cli.cdp_port;
    // Each site gets its own profile under --profile-dir and its own
    // Chromium process (0062); LATAM's runs headed offscreen (0081), its
    // cash and awards on LATAM Pass's profile (0084).
    let sources =
        move |site: &str| BrowserSources::new(site_config(&chromium, site));

    let config = Config {
        tick: cli.tick(),
        once: cli.once,
        only: Only {
            family: cli.only_family.map(FamilyId),
            destination: cli.only_route.clone(),
        },
        pace: cli.pace,
        parallel: usize::from(cli.parallel),
        jobs: JobOptions {
            max_graph_pages: cli.max_graph_pages,
            origin_override: cli.google_origin.clone(),
            latam_base: cli.latam_origin.clone(),
            latam_round_trips: cli.latam_round_trips,
            ..JobOptions::default()
        },
        budget: cli.budget,
        max_budget: cli.max_budget,
        site_budgets: cli.site_budgets.clone(),
        profile_dir: Some(cli.profile_path()),
        idle_close: cli.idle_close(),
        notifier: (!cli.no_notify)
            .then(|| Arc::new(Desktop) as Arc<dyn Notifier>),
        latam_strip: cli.latam_strip,
        ..Config::default()
    };

    let (stop, shutdown) = watch::channel(false);
    tokio::spawn(async move {
        wait_for_signal().await;
        info!("signal received: finishing the current pages");
        let _ = stop.send(true);
    });

    let mut daemon = Daemon::new(store.clone(), sources, config, shutdown);
    let result = daemon.run().await;
    if let Err(e) = &result {
        warn!("the daemon stopped with an error: {e:#}");
        daemon.stop().await;
    }
    store.close().await;
    result
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
