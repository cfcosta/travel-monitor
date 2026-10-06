//! Test support: our own headless Chromium and a tiny local HTTP server.
//! Nothing here touches the network beyond 127.0.0.1.

#![allow(dead_code)]

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use tempfile::TempDir;
use tm_browser::{Chromium, ChromiumConfig};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// Starts Chromium from `TM_CHROMIUM` with a fresh profile, or returns
/// `None` (and says so) when the variable is unset.
pub async fn chromium() -> Option<(Chromium, TempDir)> {
    let Some(binary) =
        std::env::var_os("TM_CHROMIUM").filter(|v| !v.is_empty())
    else {
        eprintln!(
            "skipping: TM_CHROMIUM is not set (run inside `nix develop`)"
        );
        return None;
    };
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut config = ChromiumConfig::new(binary, dir.path().join("profile"));
    config.window = (1120, 780);
    let chromium = Chromium::launch(config).await.expect("Chromium starts");
    Some((chromium, dir))
}

/// A request the server saw.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// The request target as sent: a path, or an absolute URL when the
    /// server is used as a proxy.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn path(&self) -> &str {
        let target = self
            .target
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or(&self.target);
        target.split('?').next().unwrap_or(target)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn html(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }

    pub fn text(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "text/plain; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }

    pub fn bytes(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: "application/octet-stream",
            body,
        }
    }

    pub fn not_found() -> Self {
        Self {
            status: 404,
            content_type: "text/plain",
            body: b"not found".to_vec(),
        }
    }
}

type Handler = dyn Fn(&Seen) -> Reply + Send + Sync + 'static;

pub struct Server {
    pub address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    /// Serves every request with `handler`, one request per connection.
    pub async fn start(
        handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let handler = Arc::clone(&handler);
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let _ = respond(stream, &*handler, &log).await;
                });
            }
        });
        Self { address, seen }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("not poisoned").clone()
    }
}

async fn respond(
    mut stream: TcpStream,
    handler: &Handler,
    log: &Mutex<Vec<Seen>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(i) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_owned();
    let target = request_line.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let seen = Seen {
        method,
        target,
        headers,
        body,
    };
    let reply = handler(&seen);
    log.lock().expect("not poisoned").push(seen);
    let head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&reply.body).await?;
    stream.shutdown().await
}
