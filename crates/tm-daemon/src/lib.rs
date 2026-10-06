//! The travel monitor daemon, `tmd` (DECISIONS.md 0017, 0045): it
//! schedules checks, drives its own headless Chromium, and records
//! prices, Explore hints and deals in the database the app reads.
//!
//! - [`cli`]: the command line, as the home-manager module passes it
//!   (0037).
//! - [`daemon`]: start-up, the tick loop, the app's requests, checks,
//!   deals and confirmations.
//! - [`sources`]: where prices come from, behind the [`sources::Sources`]
//!   trait; [`sources::browser`] is the real one.
//! - [`jobs`], [`candidates`], [`deals`], [`extract`]: the pure glue
//!   between the store, the scheduler and the sources.
//! - [`budget`]: the page-load budget, persisted (0039).

pub mod budget;
pub mod candidates;
pub mod cli;
pub mod daemon;
pub mod deals;
pub mod extract;
pub mod jobs;
pub mod sources;

pub use daemon::{Config, Daemon};
