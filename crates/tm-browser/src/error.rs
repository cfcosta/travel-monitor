//! Errors from the browser, the protocol and the pages.

use std::time::Duration;

/// Why a browser operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Chromium could not be started, or exited before it offered its
    /// DevTools endpoint.
    #[error("Chromium did not start: {0}")]
    Launch(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The WebSocket to the browser failed.
    #[error("DevTools connection failed: {0}")]
    WebSocket(String),

    /// The browser closed the connection, or it was shut down.
    #[error("the DevTools connection is closed")]
    Closed,

    /// The browser answered a command with an error.
    #[error("CDP {method} failed: {message}")]
    Protocol { method: String, message: String },

    /// A response did not have the shape the protocol documents.
    #[error("unexpected CDP response to {method}: {message}")]
    Malformed { method: String, message: String },

    /// Something did not happen in time. `what` says what was awaited.
    #[error("timed out after {after:?} waiting for {what}")]
    Timeout { what: String, after: Duration },

    /// A navigation failed before any document loaded, such as
    /// `net::ERR_CONNECTION_REFUSED`.
    #[error("navigation to {url} failed: {error}")]
    Navigation { url: String, error: String },

    /// A script threw. `message` is the exception's description.
    #[error("script failed: {0}")]
    Script(String),

    /// A script's result did not deserialize into the requested type.
    #[error("script result has an unexpected shape: {0}")]
    Decode(#[from] serde_json::Error),

    /// No element matches a selector.
    #[error("no element matches {0}")]
    NotFound(String),

    /// An element exists but cannot receive input: hidden, disabled,
    /// outside the viewport or covered by another element.
    #[error("{selector} cannot receive input: {reason}")]
    NotInteractable { selector: String, reason: String },

    /// [`crate::Page::press_key`] does not know this key.
    #[error("unsupported key {0:?}")]
    UnsupportedKey(String),
}

impl Error {
    pub(crate) fn timeout(what: impl Into<String>, after: Duration) -> Self {
        Self::Timeout {
            what: what.into(),
            after,
        }
    }

    pub(crate) fn malformed(
        method: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Malformed {
            method: method.into(),
            message: message.into(),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
