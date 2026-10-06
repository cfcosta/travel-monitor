//! What to check next, change rates, baselines and deal rules. Pure logic,
//! no I/O.
//!
//! Nothing in this crate reads the clock, touches the disk or the network,
//! or uses a global random number generator: the current time and an
//! [`rand::Rng`] are always passed in, so every decision can be replayed
//! (DECISIONS.md 0028).
//!
//! Units: change rates are **per day** and elapsed times are **days**
//! (`f64`). Prices are integer minor units (`i64`), as in `tm-domain`.
//!
//! | Module | Decision | What it does |
//! | --- | --- | --- |
//! | [`state`] | 0028 | Per-family posteriors and their priors |
//! | [`score`] | 0028 | The greedy value index, Thompson sampling |
//! | [`plan`] | 0028, 0029 | Which families to query this tick |
//! | [`rates`] | 0028 | Change-rate estimators from changed/unchanged bits |
//! | [`update`] | 0028 | Posterior updates after a check |
//! | [`hints`] | 0028, 0041 | Explore hints → families |
//! | [`fanout`] | 0028 | Families related to one that dropped |
//! | [`baseline`] | 0030 | Days-to-departure buckets, seasons, median and MAD |
//! | [`deals`] | 0018, 0030 | The deal rule |
//! | [`trips`] | 0042, 0043 | Legs → trips, the learned round-trip discount |
//! | [`budget`] | 0039 | The daily page-load budget and per-site backoff |
//! | [`effective`] | 0033, 0038 | Effective BRL cost with an FX table |

pub mod baseline;
pub mod budget;
pub mod deals;
pub mod effective;
pub mod fanout;
pub mod hints;
pub mod params;
pub mod plan;
pub mod rates;
pub mod score;
pub mod state;
pub mod trips;
pub mod update;

pub use baseline::{Baseline, BaselineKey, DtdBucket, PricePoint, Season};
pub use budget::{BudgetState, SiteBackoff};
pub use deals::evaluate;
pub use effective::FxTable;
pub use params::Params;
pub use plan::{Candidate, Pick, TickBudget, plan_tick};
pub use state::FamilyState;
pub use trips::{RoundTripDiscount, Trip};
pub use update::{CheckSummary, update_after_check};

/// Seconds in a day, for converting `chrono` durations to days.
pub(crate) const SECONDS_PER_DAY: f64 = 86_400.0;

/// The time between two instants in days. Negative when `to` is before
/// `from`.
pub(crate) fn days_between(
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> f64 {
    (to - from).num_milliseconds() as f64 / 1000.0 / SECONDS_PER_DAY
}
