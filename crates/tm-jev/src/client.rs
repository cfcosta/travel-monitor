//! The HTTP client for TypeSafe's System One endpoint.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use rustls::{ClientConfig, RootCertStore, crypto::ring};
use serde_json::json;

use crate::{
    DEFAULT_MODEL,
    Jev,
    JevError,
    Request,
    Response,
    SYSTEM_ONE_URL,
    retry::{RetryPolicy, jitter, parse_retry_after},
};

/// The environment variable [`TypeSafe::from_env`] reads.
pub const API_KEY_VAR: &str = "TYPESAFE_API_KEY";

/// How long one attempt may take, connecting included. Jev answers in a
/// fraction of a second, so a request this slow is not coming back.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// The key in `value`, trimmed, unless it is missing or blank.
pub fn api_key(value: Option<String>) -> Result<String, MissingApiKey> {
    value
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .ok_or(MissingApiKey)
}

/// The key in the file at `path`, trimmed. The daemon reads its key this
/// way, so it never sits in the Nix store or the environment (DECISIONS.md
/// 0037). A file that cannot be read, or holds only whitespace, is an
/// error. Errors name the path, never the contents.
///
/// It reads the file synchronously: call it at start-up, or from
/// `spawn_blocking`.
pub fn api_key_from_file(
    path: impl AsRef<Path>,
) -> Result<String, ApiKeyFileError> {
    let path = path.as_ref();
    let contents = std::fs::read_to_string(path).map_err(|source| {
        ApiKeyFileError::Read {
            path: path.to_owned(),
            source,
        }
    })?;
    api_key(Some(contents)).map_err(|MissingApiKey| ApiKeyFileError::Empty {
        path: path.to_owned(),
    })
}

/// `TYPESAFE_API_KEY` is not set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingApiKey;

impl fmt::Display for MissingApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{API_KEY_VAR} is not set")
    }
}

impl std::error::Error for MissingApiKey {}

/// Why [`api_key_from_file`] has no key.
#[derive(Debug, thiserror::Error)]
pub enum ApiKeyFileError {
    #[error("cannot read the Jev API key file {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the Jev API key file {} is empty", path.display())]
    Empty { path: PathBuf },
}

/// rustls with the `ring` provider and Mozilla's roots, as `tau_ai`
/// builds it: no system certificate store, and no process-wide default
/// provider to install.
fn tls_config() -> ClientConfig {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Jev over HTTPS. Clones share one connection pool.
///
/// Status 429, 503 and 529 are retried under the client's
/// [`RetryPolicy`], honoring `retry-after`; other failures are not.
/// TLS is rustls with the `ring` provider and Mozilla's roots: no system
/// certificate store.
#[derive(Clone)]
pub struct TypeSafe {
    http: reqwest::Client,
    api_key: Arc<str>,
    url: String,
    model: String,
    retry: RetryPolicy,
}

impl fmt::Debug for TypeSafe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the key.
        f.debug_struct("TypeSafe")
            .field("url", &self.url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl TypeSafe {
    pub fn new(api_key: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls_config())
            .timeout(ATTEMPT_TIMEOUT)
            .build()
            .expect("a static client configuration builds");
        Self {
            http,
            api_key: api_key.into().into(),
            url: SYSTEM_ONE_URL.to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            retry: RetryPolicy::default(),
        }
    }

    /// A client that reads its key from `TYPESAFE_API_KEY`.
    pub fn from_env() -> Result<Self, MissingApiKey> {
        api_key(std::env::var(API_KEY_VAR).ok()).map(Self::new)
    }

    /// A client whose key is in the file at `path`; see
    /// [`api_key_from_file`].
    pub fn from_key_file(
        path: impl AsRef<Path>,
    ) -> Result<Self, ApiKeyFileError> {
        api_key_from_file(path).map(Self::new)
    }

    /// The model to ask; [`DEFAULT_MODEL`] by default.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// The endpoint; [`SYSTEM_ONE_URL`] by default. For a proxy, or a
    /// local server in tests.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// One attempt. An error with `Some` may be retried, no sooner than
    /// the server's `retry-after` when it sent one. Transport errors
    /// leave the URL out, since it could be private.
    async fn attempt(
        &self,
        body: &str,
    ) -> Result<Response, (JevError, Option<Option<Duration>>)> {
        let response = self
            .http
            .post(&self.url)
            .bearer_auth(&*self.api_key)
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .map_err(|error| {
                (JevError::Transport(error.without_url().to_string()), None)
            })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let retry_after = matches!(status, 429 | 503 | 529).then(|| {
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after)
            });
            return Err((JevError::Status(status), retry_after));
        }
        let text = response.text().await.map_err(|error| {
            (JevError::Transport(error.without_url().to_string()), None)
        })?;
        serde_json::from_str(&text)
            .map_err(|error| (JevError::Malformed(error.to_string()), None))
    }
}

#[async_trait]
impl Jev for TypeSafe {
    async fn ask(&self, request: &Request) -> Result<Response, JevError> {
        let body = json!({
            "model": self.model,
            "state": request.state,
            "questions": request.questions,
        })
        .to_string();
        let mut attempts = 1;
        loop {
            match self.attempt(&body).await {
                Ok(response) => return Ok(response),
                Err((_, Some(retry_after))) if self.retry.allows(attempts) => {
                    let delay = self.retry.delay_with_hint(
                        attempts,
                        jitter(),
                        retry_after,
                    );
                    attempts += 1;
                    tokio::time::sleep(delay).await;
                }
                Err((error, _)) => return Err(error),
            }
        }
    }
}
