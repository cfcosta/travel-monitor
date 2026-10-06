//! Tunables, built from the user's [`Settings`] plus constants that are not
//! (yet) settings.

use chrono::TimeDelta;
use tm_domain::{Settings, TripRange};

/// Everything the scheduler's rules depend on.
///
/// [`Params::from_settings`] copies the user's settings and fills the
/// rest with the defaults documented on each field.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    // ---- plan (0028, 0029) ----
    /// A family is never checked again sooner than this.
    pub min_interval: TimeDelta,
    /// A family not checked for this long is checked regardless of score.
    pub max_interval: TimeDelta,
    /// Share of scored slots filled uniformly at random.
    pub random_share: f64,
    /// Sample the change rate from its posterior (Thompson sampling)
    /// instead of using the posterior mean. Default: `true`.
    pub thompson: bool,
    /// The floor on the deal rate `q` (deals per day), so the score never
    /// divides by zero. Default: `1e-4` (one deal in ~27 years).
    pub epsilon_rate: f64,

    // ---- update (0028) ----
    /// A check saw a change when the minimum over the window moved by more
    /// than this fraction. Default: `0.05` (research, "Proposed
    /// scheduler").
    pub material_change: f64,
    /// Weight of the newest check in the page-load cost EWMA.
    /// Default: `0.3`.
    pub cost_alpha: f64,
    /// The change-rate posterior remembers about this many days of
    /// exposure; older evidence is discounted so the rate can follow
    /// departures getting closer. Default: `60`.
    pub rate_memory_days: f64,
    /// Pseudo-observations kept in the Beta posteriors (drop share and
    /// hint precision) before older evidence is discounted.
    /// Default: `50`.
    pub beta_memory: f64,

    // ---- hints (0028, 0041) ----
    /// An Explore price counts as a hint when it is at least this far
    /// below the route's latest known minimum. Default: `0.10`.
    pub hint_drop: f64,

    // ---- baselines and deals (0030) ----
    /// Observations older than this are left out of the median and MAD.
    pub baseline_window: TimeDelta,
    /// A deal is at least this far under the bucket median.
    pub deal_threshold: f64,
    /// And at least this many robust standard deviations under it, when
    /// the spread is not zero. Default: `2.0`.
    pub deal_min_z: f64,
    /// Weeks of observations a bucket needs before it flags deals.
    pub min_history_weeks: f64,

    // ---- trips (0042, 0043) ----
    pub trips: TripRange,
    /// Round trips "match" their legs within this fraction.
    /// Default: `0.01`.
    pub additive_tolerance: f64,
    /// Checks in a row that match before round-trip tracking stops.
    pub additive_checks_to_stop: u16,
    /// Weight of the newest observation in the discount EWMA once warmed
    /// up. Default: `0.2`.
    pub discount_alpha: f64,

    // ---- budget (0039) ----
    pub daily_budget: u32,
    pub max_daily_budget: u32,
    /// The budget never drops below this after a block. Default: `50`.
    pub min_daily_budget: u32,
    /// Raise per full day without a block. Default: `0.10`.
    pub budget_ramp: f64,
    /// The first cooldown after a block on a site. Default: 2 h.
    pub block_cooldown: TimeDelta,
    /// Cooldowns double per consecutive block up to this. Default: 48 h.
    pub max_block_cooldown: TimeDelta,
    /// A block more than this long after the previous one starts the
    /// backoff over. Default: 72 h.
    pub backoff_reset: TimeDelta,
    /// Per-tick allowance is multiplied by a uniform factor in
    /// `1 ± pacing_jitter`. Default: `0.25`.
    pub pacing_jitter: f64,

    // ---- effective cost (0038) ----
    pub iof: f64,
    pub card_spread: f64,
}

impl Params {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            min_interval: TimeDelta::hours(i64::from(
                settings.min_interval_hours,
            )),
            max_interval: TimeDelta::hours(i64::from(
                settings.max_interval_hours,
            )),
            random_share: settings.random_share.clamp(0.0, 1.0),
            thompson: true,
            epsilon_rate: 1e-4,
            material_change: 0.05,
            cost_alpha: 0.3,
            rate_memory_days: 60.0,
            beta_memory: 50.0,
            hint_drop: 0.10,
            baseline_window: TimeDelta::days(60),
            deal_threshold: settings.deal_threshold,
            deal_min_z: 2.0,
            min_history_weeks: f64::from(settings.min_history_weeks),
            trips: settings.trips,
            additive_tolerance: 0.01,
            additive_checks_to_stop: settings.additive_checks_to_stop,
            discount_alpha: 0.2,
            daily_budget: settings.daily_budget,
            max_daily_budget: settings
                .max_daily_budget
                .max(settings.daily_budget),
            min_daily_budget: 50.min(settings.daily_budget),
            budget_ramp: 0.10,
            block_cooldown: TimeDelta::hours(2),
            max_block_cooldown: TimeDelta::hours(48),
            backoff_reset: TimeDelta::hours(72),
            pacing_jitter: 0.25,
            iof: settings.iof,
            card_spread: settings.card_spread,
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Self::from_settings(&Settings::default())
    }
}

impl From<&Settings> for Params {
    fn from(settings: &Settings) -> Self {
        Self::from_settings(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_settings() {
        let settings = Settings {
            min_interval_hours: 4,
            max_interval_hours: 48,
            random_share: 0.2,
            ..Settings::default()
        };
        let p = Params::from_settings(&settings);
        assert_eq!(p.min_interval, TimeDelta::hours(4));
        assert_eq!(p.max_interval, TimeDelta::hours(48));
        assert_eq!(p.random_share, 0.2);
        assert_eq!(p.daily_budget, 2400);
        assert_eq!(p.min_history_weeks, 4.0);
    }

    #[test]
    fn min_budget_never_exceeds_start() {
        let settings = Settings {
            daily_budget: 20,
            ..Settings::default()
        };
        assert_eq!(Params::from_settings(&settings).min_daily_budget, 20);
    }
}
