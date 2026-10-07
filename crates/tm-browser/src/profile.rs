//! What a Chromium profile keeps on disk, read while no Chromium runs on
//! it: its cookie jar, to tell whether a miles login took (0024).
//!
//! # When a profile counts as logged in
//!
//! Cookie values are encrypted, but each cookie's host, name and expiry
//! are plain columns of the profile's `Cookies` SQLite file. A profile
//! counts as logged in to a site when it holds at least one cookie that:
//!
//! - belongs to one of the site's domains or a subdomain of one,
//! - is persistent and not expired (`expires_utc` in the future). A
//!   session cookie ends with the browser that set it, so it cannot
//!   carry a login from the login window to the daemon's next launch
//!   anyway, and
//! - is not a well-known analytics, consent, load-balancer or
//!   bot-manager cookie ([`is_tracker`]), which any visit sets, logged
//!   in or not.
//!
//! Sites whose session cookie a probe has named ([`KNOWN_SESSIONS`])
//! are held to that cookie instead: LATAM's own cookies, set to any
//! visitor, are persistent and not trackers, so the general rule would
//! read every LATAM visit as a login (docs/research/sites/latam-pass.md,
//! "Measuring cash against miles", §6). For those sites only the named
//! cookie on its named host counts, still persistent and unexpired.
//!
//! The general rule is a heuristic, with these limits:
//!
//! - Any other persistent cookie counts: a site that keeps a
//!   preference, or a tracker missing from the list, for a visitor who
//!   never logged in reads as logged in.
//! - A cookie whose session the server already ended still counts until
//!   its expiry date.
//! - A site that holds its login only in session cookies, or in local
//!   storage, reads as logged out.
//! - Values are not read, so a cookie the daemon's Chromium cannot
//!   decrypt (one written without [`crate::profile_args`]) still counts;
//!   that Chromium drops it at its next launch.
//!
//! Only a page load proves a session; the daemon finds out on its first
//! check of the program, and a logged-out check marks it expired.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use sqlx::{
    Connection as _,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

use crate::error::{Error, Result};

/// One cookie in a profile's jar, without its (encrypted) value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCookie {
    /// `host_key`: `.example.com` for a domain cookie, `www.example.com`
    /// for a host-only one.
    pub host: String,
    pub name: String,
    /// `None` for a session cookie.
    pub expires: Option<DateTime<Utc>>,
}

impl StoredCookie {
    /// Whether the cookie belongs to `domain` or one of its subdomains.
    pub fn is_on(&self, domain: &str) -> bool {
        let host = self.host.trim_start_matches('.').to_ascii_lowercase();
        let domain = domain.to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    }

    /// Whether it counts toward a login at `now` under the general rule
    /// (see the module docs).
    pub fn counts_as_login(&self, now: DateTime<Utc>) -> bool {
        self.is_live(now) && !is_tracker(&self.name)
    }

    /// Persistent and not expired at `now`.
    fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.expires.is_some_and(|e| e > now)
    }

    /// Whether it is `session`'s cookie: the same name, on exactly the
    /// session's host (a host-only cookie or a domain cookie of that
    /// host).
    pub fn is_session(&self, session: &SessionCookie) -> bool {
        self.name == session.name
            && self
                .host
                .trim_start_matches('.')
                .eq_ignore_ascii_case(session.host)
    }
}

/// The cookie that holds a site's login, found by a probe of the site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCookie {
    /// The registrable domain the rule applies to, one of a program's
    /// session domains (`tm_domain::miles_session_domains`).
    pub domain: &'static str,
    /// The host that sets it, without a leading dot.
    pub host: &'static str,
    pub name: &'static str,
}

/// Sites whose login cookie is known. For a site listed here, only that
/// cookie means logged in (see the module docs).
///
/// - LATAM Pass: `_xp_session` on `www.latamairlines.com`, httpOnly,
///   five days from the last page load. Auth0's own `auth0` cookie on
///   `auth.latamairlines.com` lasts three days from the login and is
///   not renewed by the site's pages, so it is not required; the
///   site's pages read the login from `_xp_session`.
pub const KNOWN_SESSIONS: &[SessionCookie] = &[SessionCookie {
    domain: "latamairlines.com",
    host: "www.latamairlines.com",
    name: "_xp_session",
}];

