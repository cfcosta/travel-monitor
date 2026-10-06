//! The browser connection, its contexts, and opening pages.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    cdp::Connection,
    error::{Error, Result},
    page::Page,
};

/// A connection to a browser's DevTools endpoint. Clones share the
/// connection.
#[derive(Clone)]
pub struct Browser {
    connection: Arc<Connection>,
}

/// What `Browser.getVersion` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub protocol_version: String,
    pub product: String,
    pub user_agent: String,
}

/// How a browser context is set up. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextOptions {
    /// A proxy for every request of the context, such as
    /// `http://127.0.0.1:8080` or `socks5://10.0.0.1:1080`. Proxies that
    /// need authentication are not supported.
    pub proxy: Option<String>,
    /// A locale for `Intl` in the context's pages, such as `pt-BR`. It
    /// does not change `Accept-Language`; Google's language comes from
    /// the URL's `hl`.
    pub locale: Option<String>,
    /// An IANA time zone for the context's pages, such as
    /// `America/Sao_Paulo`.
    pub timezone: Option<String>,
}

impl Browser {
    /// Connects to a browser's WebSocket endpoint
    /// (`ws://host:port/devtools/browser/<id>`).
    pub async fn connect(ws_url: &str) -> Result<Self> {
        Ok(Self {
            connection: Connection::connect(ws_url).await?,
        })
    }

    pub async fn version(&self) -> Result<Version> {
        let version = self.call("Browser.getVersion", json!({})).await?;
        Ok(serde_json::from_value(version)?)
    }

    /// Sends a browser-level command and returns its result. For what
    /// the typed methods do not cover.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.connection.call(None, method, params).await
    }

    /// Opens a page in the default context: the profile's own, whose
    /// cookies persist in the user data directory (logins, 0024).
    pub async fn new_page(&self) -> Result<Page> {
        Page::open(
            Arc::clone(&self.connection),
            None,
            &ContextOptions::default(),
        )
        .await
    }

    /// Creates an isolated context, like an incognito window: its own
    /// cookies and storage, discarded when it is closed. Each point of
    /// sale or proxy gets one (0033).
    pub async fn new_context(
        &self,
        options: ContextOptions,
    ) -> Result<Context> {
        let mut params = json!({"disposeOnDetach": true});
        if let Some(proxy) = &options.proxy {
            params["proxyServer"] = json!(proxy);
        }
        let result = self.call("Target.createBrowserContext", params).await?;
        let id = result["browserContextId"]
            .as_str()
            .ok_or_else(|| {
                Error::malformed(
                    "Target.createBrowserContext",
                    "no browserContextId",
                )
            })?
            .to_owned();
        Ok(Context {
            connection: Arc::clone(&self.connection),
            id,
            options,
            disposed: false,
        })
    }

    /// Asks the browser to close. The connection closes with it.
    pub async fn close(&self) -> Result<()> {
        match self.call("Browser.close", json!({})).await {
            Ok(_) | Err(Error::Closed) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Whether the connection to the browser is gone.
    pub fn is_closed(&self) -> bool {
        self.connection.is_closed()
    }
}

/// An isolated browser context. Dropping it disposes of it and closes
/// its pages; prefer [`Context::close`], which waits for that.
pub struct Context {
    connection: Arc<Connection>,
    id: String,
    options: ContextOptions,
    disposed: bool,
}

impl Context {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn options(&self) -> &ContextOptions {
        &self.options
    }

    /// Opens a page in this context, with the context's locale and time
    /// zone applied.
    pub async fn new_page(&self) -> Result<Page> {
        Page::open(Arc::clone(&self.connection), Some(&self.id), &self.options)
            .await
    }

    /// Disposes of the context, closing its pages and discarding its
    /// cookies and storage.
    pub async fn close(mut self) -> Result<()> {
        self.disposed = true;
        self.connection
            .call(
                None,
                "Target.disposeBrowserContext",
                json!({"browserContextId": self.id}),
            )
            .await
            .map(drop)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        if !self.disposed {
            self.connection.send(
                None,
                "Target.disposeBrowserContext",
                json!({"browserContextId": self.id}),
            );
        }
    }
}
