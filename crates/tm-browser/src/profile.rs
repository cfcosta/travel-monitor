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
//! This is a heuristic, with these limits:
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

    /// Whether it counts toward a login at `now` (see the module docs).
    pub fn counts_as_login(&self, now: DateTime<Utc>) -> bool {
        self.expires.is_some_and(|e| e > now) && !is_tracker(&self.name)
    }
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

/// The default profile's cookie files: `Network/Cookies` since Chromium
/// 96, `Cookies` before.
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
/// at `now` (see the module docs). Empty when it does not.
pub async fn login_cookies(
    user_data_dir: &Path,
    domains: &[&str],
    now: DateTime<Utc>,
) -> Result<Vec<StoredCookie>> {
    Ok(stored_cookies(user_data_dir, domains)
        .await?
        .into_iter()
        .filter(|c| c.counts_as_login(now))
        .collect())
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
            "CREATE TABLE cookies (creation_utc INTEGER NOT NULL, \
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
    }
}