/// The known session cookies of `domains` ([`KNOWN_SESSIONS`]); empty
/// when none of them has one, and the general rule applies.
pub fn known_sessions(domains: &[&str]) -> Vec<SessionCookie> {
    KNOWN_SESSIONS
        .iter()
        .filter(|s| domains.iter().any(|d| d.eq_ignore_ascii_case(s.domain)))
        .copied()
        .collect()
}

/// The cookies among `cookies` that make a profile count as logged in to
/// `domains` at `now`: the live known session cookies when the domains
/// have one ([`known_sessions`]), else every cookie that passes the
/// general rule ([`StoredCookie::counts_as_login`]).
pub fn session_cookies(
    cookies: Vec<StoredCookie>,
    domains: &[&str],
    now: DateTime<Utc>,
) -> Vec<StoredCookie> {
    let known = known_sessions(domains);
    cookies
        .into_iter()
        .filter(|c| {
            if known.is_empty() {
                c.counts_as_login(now)
            } else {
                c.is_live(now) && known.iter().any(|s| c.is_session(s))
            }
        })
        .collect()
}

/// Cookie names (or name prefixes) that any visit sets, logged in or
/// not: analytics, consent banners, load balancers and bot managers.
const TRACKERS: &[&str] = &[
    // Akamai Bot Manager and edge.
    "_abck",
    "bm_",
    "ak_bmsc",
    "akavpau_",
    "aka_",
    "akacd_",
    "akamai_",
    // Cloudflare, PerimeterX, Imperva, AWS load balancers.
    "__cf",
    "cf_",
    "_cfuvid",
    "_px",
    "pxcts",
    "incap_ses",
    "visid_incap",
    "nlbi_",
    "AWSALB",
    // Google, Meta, Microsoft, TikTok, Pinterest, Reddit, Snap,
    // LinkedIn, Criteo.
    "_ga",
    "_gid",
    "_gat",
    "_gcl",
    "__utm",
    "_dc_gtm",
    "_fbp",
    "_fbc",
    "_uet",
    "_clck",
    "_clsk",
    "_tt",
    "ttcsid",
    "_pin",
    "_rdt",
    "_scid",
    "li_",
    "cto_",
    // Adobe, Dynatrace, Hotjar, VWO, Matomo, Tealium, Quantum Metric,
    // Contentsquare, Datadog, RD Station.
    "AMCV_",
    "AMCVS_",
    "s_",
    "mbox",
    "kndctr_",
    "dtCookie",
    "dtPC",
    "dtSa",
    "dtLatC",
    "rxVisitor",
    "rxvt",
    "_hj",
    "_vwo",
    "_vis_opt",
    "_pk_",
    "utag_",
    "QuantumMetric",
    "_cs_",
    "_dd_s",
    "rdtrk",
    "__trf",
    // Google Ads, FullStory, Medallia (Kampyle), Bloomreach (Exponea),
    // ThreatMetrix: all set on LATAM's pages before any login.
    "__gads",
    "__gpi",
    "__eoi",
    "fs_uid",
    "fs_lua",
    "fs_cid",
    "kampyle",
    "__exponea",
    "thx_guid",
    "tmx_guid",
    // Consent banners (OneTrust, IAB TCF).
    "OptanonConsent",
    "OptanonAlertBoxClosed",
    "eupubconsent",
];

/// Whether `name` is a cookie that any visit sets (see [`TRACKERS`]).
pub fn is_tracker(name: &str) -> bool {
    TRACKERS.iter().any(|prefix| name.starts_with(prefix))
}

/// Microseconds from 1601-01-01 (Chromium's epoch) to 1970-01-01.
const WINDOWS_TO_UNIX_MICROS: i64 = 11_644_473_600_000_000;

/// A Chromium timestamp (`expires_utc`); 0 means none.
fn chromium_time(micros: i64) -> Option<DateTime<Utc>> {
    if micros <= 0 {
        return None;
    }
    DateTime::from_timestamp_micros(micros - WINDOWS_TO_UNIX_MICROS)
}

