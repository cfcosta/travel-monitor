//! Miles logins (DECISIONS.md 0024, 0062): the program's own Chromium
//! profile, the one the daemon uses for that site
//! (`<profile-dir>/miles-<program>`,
//! [`tm_browser::profile::site_profile_dir`]), is opened in a visible
//! window so the person logs in by hand. A profile can be open in one
//! process only, so the app first asks the daemon to pause that site
//! (`OpenLogin`), waits for it, opens the window, and asks it to resume
//! (`CloseLogin`) when the window closes. Every other site keeps
//! running. Logins made in the shared profile before 0062 are not
//! carried over.
//!
//! The window passes the daemon's profile flags
//! ([`tm_browser::profile_args`]), so the cookies it writes are the ones
//! the daemon's Chromium can read. Once it closes, the profile's cookie
//! jar says whether the login took ([`tm_browser::profile`] has the
//! heuristic and its limits); the daemon checks the same jar when it
//! resumes and records what it finds.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, bail};
use chrono::Utc;
use gpui::Context;

use crate::{
    data::{Health, Request, SharedBackend, Ticket, TicketState},
    ui::TravelMonitor,
};

/// Where the daemon's profiles are and which Chromium opens them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginConfig {
    /// The daemon's `--profile-dir`: every site's profile is under it.
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

    /// The profile of `program`'s site: the directory the daemon's
    /// Chromium for that site uses (0062).
    pub fn program_dir(&self, program: &str) -> PathBuf {
        let site = tm_domain::Source::Miles {
            program: program.to_owned(),
        }
        .site();
        tm_browser::profile::site_profile_dir(&self.profile_dir, &site)
    }

    /// The command line of `program`'s login window: its site's profile
    /// with the daemon's profile flags, visible and without DevTools.
    pub fn args(&self, program: &str, url: &str) -> Vec<String> {
        let mut args = vec![format!(
            "--user-data-dir={}",
            self.program_dir(program).display()
        )];
        args.extend(tm_browser::profile_args().iter().map(|&a| a.to_owned()));
        args.extend([
            "--no-first-run".to_owned(),
            "--no-default-browser-check".to_owned(),
            "--new-window".to_owned(),
            url.to_owned(),
        ]);
        args
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

/// What the profile holds for a program once the login window closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// A cookie that counts as a login is there.
    Found,
    /// None is: the login did not take, or its cookies end with the
    /// browser.
    Missing,
    /// The cookie jar could not be read.
    Unknown(String),
}

impl Session {
    /// The toast after a login attempt for `name` (`"LATAM Pass"`).
    pub fn message(&self, name: &str) -> String {
        match self {
            Session::Found => format!("{name}: logged in"),
            Session::Missing => {
                format!("{name}: no session found \u{2014} try again")
            }
            Session::Unknown(error) => {
                format!("{name}: could not check the session ({error})")
            }
        }
    }
}

