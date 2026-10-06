//! Starting and stopping our own Chromium (0021).

use std::{
    collections::VecDeque,
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::oneshot,
};

use crate::{
    browser::Browser,
    error::{Error, Result},
};

/// The environment variable holding the Chromium binary. The dev shell
/// and the packaged daemon set it.
pub const CHROMIUM_VAR: &str = "TM_CHROMIUM";

/// How long Chromium may take to offer its DevTools endpoint.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a graceful shutdown may take before the process is killed.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How to start Chromium.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromiumConfig {
    pub binary: PathBuf,
    /// The profile directory. Only one Chromium can use it at a time.
    pub user_data_dir: PathBuf,
    /// `--headless=new` when true; a visible window when false (the
    /// miles-program logins of 0024).
    pub headless: bool,
    /// The DevTools port. 0 lets the system pick a free one.
    pub port: u16,
    /// Window size in pixels, width then height. Google's Explore page
    /// returns more destinations for a larger viewport.
    pub window: (u32, u32),
    /// More command-line flags, passed after ours.
    pub extra_args: Vec<String>,
}

impl ChromiumConfig {
    /// Headless, on a free port, in a 1440×1000 window.
    pub fn new(
        binary: impl Into<PathBuf>,
        user_data_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            binary: binary.into(),
            user_data_dir: user_data_dir.into(),
            headless: true,
            port: 0,
            window: (1440, 1000),
            extra_args: Vec::new(),
        }
    }

    /// [`ChromiumConfig::new`] with the binary from `TM_CHROMIUM`.
    pub fn from_env(user_data_dir: impl Into<PathBuf>) -> Result<Self> {
        let binary = std::env::var_os(CHROMIUM_VAR)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::Launch(format!("{CHROMIUM_VAR} is not set"))
            })?;
        Ok(Self::new(binary, user_data_dir))
    }

    /// The command line, without the binary.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec![
            format!("--remote-debugging-port={}", self.port),
            format!("--user-data-dir={}", self.user_data_dir.display()),
            format!("--window-size={},{}", self.window.0, self.window.1),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-sync".into(),
            "--disable-features=Translate,MediaRouter".into(),
            "--password-store=basic".into(),
            "--use-mock-keychain".into(),
        ];
        if self.headless {
            args.push("--headless=new".into());
        }
        args.extend(self.extra_args.iter().cloned());
        args.push("about:blank".into());
        args
    }
}

/// A running Chromium, connected. Dropping it kills the process; prefer
/// [`Chromium::shutdown`], which lets it close its profile cleanly.
pub struct Chromium {
    child: Child,
    browser: Browser,
    ws_url: String,
}

impl Chromium {
    /// Starts Chromium, waits for its DevTools endpoint and connects.
    pub async fn launch(config: ChromiumConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.user_data_dir)?;
        let mut child = Command::new(&config.binary)
            .args(config.args())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::Launch(format!("{}: {e}", config.binary.display()))
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Launch("no stderr pipe".into()))?;
        let (found, endpoint) = oneshot::channel();
        tokio::spawn(read_stderr(stderr, found));
        let ws_url = match tokio::time::timeout(STARTUP_TIMEOUT, endpoint).await
        {
            Ok(Ok(Ok(url))) => url,
            Ok(Ok(Err(tail))) => {
                let status = child.wait().await.ok();
                return Err(Error::Launch(format!(
                    "exited ({}) before offering DevTools: {}",
                    status.map_or_else(|| "unknown".into(), |s| s.to_string()),
                    tail
                )));
            }
            Ok(Err(_)) => {
                let _ = child.start_kill();
                return Err(Error::Launch("stderr reader stopped".into()));
            }
            Err(_) => {
                let _ = child.start_kill();
                return Err(Error::timeout(
                    "Chromium's DevTools endpoint",
                    STARTUP_TIMEOUT,
                ));
            }
        };
        let browser = match Browser::connect(&ws_url).await {
            Ok(browser) => browser,
            Err(error) => {
                let _ = child.start_kill();
                return Err(error);
            }
        };
        Ok(Self {
            child,
            browser,
            ws_url,
        })
    }

    pub fn browser(&self) -> &Browser {
        &self.browser
    }

    /// The browser's DevTools WebSocket URL.
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Closes the browser and waits for the process to exit, killing it
    /// if it does not exit in time.
    pub async fn shutdown(mut self) -> Result<()> {
        let _ =
            tokio::time::timeout(SHUTDOWN_TIMEOUT, self.browser.close()).await;
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(status) => status.map(drop).map_err(Error::from),
            Err(_) => {
                self.child.kill().await?;
                Ok(())
            }
        }
    }
}

impl Drop for Chromium {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Reads stderr until the DevTools line, then keeps draining it so the
/// pipe never fills. Sends the endpoint, or the last lines if stderr
/// ends first.
async fn read_stderr(
    stderr: tokio::process::ChildStderr,
    found: oneshot::Sender<Result<String, String>>,
) {
    let mut lines = BufReader::new(stderr).lines();
    let mut tail = VecDeque::new();
    let mut found = Some(found);
    while let Ok(Some(line)) = lines.next_line().await {
        if found.is_some()
            && let Some(url) = devtools_url(&line)
        {
            if let Some(found) = found.take() {
                let _ = found.send(Ok(url.to_owned()));
            }
            continue;
        }
        tracing::trace!(target: "chromium", "{line}");
        if found.is_some() {
            tail.push_back(line);
            if tail.len() > 20 {
                tail.pop_front();
            }
        }
    }
    if let Some(found) = found {
        let _ = found.send(Err(Vec::from(tail).join("\n")));
    }
}

/// The WebSocket URL in Chromium's `DevTools listening on ws://…` line.
fn devtools_url(line: &str) -> Option<&str> {
    let url = line.strip_prefix("DevTools listening on ")?.trim();
    url.starts_with("ws://").then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoint_is_read_from_the_devtools_line() {
        assert_eq!(
            devtools_url(
                "DevTools listening on ws://127.0.0.1:41235/devtools/browser/ab-cd\n"
            ),
            Some("ws://127.0.0.1:41235/devtools/browser/ab-cd")
        );
        assert_eq!(devtools_url("[0101/000000.0:ERROR] DevTools x"), None);
    }

    #[test]
    fn headless_uses_the_new_mode_and_extra_args_come_last() {
        let mut config = ChromiumConfig::new("/bin/chromium", "/tmp/profile");
        config.extra_args.push("--lang=pt-BR".into());
        let args = config.args();
        assert!(args.contains(&"--headless=new".to_owned()));
        assert!(args.contains(&"--remote-debugging-port=0".to_owned()));
        assert!(args.contains(&"--user-data-dir=/tmp/profile".to_owned()));
        assert_eq!(args[args.len() - 2], "--lang=pt-BR");
        config.headless = false;
        assert!(!config.args().iter().any(|a| a.starts_with("--headless")));
    }
}