/// The default profile's cookie files: `Network/Cookies` where Chromium
/// moved it (96 on, on most platforms), `Cookies` otherwise (the Linux
/// Chromium 151 the daemon runs still writes `Default/Cookies`).
fn cookie_files(user_data_dir: &Path) -> Vec<PathBuf> {
    let default = user_data_dir.join("Default");
    [
        default.join("Network").join("Cookies"),
        default.join("Cookies"),
    ]
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

/// The cookies of `domains` (and their subdomains) in the profile at
/// `user_data_dir`. Reads the `Cookies` files read-only; call it only
/// while no Chromium runs on the profile, which holds the file locked.
/// A profile without a cookie file has no cookies.
pub async fn stored_cookies(
    user_data_dir: &Path,
    domains: &[&str],
) -> Result<Vec<StoredCookie>> {
    let mut found = Vec::new();
    for file in cookie_files(user_data_dir) {
        let rows = read_cookies(&file).await.map_err(|e| {
            Error::Profile(format!("cannot read {}: {e}", file.display()))
        })?;
        found.extend(
            rows.into_iter()
                .filter(|c| domains.iter().any(|d| c.is_on(d))),
        );
    }
    Ok(found)
}

async fn read_cookies(file: &Path) -> sqlx::Result<Vec<StoredCookie>> {
    let options = SqliteConnectOptions::new()
        .filename(file)
        .read_only(true)
        .busy_timeout(Duration::from_secs(1));
    let mut connection = SqliteConnection::connect_with(&options).await?;
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT host_key, name, expires_utc FROM cookies")
            .fetch_all(&mut connection)
            .await?;
    connection.close().await?;
    Ok(rows
        .into_iter()
        .map(|(host, name, expires)| StoredCookie {
            host,
            name,
            expires: chromium_time(expires),
        })
        .collect())
}

/// The cookies that make the profile count as logged in to `domains`
/// at `now` (see the module docs and [`session_cookies`]). Empty when
/// it does not.
pub async fn login_cookies(
    user_data_dir: &Path,
    domains: &[&str],
    now: DateTime<Utc>,
) -> Result<Vec<StoredCookie>> {
    Ok(session_cookies(
        stored_cookies(user_data_dir, domains).await?,
        domains,
        now,
    ))
}

/// The Chromium profile directory of `site` (a [`tm_domain::Source::site`])
/// under the daemon's profile directory `base`: `base/<site slug>`
/// (`chromium/google_flights`, `chromium/airline-tp`,
/// `chromium/miles-smiles`). Each site has its own profile and browser
/// process, so a login window on one site's profile pauses only that
/// site (DECISIONS.md 0062). The daemon and the app's login window both
/// use this path.
pub fn site_profile_dir(base: &Path, site: &str) -> PathBuf {
    base.join(tm_domain::site_slug(site))
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn cookie(host: &str, name: &str, expires: Option<i64>) -> StoredCookie {
        StoredCookie {
            host: host.into(),
            name: name.into(),
            expires: expires.and_then(DateTime::from_timestamp_micros),
        }
    }

    #[test]
    fn each_site_has_its_own_directory() {
        let base = Path::new("/data/chromium");
        assert_eq!(
            site_profile_dir(base, "google_flights"),
            PathBuf::from("/data/chromium/google_flights")
        );
        assert_eq!(
            site_profile_dir(base, "miles:smiles"),
            PathBuf::from("/data/chromium/miles-smiles")
        );
        assert_eq!(
            site_profile_dir(base, "airline:TP"),
            PathBuf::from("/data/chromium/airline-tp")
        );
    }

    #[test]
    fn domains_match_their_subdomains_only() {
        let c = cookie(".www.smiles.com.br", "x", None);
        assert!(c.is_on("smiles.com.br"));
        assert!(c.is_on("www.smiles.com.br"));
        assert!(!c.is_on("com.br.example"));
        assert!(cookie("smiles.com.br", "x", None).is_on("smiles.com.br"));
        assert!(!cookie("notsmiles.com.br", "x", None).is_on("smiles.com.br"));
        assert!(!cookie("latam.com.evil.io", "x", None).is_on("latam.com"));
    }

    #[test]
    fn only_unexpired_persistent_non_tracker_cookies_count() {
        let now = Utc::now();
        let later = Some((now + TimeDelta::days(30)).timestamp_micros());
        let earlier = Some((now - TimeDelta::days(1)).timestamp_micros());
        assert!(cookie("a.com", "sessionToken", later).counts_as_login(now));
        assert!(!cookie("a.com", "sessionToken", None).counts_as_login(now));
        assert!(!cookie("a.com", "sessionToken", earlier).counts_as_login(now));
        for tracker in ["_abck", "bm_sz", "_ga_XYZ", "OptanonConsent", "AWSALB"]
        {
            assert!(!cookie("a.com", tracker, later).counts_as_login(now));
        }
    }

    /// The LATAM cookies a visit leaves before any login, as the
    /// `miles-latam_pass` profile held them on 2026-10-07
    /// (docs/research/sites/fixtures/latam-pass/session-cookies.json):
    /// host, name, days to expiry (`None`: a session cookie).
    const LATAM_VISIT: &[(&str, &str, Option<i64>)] = &[
        (".latamairlines.com", "_abck", Some(365)),
        (".latamairlines.com", "bm_sz", Some(1)),
        (".latamairlines.com", "bm_s", Some(31)),
        (".latamairlines.com", "ak_bmsc", Some(1)),
        (".latamairlines.com", "AKA_A2", Some(1)),
        (".latamairlines.com", "_ga", Some(400)),
        (".latamairlines.com", "_ga_0FQ5F9TMWN", Some(400)),
        (".latamairlines.com", "_gcl_au", Some(90)),
        (".latamairlines.com", "__gads", Some(390)),
        (".latamairlines.com", "__gpi", Some(390)),
        (".latamairlines.com", "__eoi", Some(180)),
        (".latamairlines.com", "__exponea_etc__", Some(400)),
        (".latamairlines.com", "fs_uid", Some(365)),
        (".latamairlines.com", "_xp_exp_id", Some(400)),
        (".latamairlines.com", "_xp_application_country", None),
        (".latamairlines.com", "PIM-SESSION-ID", None),
        (".www.latamairlines.com", "RT", Some(7)),
        (".www.latamairlines.com", "bm_lso", Some(30)),
        ("www.latamairlines.com", "_xp_show_login_incentive", Some(1)),
        ("www.latamairlines.com", "_xp_metadata_br_pt", Some(7)),
        ("www.latamairlines.com", "kampyle_userid", Some(365)),
        ("www.latamairlines.com", "akavpau_CyberLatam", None),
        ("auth.latamairlines.com", "did", Some(365)),
        ("auth.latamairlines.com", "did_compat", Some(365)),
        ("auth.latamairlines.com", "fpid-af", None),
        ("loyaltyprogram.latam.com", "thx_guid", Some(400)),
        ("loyaltyprogram.latam.com", "tmx_guid", Some(400)),
    ];

    /// A LATAM jar: [`LATAM_VISIT`] plus `extra`, at `now`.
    fn latam_jar(
        now: DateTime<Utc>,
        extra: &[(&str, &str, Option<i64>)],
    ) -> Vec<StoredCookie> {
        LATAM_VISIT
            .iter()
            .chain(extra)
            .map(|&(host, name, days)| {
                cookie(
                    host,
                    name,
                    days.map(|d| (now + TimeDelta::days(d)).timestamp_micros()),
                )
            })
            .collect()
    }

    const LATAM: [&str; 2] = ["latamairlines.com", "latam.com"];

    #[test]
    fn the_general_rule_reads_a_latam_visit_as_a_login() {
        // Why LATAM needs its named cookie: these pass the general rule.
        let now = Utc::now();
        let general: Vec<_> = latam_jar(now, &[])
            .into_iter()
            .filter(|c| c.counts_as_login(now))
            .map(|c| c.name)
            .collect();
        assert!(general.contains(&"_xp_show_login_incentive".to_owned()));
        assert!(general.contains(&"did".to_owned()));
        // The trackers among them are not.
        for tracker in ["__gads", "fs_uid", "kampyle_userid", "thx_guid"] {
            assert!(!general.contains(&tracker.to_owned()), "{tracker}");
        }
    }

    #[test]
    fn latam_counts_only_its_session_cookie() {
        let now = Utc::now();
        assert_eq!(
            known_sessions(&LATAM),
            vec![SessionCookie {
                domain: "latamairlines.com",
                host: "www.latamairlines.com",
                name: "_xp_session",
            }]
        );
        assert!(known_sessions(&["smiles.com.br"]).is_empty());

        // A visit, logged out.
        assert!(session_cookies(latam_jar(now, &[]), &LATAM, now).is_empty());
        // Auth0's login-domain session alone: the site's pages do not
        // read it.
        let auth0 = [("auth.latamairlines.com", "auth0", Some(3))];
        assert!(
            session_cookies(latam_jar(now, &auth0), &LATAM, now).is_empty()
        );
        // Logged in: `_xp_session` on www, five days.
        let logged_in = [
            ("auth.latamairlines.com", "auth0", Some(3)),
            ("www.latamairlines.com", "_xp_session", Some(5)),
        ];
        let found = session_cookies(latam_jar(now, &logged_in), &LATAM, now);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "_xp_session");
        assert_eq!(found[0].host, "www.latamairlines.com");
        // Expired, or ending with the browser: logged out.
        for days in [Some(-1), None] {
            let jar = latam_jar(
                now,
                &[("www.latamairlines.com", "_xp_session", days)],
            );
            assert!(session_cookies(jar, &LATAM, now).is_empty(), "{days:?}");
        }
        // The same name on another host is not the session.
        for host in [".latamairlines.com", "auth.latamairlines.com"] {
            let jar = latam_jar(now, &[(host, "_xp_session", Some(5))]);
            assert!(session_cookies(jar, &LATAM, now).is_empty(), "{host}");
        }
    }

    #[test]
    fn other_sites_keep_the_general_rule() {
        let now = Utc::now();
        let later = Some((now + TimeDelta::days(30)).timestamp_micros());
        let jar = vec![
            cookie(".smiles.com.br", "_abck", later),
            cookie("www.smiles.com.br", "token", later),
        ];
        let found = session_cookies(jar, &["smiles.com.br"], now);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "token");
    }

    #[test]
    fn chromium_times_start_in_1601() {
        assert_eq!(chromium_time(0), None);
        assert_eq!(
            chromium_time(WINDOWS_TO_UNIX_MICROS),
            DateTime::from_timestamp(0, 0)
        );
    }

    /// A jar shaped like Chromium's (the columns read here, with
    /// Chromium's names).
    async fn jar(dir: &Path, rows: &[(&str, &str, i64)]) {
        let network = dir.join("Default").join("Network");
        std::fs::create_dir_all(&network).expect("dir");
        let options = SqliteConnectOptions::new()
            .filename(network.join("Cookies"))
            .create_if_missing(true);
        let mut c = SqliteConnection::connect_with(&options)
            .await
            .expect("create");
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS cookies (creation_utc INTEGER NOT NULL, \
             host_key TEXT NOT NULL, name TEXT NOT NULL, \
             value TEXT NOT NULL, encrypted_value BLOB NOT NULL, \
             path TEXT NOT NULL, expires_utc INTEGER NOT NULL, \
             has_expires INTEGER NOT NULL)",
        )
        .execute(&mut c)
        .await
        .expect("table");
        for (host, name, expires) in rows {
            sqlx::query(
                "INSERT INTO cookies VALUES (0, ?1, ?2, '', x'00', '/', ?3, ?4)",
            )
            .bind(host)
            .bind(name)
            .bind(expires)
            .bind(i64::from(*expires > 0))
            .execute(&mut c)
            .await
            .expect("row");
        }
        c.close().await.expect("close");
    }

    fn chromium_micros(t: DateTime<Utc>) -> i64 {
        t.timestamp_micros() + WINDOWS_TO_UNIX_MICROS
    }

    #[tokio::test]
    async fn reads_a_profiles_jar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = Utc::now();
        let later = chromium_micros(now + TimeDelta::days(30));
        let domains = ["latamairlines.com", "latam.com"];

        assert!(
            stored_cookies(dir.path(), &domains)
                .await
                .expect("empty")
                .is_empty()
        );

        jar(
            dir.path(),
            &[
                (".latamairlines.com", "_abck", later),
                (".latamairlines.com", "bm_sz", later),
                ("www.latamairlines.com", "session", 0),
                (".smiles.com.br", "token", later),
            ],
        )
        .await;
        let all = stored_cookies(dir.path(), &domains).await.expect("read");
        assert_eq!(all.len(), 3);
        assert!(
            login_cookies(dir.path(), &domains, now)
                .await
                .expect("read")
                .is_empty(),
            "trackers and session cookies are not a login"
        );
        let smiles = login_cookies(dir.path(), &["smiles.com.br"], now)
            .await
            .expect("read");
        assert_eq!(smiles.len(), 1);
        assert_eq!(smiles[0].name, "token");

        // A persistent LATAM cookie that is not a tracker is still not a
        // login; `_xp_session` is.
        jar(
            dir.path(),
            &[("www.latamairlines.com", "_xp_show_login_incentive", later)],
        )
        .await;
        assert!(
            login_cookies(dir.path(), &domains, now)
                .await
                .expect("read")
                .is_empty()
        );
        jar(
            dir.path(),
            &[("www.latamairlines.com", "_xp_session", later)],
        )
        .await;
        let latam = login_cookies(dir.path(), &domains, now)
            .await
            .expect("read");
        assert_eq!(latam.len(), 1);
        assert_eq!(latam[0].name, "_xp_session");
    }
}
