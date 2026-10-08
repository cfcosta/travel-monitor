//! The SQLite database: configuration, query log, observations, deals
//! and app requests (DECISIONS.md 0020, 0035, 0046).
//!
//! [`ChangeWatch`] tells the app when another program wrote (0057).
//!
//! One file, opened in WAL mode so the gpui app reads while the daemon
//! writes (0040). Every query is checked at compile time by sqlx; the
//! offline metadata in `.sqlx/` lets the crate build without a database
//! (`SQLX_OFFLINE=true`).
//!
//! # Explore families
//!
//! A `FamilyKey` needs a route, so each origin gets a *sweep
//! pseudo-route*: a `routes` row with `sweep = 1` whose destination is
//! the origin itself. `sync_routes` keeps one per origin (active while
//! the origin is), and `sync_families` gives it one
//! `FamilyKind::Explore` family per cabin in {economy, business} and per
//! active point of sale, all on Google Flights (0025, 0033).
//! [`Store::list_routes`] leaves sweep routes out;
//! [`Store::sweep_routes`] lists them, [`Store::route`] returns either.

mod config;
mod convert;
mod daemon;
mod deals;
mod error;
mod families;
mod finds;
mod observations;
mod queries;
mod records;
mod requests;
mod seed;
mod watch;

use std::{path::Path, str::FromStr, time::Duration};

pub use error::{Result, StoreError};
pub use finds::ExploreDrop;
pub use records::{
    AirlineSource,
    AwardRate,
    BlockRecord,
    CheckInProgress,
    DealFilter,
    Destination,
    ExchangeRate,
    FamilyRecord,
    FamilyStateRecord,
    FamilySync,
    Heartbeat,
    LatestCell,
    LoginState,
    MilesProgram,
    NewPlace,
    Origin,
    PriceCell,
    QueryLogEntry,
    RequestKind,
    RequestRecord,
    RequestStatus,
    RouteTripState,
    SiteBudget,
    SiteStatus,
};
use sqlx::{
    Sqlite,
    SqlitePool,
    Transaction,
    migrate::Migrator,
    sqlite::{
        SqliteConnectOptions,
        SqliteJournalMode,
        SqlitePoolOptions,
        SqliteSynchronous,
    },
};
pub use watch::ChangeWatch;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// How long a connection waits for another writer (the daemon or the
/// app) before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// The database. Cheap to clone: clones share one connection pool.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens the database at `path`, creating it if missing, in WAL mode
    /// with foreign keys on, and runs the migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(BUSY_TIMEOUT)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// A private in-memory database, for tests. One connection, kept
    /// open for the pool's life (closing it would drop the database).
    pub async fn open_in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Closes every connection (checkpointing the WAL).
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// A counter bumped by every configuration change (places, points of
    /// sale, settings, airline and miles sources, a route's additive
    /// flag). The daemon reloads and re-syncs families when it moves
    /// (0035).
    pub async fn config_version(&self) -> Result<i64> {
        let version = sqlx::query_scalar!(
            r#"SELECT config_version AS "v!: i64" FROM meta WHERE id = 1"#
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(version)
    }

    /// A write transaction. `BEGIN IMMEDIATE` takes the write lock up
    /// front, so a transaction that reads before it writes cannot fail
    /// to upgrade when another process writes meanwhile.
    async fn write(&self) -> Result<Transaction<'static, Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }
}