/// Reads the profile's cookie jar for `program`'s session. Only while no
/// Chromium runs on the profile.
pub async fn check_session(profile_dir: &Path, program: &str) -> Session {
    let domains = tm_domain::miles_session_domains(program);
    match tm_browser::profile::login_cookies(profile_dir, domains, Utc::now())
        .await
    {
        Ok(cookies) if cookies.is_empty() => Session::Missing,
        Ok(cookies) => {
            let names: Vec<_> = cookies
                .iter()
                .map(|c| format!("{}:{}", c.host, c.name))
                .collect();
            tracing::info!(%program, ?names, "login cookies found");
            Session::Found
        }
        Err(error) => Session::Unknown(error.to_string()),
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

/// The whole login, on tokio: pause the program's site (when the daemon
/// runs), open the window on that site's profile, wait for it to close,
/// check the profile for a session, resume the site.
pub async fn run(
    backend: SharedBackend,
    config: LoginConfig,
    program: String,
    daemon_running: bool,
) -> anyhow::Result<Session> {
    if daemon_running {
        let ticket =
            backend.enqueue(Request::OpenLogin(program.clone())).await?;
        if let Err(error) =
            wait_for(&backend, ticket, PAUSE_TIMEOUT, Duration::from_secs(1))
                .await
        {
            let _ = backend.enqueue(Request::CloseLogin(program.clone())).await;
            return Err(error);
        }
    }
    let window = async {
        let profile = config.program_dir(&program);
        std::fs::create_dir_all(&profile)
            .with_context(|| format!("cannot create {}", profile.display()))?;
        let status = tokio::process::Command::new(&config.chromium)
            .args(config.args(&program, login_url(&program)))
            .status()
            .await
            .with_context(|| {
                format!("cannot start {}", config.chromium.display())
            })?;
        tracing::info!("login window for {program} closed ({status})");
        anyhow::Ok(check_session(&profile, &program).await)
    }
    .await;
    if daemon_running {
        backend
            .enqueue(Request::CloseLogin(program.clone()))
            .await?;
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
        let name = self
            .snapshot
            .as_ref()
            .and_then(|s| s.miles.iter().find(|m| m.program == program))
            .map_or_else(|| program.clone(), |m| m.name.clone());
        self.logins.insert(program.clone());
        let job = tokio.spawn(run(
            self.env.backend.clone(),
            self.env.login.clone(),
            program.clone(),
            running,
        ));
        self.show_toast(
            if running {
                "Pausing the daemon's browser for this program…"
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
                    Ok(session) => this.show_toast(session.message(&name), cx),
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
        let args = c.args("smiles", "https://example.com");
        // The program's own profile, as the daemon opens it (0062).
        assert_eq!(args[0], "--user-data-dir=/p/miles-smiles");
        assert_eq!(
            c.program_dir("latam_pass"),
            tm_browser::profile::site_profile_dir(
                Path::new("/p"),
                "miles:latam_pass"
            )
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.com")
        );
    }

    /// The window opens the profile the way the daemon's Chromium does,
    /// so the daemon can read the cookies it writes; but it stays a
    /// visible window without DevTools.
    #[test]
    fn the_window_passes_every_profile_flag() {
        let c = LoginConfig::resolve(Some("/p".into()), Some("c".into()));
        let args = c.args("smiles", "https://example.com");
        for flag in tm_browser::profile_args() {
            assert!(args.iter().any(|a| a == flag), "{flag} missing");
        }
        assert!(!args.iter().any(|a| a.starts_with("--headless")
            || a.starts_with("--remote-debugging")));
    }

    #[test]
    fn every_program_has_session_domains() {
        for p in ["smiles", "latam_pass", "azul_fidelidade"] {
            assert!(!tm_domain::miles_session_domains(p).is_empty());
        }
        assert_eq!(
            Session::Missing.message("LATAM Pass"),
            "LATAM Pass: no session found \u{2014} try again"
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
        let session = run(backend, config, "smiles".into(), true)
            .await
            .expect("login");
        assert_eq!(session, Session::Missing, "an empty profile");
        assert_eq!(
            fake.requests(),
            vec![
                Request::OpenLogin("smiles".into()),
                Request::CloseLogin("smiles".into())
            ]
        );
        assert!(dir.join("miles-smiles").is_dir(), "the program's profile");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A local site: `/login` sets a cookie that lasts a day, any other
    /// path sets nothing.
    async fn serve(listener: tokio::net::TcpListener) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let cookie = if request.starts_with("GET /login ") {
                "Set-Cookie: tm_login=yes; Max-Age=86400; Path=/\r\n"
            } else {
                ""
            };
            let body = "<!doctype html><title>ok</title>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{cookie}\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    }

    /// The bug of docs/research/sites/latam-pass.md: a cookie set in the
    /// login window must be there for the daemon's Chromium on the same
    /// profile. The window runs headless with DevTools here, only so the
    /// test can wait for the cookie and close it cleanly. Skips without
    /// `TM_CHROMIUM`.
    #[tokio::test]
    async fn the_windows_cookies_reach_the_daemons_browser() {
        use tm_browser::{CHROMIUM_VAR, Chromium, ChromiumConfig};

        let Some(chromium) =
            std::env::var_os(CHROMIUM_VAR).filter(|v| !v.is_empty())
        else {
            eprintln!("skipped: {CHROMIUM_VAR} is not set");
            return;
        };
        let profile = tempfile::tempdir().expect("profile");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(serve(listener));
        let config = LoginConfig {
            profile_dir: profile.path().to_owned(),
            chromium: chromium.into(),
        };

        let mut args = vec![
            "--headless=new".to_owned(),
            "--remote-debugging-port=0".to_owned(),
        ];
        args.extend(
            config.args("smiles", &format!("http://127.0.0.1:{port}/login")),
        );
        let site_profile = config.program_dir("smiles");
        let window = Chromium::launch_with(&config.chromium, args)
            .await
            .expect("login window");
        let start = std::time::Instant::now();
        loop {
            let cookies = window
                .browser()
                .call("Storage.getCookies", serde_json::json!({}))
                .await
                .expect("cookies");
            if cookies.to_string().contains("tm_login") {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(30), "no cookie");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        window.shutdown().await.expect("close the window");

        assert_eq!(
            check_session(&site_profile, "smiles").await,
            Session::Missing,
            "no cookie on smiles.com.br"
        );
        let jar = tm_browser::profile::login_cookies(
            &site_profile,
            &["127.0.0.1"],
            Utc::now(),
        )
        .await
        .expect("jar");
        assert!(jar.iter().any(|c| c.name == "tm_login"), "{jar:?}");

        // The daemon's Chromium for Smiles' site (0062).
        let daemon = Chromium::launch(ChromiumConfig::new(
            &config.chromium,
            tm_browser::profile::site_profile_dir(
                profile.path(),
                "miles:smiles",
            ),
        ))
        .await
        .expect("daemon's Chromium");
        let page = daemon.browser().new_page().await.expect("page");
        page.navigate(
            &format!("http://127.0.0.1:{port}/check"),
            Duration::from_secs(30),
        )
        .await
        .expect("navigate");
        let cookie: String =
            page.eval_as("document.cookie").await.expect("cookie");
        daemon
            .shutdown()
            .await
            .expect("close the daemon's Chromium");
        assert!(cookie.contains("tm_login=yes"), "{cookie:?}");
    }
}
