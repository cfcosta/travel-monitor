//! Miles logins (DECISIONS.md 0024): the daemon's Chromium profile is
//! opened in a visible window so the person logs in by hand. A profile
//! can be open in one process only, so the app first asks the daemon to
//! pause its headless browser (`OpenLogin`), waits for it, opens the
//! window, and asks it to resume (`CloseLogin`) when the window closes.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, bail};
use gpui::Context;

use crate::{
    data::{Health, Request, SharedBackend, Ticket, TicketState},
    ui::TravelMonitor,
};

/// Where the daemon's profile is and which Chromium opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginConfig {
    pub profile_dir: PathBuf,
    pub chromium: PathBuf,
}

impl LoginConfig {
    /// `--profile-dir`, else `TM_PROFILE_DIR`, else the home-manager
    /// module's default (`$XDG_DATA_HOME/travel-monitor/chromium`);
    /// `--chromium`, else `TM_CHROMIUM`, else `chromium` on the `PATH`.
    pub fn resolve(
        profile_dir: Option<PathBuf>,
        chromium: Option<PathBuf>,
    ) -> Self {
        let profile_dir = profile_dir
            .or_else(|| std::env::var_os("TM_PROFILE_DIR").map(PathBuf::from))
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("travel-monitor")
                    .join("chromium")
            });
        let chromium = chromium
            .or_else(|| std::env::var_os("TM_CHROMIUM").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("chromium"));
        Self {
            profile_dir,
            chromium,
        }
    }

    /// The command line of the login window.
    pub fn args(&self, url: &str) -> Vec<String> {
        vec![
            format!("--user-data-dir={}", self.profile_dir.display()),
            "--no-first-run".to_owned(),
            "--no-default-browser-check".to_owned(),
            "--new-window".to_owned(),
            url.to_owned(),
        ]
    }
}

/// Each program's login page.
pub fn login_url(program: &str) -> &'static str {
    match program {
        "smiles" => "https://www.smiles.com.br/login",
        "latam_pass" => "https://www.latamairlines.com/br/pt/minha-conta",
        "azul_fidelidade" => {
            "https://www.voeazul.com.br/br/pt/programa-fidelidade"
        }
        _ => "about:blank",
    }
}

/// How long the daemon gets to pause its browser.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(90);

/// Polls `ticket` until the daemon finishes it.
pub async fn wait_for(
    backend: &SharedBackend,
    ticket: Ticket,
    timeout: Duration,
    poll: Duration,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    loop {
        match backend.ticket(ticket).await? {
            TicketState::Done => return Ok(()),
            TicketState::Failed => {
                bail!("the daemon could not pause its browser")
            }
            TicketState::Pending | TicketState::Running => {}
        }
        if start.elapsed() >= timeout {
            bail!(
                "the daemon did not pause its browser in {} s",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(poll).await;
    }
}

/// The whole login, on tokio: pause the daemon (when it runs), open
/// the window, wait for it to close, resume the daemon.
pub async fn run(
    backend: SharedBackend,
    config: LoginConfig,
    program: String,
    daemon_running: bool,
) -> anyhow::Result<()> {
    if daemon_running {
        let ticket =
            backend.enqueue(Request::OpenLogin(program.clone())).await?;
        if let Err(error) =
            wait_for(&backend, ticket, PAUSE_TIMEOUT, Duration::from_secs(1))
                .await
        {
            let _ = backend.enqueue(Request::CloseLogin).await;
            return Err(error);
        }
    }
    let window = async {
        std::fs::create_dir_all(&config.profile_dir).with_context(|| {
            format!("cannot create {}", config.profile_dir.display())
        })?;
        let status = tokio::process::Command::new(&config.chromium)
            .args(config.args(login_url(&program)))
            .status()
            .await
            .with_context(|| {
                format!("cannot start {}", config.chromium.display())
            })?;
        tracing::info!("login window for {program} closed ({status})");
        anyhow::Ok(())
    }
    .await;
    if daemon_running {
        backend.enqueue(Request::CloseLogin).await?;
    }
    window
}

impl TravelMonitor {
    pub fn start_login(&mut self, program: String, cx: &mut Context<Self>) {
        if self.logins.contains(&program) {
            return;
        }
        let Some(tokio) = self.env.tokio.clone() else {
            self.show_toast("Logins need the app's runtime", cx);
            return;
        };
        let running = self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.daemon.health(s.taken_at) == Health::Running);
        self.logins.insert(program.clone());
        let job = tokio.spawn(run(
            self.env.backend.clone(),
            self.env.login.clone(),
            program.clone(),
            running,
        ));
        self.show_toast(
            if running {
                "Pausing the daemon's browser…"
            } else {
                "Opening the login window…"
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = match job.await {
                Ok(result) => result,
                Err(error) => Err(error.into()),
            };
            let _ = this.update(cx, |this, cx| {
                this.logins.remove(&program);
                match result {
                    Ok(()) => this.show_toast(
                        "Login window closed; the daemon resumes",
                        cx,
                    ),
                    Err(error) => {
                        this.show_toast(format!("Login failed: {error:#}"), cx)
                    }
                }
                this.refresh(cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] is a synchronous entry point"
)]
mod tests {
    use super::*;

    #[test]
    fn flags_beat_the_environment() {
        let c = LoginConfig::resolve(
            Some("/p".into()),
            Some("/bin/chromium".into()),
        );
        assert_eq!(c.profile_dir, PathBuf::from("/p"));
        assert_eq!(c.chromium, PathBuf::from("/bin/chromium"));
        let args = c.args("https://example.com");
        assert_eq!(args[0], "--user-data-dir=/p");
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.com")
        );
    }

    #[test]
    fn every_program_has_a_page() {
        for p in ["smiles", "latam_pass", "azul_fidelidade"] {
            assert!(login_url(p).starts_with("https://"));
        }
    }

    #[tokio::test]
    async fn waits_for_the_daemon() {
        let backend: SharedBackend =
            std::sync::Arc::new(crate::data::fake::FakeBackend::new());
        let ticket = backend
            .enqueue(Request::OpenLogin("smiles".into()))
            .await
            .expect("ticket");
        wait_for(
            &backend,
            ticket,
            Duration::from_secs(5),
            Duration::from_millis(1),
        )
        .await
        .expect("done");
    }

    /// With a "Chromium" that exits at once, the daemon is paused and
    /// resumed around it.
    #[tokio::test]
    async fn resumes_the_daemon_after_the_window() {
        let fake = std::sync::Arc::new(crate::data::fake::FakeBackend::new());
        let backend: SharedBackend = fake.clone();
        let dir = std::env::temp_dir()
            .join(format!("tm-login-{}", std::process::id()));
        let config = LoginConfig {
            profile_dir: dir.clone(),
            chromium: PathBuf::from("true"),
        };
        // `wait_for` polls once a second; the fake is done on the 3rd poll.
        run(backend, config, "smiles".into(), true)
            .await
            .expect("login");
        assert_eq!(
            fake.requests(),
            vec![Request::OpenLogin("smiles".into()), Request::CloseLogin]
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
