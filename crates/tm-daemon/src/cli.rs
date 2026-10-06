//! The command line (DECISIONS.md 0037). The flags the home-manager
//! module passes (`nix/hm-module.nix`) are the contract:
//!
//! ```text
//! tmd --db <path> --profile-dir <path> --chromium <path> [--headed]
//!     --cdp-port <n> --budget <n> --max-budget <n>
//!     [--site-budget <site>=<start>:<max>]...
//! ```
//!
//! `--db`, `--profile-dir` and `--chromium` fall back to `TM_DB`,
//! `TM_PROFILE_DIR` and `TM_CHROMIUM`, then to the module's defaults.
//! `--budget` and `--max-budget` are Google Flights' (0060);
//! `--site-budget` sets another site's (`airline:TP=300:1000`).
//! `--profile-dir` holds one Chromium profile per site (0062). The
//! hidden flags exist for debugging and tests.

use std::{path::PathBuf, time::Duration};

use clap::Parser;
use tm_domain::{GOOGLE_FLIGHTS, SiteLimits, Source};

#[derive(Debug, Clone, PartialEq, Parser)]
#[command(
    name = "tmd",
    version,
    about = "The travel monitor daemon: schedules checks, drives a headless Chromium and records prices."
)]
pub struct Cli {
    /// The SQLite database (default: $XDG_DATA_HOME/travel-monitor/tm.db).
    #[arg(long, env = "TM_DB", value_name = "PATH")]
    pub db: Option<PathBuf>,

    /// The directory of the sites' Chromium profiles (default:
    /// $XDG_DATA_HOME/travel-monitor/chromium): each site has its own
    /// under it (`google_flights`, `airline-tp`, `miles-smiles`), where
    /// its miles login lives.
    #[arg(long, env = "TM_PROFILE_DIR", value_name = "PATH")]
    pub profile_dir: Option<PathBuf>,

    /// The Chromium binary (default: `chromium` on the PATH).
    #[arg(long, env = "TM_CHROMIUM", value_name = "PATH")]
    pub chromium: Option<PathBuf>,

    /// Show the browser window instead of running headless.
    #[arg(long)]
    pub headed: bool,

    /// Google Flights' Chromium DevTools port; 0 picks a free one. Other
    /// sites always pick a free one.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub cdp_port: u16,

    /// Google Flights' daily page-load budget to start from
    /// (DECISIONS.md 0039, 0060).
    #[arg(long, value_name = "N")]
    pub budget: Option<u32>,

    /// The highest daily budget Google Flights' ramp may reach.
    #[arg(long, value_name = "N")]
    pub max_budget: Option<u32>,

    /// Another site's daily budget and its maximum, as
    /// `site=start:max` (`airline:TP=300:1000`). Repeatable. Sites left
    /// out start at 300 and ramp to 1,000 (0060).
    #[arg(long = "site-budget", value_name = "SITE=START:MAX", value_parser = parse_site_budget)]
    pub site_budgets: Vec<(String, SiteLimits)>,

    /// Start up, run one tick, and exit.
    #[arg(long)]
    pub once: bool,

    /// Seconds between scheduler ticks.
    #[arg(long, default_value_t = 60, value_name = "N")]
    pub tick_seconds: u64,

    /// Only check this family (debugging).
    #[arg(long, hide = true, value_name = "ID")]
    pub only_family: Option<i64>,

    /// Only check routes to this destination code, without Explore
    /// sweeps (debugging).
    #[arg(long, hide = true, value_name = "CODE")]
    pub only_route: Option<String>,

    /// Cap the price-graph pages of one check (debugging).
    #[arg(long, hide = true, value_name = "N")]
    pub max_graph_pages: Option<u32>,

    /// Seconds of random pause between page loads, as `min,max`.
    #[arg(long, hide = true, default_value = "4,15", value_parser = parse_pace)]
    pub pace: (u64, u64),

    /// Replace `https://www.google.com` in generated URLs (tests).
    #[arg(long, hide = true, value_name = "URL")]
    pub google_origin: Option<String>,

    /// Seconds without work before a site's browser closes (0062).
    #[arg(long, hide = true, default_value_t = 300, value_name = "N")]
    pub idle_close_seconds: u64,
}

fn parse_site_budget(text: &str) -> Result<(String, SiteLimits), String> {
    let (site, limits) = text
        .rsplit_once('=')
        .ok_or_else(|| "expected site=start:max".to_owned())?;
    let site = site.trim();
    if site == GOOGLE_FLIGHTS {
        return Err(
            "Google Flights' budget is --budget and --max-budget".to_owned()
        );
    }
    if Source::from_site(site).is_none() {
        return Err(format!(
            "{site:?} is not a site (google_flights, airline:XX or miles:program)"
        ));
    }
    let (start, max) = limits
        .split_once(':')
        .ok_or_else(|| "expected start:max".to_owned())?;
    let start: u32 = start.trim().parse().map_err(|e| format!("{e}"))?;
    let max: u32 = max.trim().parse().map_err(|e| format!("{e}"))?;
    if start == 0 || max < start {
        return Err("expected 0 < start <= max".to_owned());
    }
    Ok((site.to_owned(), SiteLimits { start, max }))
}

