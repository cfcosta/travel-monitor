//! `tmd`, the travel monitor daemon. See the library's documentation.

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use tm_browser::ChromiumConfig;
use tm_daemon::{
    Config,
    Daemon,
    candidates::Only,
    cli::Cli,
    jobs::JobOptions,
    sources::browser::BrowserSources,
};
use tm_domain::FamilyId;
use tm_jev::{Jev, TypeSafe};
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

    let jev: Option<Arc<dyn Jev>> = match &cli.jev_key_file {
        Some(path) => {
            let client =
                TypeSafe::from_key_file(path)?.model(cli.jev_model.clone());
            info!(model = %cli.jev_model, "Jev configured");
            Some(Arc::new(client))
        }
        None => {
            info!("no Jev key: airline and miles sources and recovery are off");
            None
        }
    };

    let mut chromium =
        ChromiumConfig::new(cli.chromium_path(), cli.profile_path());
    chromium.headless = !cli.headed;
    chromium.port = cli.cdp_port;
    let sources = BrowserSources::new(chromium, jev);

    let config = Config {
        tick: cli.tick(),
        once: cli.once,
        only: Only {
            family: cli.only_family.map(FamilyId),
            destination: cli.only_route.clone(),
        },
        pace: cli.pace,
        jobs: JobOptions {
            max_graph_pages: cli.max_graph_pages,
            origin_override: cli.google_origin.clone(),
            ..JobOptions::default()
        },
        budget: cli.budget,
        max_budget: cli.max_budget,
        ..Config::default()
    };

    let (stop, shutdown) = watch::channel(false);
    tokio::spawn(async move {
        wait_for_signal().await;
        info!("signal received: finishing the current page");
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
