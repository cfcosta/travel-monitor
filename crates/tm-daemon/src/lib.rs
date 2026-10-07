//! The travel monitor daemon, `tmd` (DECISIONS.md 0017, 0045): it
//! schedules checks, drives its own headless Chromium (one per site,
//! 0062), and records prices, Explore hints and deals in the database
//! the app reads.
//!
//! - [`cli`]: the command line, as the home-manager module passes it
//!   (0037).
//! - [`daemon`]: start-up, the loop, the app's requests, and one worker
//!   per site.
//! - [`worker`]: one site's worker: its browser, budget, pacing and
//!   tick; checks, deals and confirmations.
//! - [`sources`]: where prices come from, behind the [`sources::Sources`]
//!   trait; [`sources::browser`] is the real one.
//! - [`sites`]: the airline and miles sites verified to run (0050),
//!   with their pure parsers ([`sites::tap`], 0051).
//! - [`jobs`], [`candidates`], [`deals`]: the pure glue between the
//!   store, the scheduler and the sources.
//! - [`budget`]: each site's page-load budget, persisted (0039, 0060).
//! - [`notify`]: desktop notifications for confirmed deals (0080).

pub mod budget;
pub mod candidates;
pub mod cli;
pub mod daemon;
pub mod deals;
pub mod jobs;
pub mod notify;
pub mod sites;
pub mod sources;
pub mod worker;

pub use daemon::{Config, Daemon, SourceFactory};
