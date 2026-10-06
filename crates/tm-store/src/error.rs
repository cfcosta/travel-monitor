//! What can go wrong in the store.

use thiserror::Error;
use tm_domain::settings::SettingsError;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("invalid settings: {0}")]
    Settings(#[from] SettingsError),
    #[error("{what} {id} not found")]
    NotFound { what: &'static str, id: String },
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A row holds something the schema's checks should have kept out
    /// (an unknown enum name, a negative count).
    #[error("corrupt row: {0}")]
    Corrupt(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl StoreError {
    pub(crate) fn not_found(what: &'static str, id: impl ToString) -> Self {
        Self::NotFound {
            what,
            id: id.to_string(),
        }
    }
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;
