//! Noticing another program's writes (DECISIONS.md 0057).
//!
//! SQLite bumps `PRAGMA data_version` on a connection whenever *another*
//! connection (in this process or another one) commits to the file. A
//! [`ChangeWatch`] keeps one connection of its own, outside the pool, so
//! its counter moves for every commit the daemon or the app makes, and
//! reading it costs one tiny query.

use std::path::Path;

use sqlx::{ConnectOptions, SqliteConnection, sqlite::SqliteConnectOptions};

use crate::{BUSY_TIMEOUT, Result};

/// A dedicated connection that tells whether the database changed.
pub struct ChangeWatch {
    conn: SqliteConnection,
    last: Option<i64>,
}

impl std::fmt::Debug for ChangeWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangeWatch")
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

impl ChangeWatch {
    /// Opens a connection to the existing database at `path`. Nothing is
    /// created or migrated: [`crate::Store::open`] does that.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .busy_timeout(BUSY_TIMEOUT)
            .connect()
            .await?;
        Ok(Self { conn, last: None })
    }

    /// The connection's `PRAGMA data_version`: it moves when another
    /// connection commits.
    pub async fn data_version(&mut self) -> Result<i64> {
        let version: i64 = sqlx::query_scalar("PRAGMA data_version")
            .fetch_one(&mut self.conn)
            .await?;
        Ok(version)
    }

    /// Whether another connection committed since the last call. The
    /// first call only takes the baseline and answers `false`.
    pub async fn changed(&mut self) -> Result<bool> {
        let version = self.data_version().await?;
        let changed = self.last.is_some_and(|last| last != version);
        self.last = Some(version);
        Ok(changed)
    }
}