fn parse_pace(text: &str) -> Result<(u64, u64), String> {
    let (min, max) = text
        .split_once(',')
        .ok_or_else(|| "expected min,max".to_owned())?;
    let min: u64 = min.trim().parse().map_err(|e| format!("{e}"))?;
    let max: u64 = max.trim().parse().map_err(|e| format!("{e}"))?;
    if min > max {
        return Err("min must not exceed max".to_owned());
    }
    Ok((min, max))
}

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("travel-monitor")
}

impl Cli {
    pub fn db_path(&self) -> PathBuf {
        self.db.clone().unwrap_or_else(|| data_dir().join("tm.db"))
    }

    pub fn profile_path(&self) -> PathBuf {
        self.profile_dir
            .clone()
            .unwrap_or_else(|| data_dir().join("chromium"))
    }

    pub fn chromium_path(&self) -> PathBuf {
        self.chromium
            .clone()
            .unwrap_or_else(|| PathBuf::from("chromium"))
    }

    pub fn tick(&self) -> Duration {
        Duration::from_secs(self.tick_seconds.max(1))
    }

    pub fn idle_close(&self) -> Duration {
        Duration::from_secs(self.idle_close_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly what `nix/hm-module.nix` puts on the command line.
    #[test]
    fn parses_the_home_manager_command_line() {
        let cli = Cli::try_parse_from([
            "tmd",
            "--db",
            "/home/me/.local/share/travel-monitor/tm.db",
            "--profile-dir",
            "/home/me/.local/share/travel-monitor/chromium",
            "--chromium",
            "/nix/store/x-chromium/bin/chromium",
            "--headed",
            "--cdp-port",
            "0",
            "--budget",
            "300",
            "--max-budget",
            "2000",
            "--site-budget",
            "airline:TP=300:1000",
            "--site-budget",
            "miles:smiles=50:100",
        ])
        .expect("parses");
        assert_eq!(
            cli.site_budgets,
            vec![
                (
                    "airline:TP".to_owned(),
                    SiteLimits {
                        start: 300,
                        max: 1000
                    }
                ),
                (
                    "miles:smiles".to_owned(),
                    SiteLimits {
                        start: 50,
                        max: 100
                    }
                ),
            ]
        );
        assert_eq!(
            cli.db_path(),
            PathBuf::from("/home/me/.local/share/travel-monitor/tm.db")
        );
        assert_eq!(
            cli.profile_path(),
            PathBuf::from("/home/me/.local/share/travel-monitor/chromium")
        );
        assert_eq!(
            cli.chromium_path(),
            PathBuf::from("/nix/store/x-chromium/bin/chromium")
        );
        assert!(cli.headed);
        assert_eq!(cli.cdp_port, 0);
        assert_eq!(cli.budget, Some(300));
        assert_eq!(cli.max_budget, Some(2000));
        assert!(!cli.once);
        assert_eq!(cli.tick(), Duration::from_secs(60));
        assert_eq!(cli.pace, (4, 15));
    }

    /// Headless: the module leaves `--headed` out.
    #[test]
    fn optional_flags_may_be_left_out() {
        let cli = Cli::try_parse_from([
            "tmd",
            "--db",
            "/tmp/tm.db",
            "--profile-dir",
            "/tmp/p",
            "--chromium",
            "/bin/chromium",
            "--cdp-port",
            "9333",
            "--budget",
            "100",
            "--max-budget",
            "200",
            "--once",
            "--tick-seconds",
            "5",
            "--only-route",
            "LIS",
            "--max-graph-pages",
            "2",
            "--pace",
            "0,0",
        ])
        .expect("parses");
        assert!(!cli.headed);
        assert_eq!(cli.cdp_port, 9333);
        assert!(cli.once);
        assert_eq!(cli.tick(), Duration::from_secs(5));
        assert_eq!(cli.only_route.as_deref(), Some("LIS"));
        assert_eq!(cli.max_graph_pages, Some(2));
        assert_eq!(cli.pace, (0, 0));
    }

    #[test]
    fn defaults() {
        let cli = Cli::try_parse_from(["tmd"]).expect("parses");
        assert_eq!(cli.cdp_port, 0);
        assert_eq!(cli.budget, None);
        assert!(cli.db_path().ends_with("travel-monitor/tm.db"));
        assert!(cli.profile_path().ends_with("travel-monitor/chromium"));
        assert!(cli.site_budgets.is_empty());
        assert_eq!(cli.idle_close(), Duration::from_secs(300));
        assert!(Cli::try_parse_from(["tmd", "--pace", "5,1"]).is_err());
        for bad in [
            "google_flights=1:2",
            "airline:TP",
            "airline:TP=300",
            "airline:TP=300:100",
            "airline:TP=0:100",
            "bing=1:2",
        ] {
            assert!(
                Cli::try_parse_from(["tmd", "--site-budget", bad]).is_err(),
                "{bad}"
            );
        }
        assert!(Cli::try_parse_from(["tmd", "--cdp-port", "x"]).is_err());
    }
}
