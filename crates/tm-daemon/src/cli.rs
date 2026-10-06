//! The command line (DECISIONS.md 0037). The flags the home-manager
//! module passes (`nix/hm-module.nix`) are the contract:
//!
//! ```text
//! tmd --db <path> --profile-dir <path> --chromium <path> [--headed]
//!     --cdp-port <n> [--jev-key-file <path>] --jev-model <name>
//!     --budget <n> --max-budget <n>
//! ```
//!
//! `--db`, `--profile-dir` and `--chromium` fall back to `TM_DB`,
//! `TM_PROFILE_DIR` and `TM_CHROMIUM`, then to the module's defaults.
//! The hidden flags exist for debugging and tests.

use std::{path::PathBuf, time::Duration};

use clap::Parser;

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

    /// Chromium's user data directory (default:
    /// $XDG_DATA_HOME/travel-monitor/chromium). Miles logins live here.
    #[arg(long, env = "TM_PROFILE_DIR", value_name = "PATH")]
    pub profile_dir: Option<PathBuf>,

    /// The Chromium binary (default: `chromium` on the PATH).
    #[arg(long, env = "TM_CHROMIUM", value_name = "PATH")]
    pub chromium: Option<PathBuf>,

    /// Show the browser window instead of running headless.
    #[arg(long)]
    pub headed: bool,

    /// Chromium's DevTools port; 0 picks a free one.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub cdp_port: u16,

    /// A file holding the Jev API key. Without it the daemon runs
    /// without Jev: no airline or miles sources and no recovery.
    #[arg(long, value_name = "PATH")]
    pub jev_key_file: Option<PathBuf>,

    /// The Jev model to ask.
    #[arg(long, default_value = tm_jev::DEFAULT_MODEL, value_name = "NAME")]
    pub jev_model: String,

    /// The daily page-load budget to start from (DECISIONS.md 0039).
    #[arg(long, value_name = "N")]
    pub budget: Option<u32>,

    /// The highest daily budget the ramp may reach.
    #[arg(long, value_name = "N")]
    pub max_budget: Option<u32>,

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
            "--jev-key-file",
            "/run/secrets/jev",
            "--jev-model",
            "jev-latest",
            "--budget",
            "300",
            "--max-budget",
            "2000",
        ])
        .expect("parses");
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
        assert_eq!(cli.jev_key_file, Some(PathBuf::from("/run/secrets/jev")));
        assert_eq!(cli.jev_model, "jev-latest");
        assert_eq!(cli.budget, Some(300));
        assert_eq!(cli.max_budget, Some(2000));
        assert!(!cli.once);
        assert_eq!(cli.tick(), Duration::from_secs(60));
        assert_eq!(cli.pace, (4, 15));
    }

    /// Headless, no Jev: the module leaves `--headed` and
    /// `--jev-key-file` out.
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
            "--jev-model",
            "jev-1",
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
        assert_eq!(cli.jev_key_file, None);
        assert_eq!(cli.jev_model, "jev-1");
        assert!(cli.once);
        assert_eq!(cli.tick(), Duration::from_secs(5));
        assert_eq!(cli.only_route.as_deref(), Some("LIS"));
        assert_eq!(cli.max_graph_pages, Some(2));
        assert_eq!(cli.pace, (0, 0));
    }

    #[test]
    fn defaults() {
        let cli = Cli::try_parse_from(["tmd"]).expect("parses");
        assert_eq!(cli.jev_model, tm_jev::DEFAULT_MODEL);
        assert_eq!(cli.cdp_port, 0);
        assert_eq!(cli.budget, None);
        assert!(cli.db_path().ends_with("travel-monitor/tm.db"));
        assert!(cli.profile_path().ends_with("travel-monitor/chromium"));
        assert!(Cli::try_parse_from(["tmd", "--pace", "5,1"]).is_err());
        assert!(Cli::try_parse_from(["tmd", "--cdp-port", "x"]).is_err());
    }
}
