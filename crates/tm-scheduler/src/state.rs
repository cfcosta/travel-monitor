//! What the scheduler remembers about one query family (DECISIONS.md 0028).
//!
//! The store persists exactly these fields, under the same names.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::days_between;

/// Prior shape of the change-rate posterior: one pseudo-change …
pub const PRIOR_CHG_A: f64 = 1.0;
/// … over one pseudo-day, so the prior mean is one change per day.
/// Airfares re-price about once a day (Etzioni et al. 2003; Williams
/// 2022). The prior is weak: a few checks dominate it.
pub const PRIOR_CHG_B: f64 = 1.0;
/// Prior on P(a change is a drop): Beta(1, 1), mean ½. Fares move both
/// ways; with no data neither direction is favoured.
pub const PRIOR_DEAL_A: f64 = 1.0;
pub const PRIOR_DEAL_B: f64 = 1.0;
/// Prior on Explore hint precision: Beta(1, 2), mean ⅓. Explore prices a
/// different trip (about a week, one date pair, 0041), so a hint is
/// assumed more often wrong than right until shown otherwise.
pub const PRIOR_PREC_A: f64 = 1.0;
pub const PRIOR_PREC_B: f64 = 2.0;
/// Prior page loads per check: a leg's price graph is a few loads
/// (research: "about 67 families × about 3 grid loads").
pub const PRIOR_COST_LOADS: f64 = 3.0;

/// Learned state of one query family.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyState {
    /// Gamma(shape `chg_a`, rate `chg_b`) posterior on the change rate Δ,
    /// in changes per day. `chg_b` is days of exposure.
    pub chg_a: f64,
    pub chg_b: f64,
    /// Beta posterior on P(a change is a drop).
    pub deal_a: f64,
    pub deal_b: f64,
    /// Beta posterior on P(an Explore hint for this family is right).
    pub prec_a: f64,
    pub prec_b: f64,
    /// Explore hints (and fan-out boosts) since the last check.
    pub n_hints: u32,
    /// When it was last checked; `None` until its first check.
    pub last_check: Option<DateTime<Utc>>,
    /// The minimum price over the window at the last check.
    pub last_min_price_minor: Option<i64>,
    /// Checks so far.
    pub checks: u32,
    /// Expected page loads per check (EWMA).
    pub cost_loads: f64,
}

impl Default for FamilyState {
    fn default() -> Self {
        Self {
            chg_a: PRIOR_CHG_A,
            chg_b: PRIOR_CHG_B,
            deal_a: PRIOR_DEAL_A,
            deal_b: PRIOR_DEAL_B,
            prec_a: PRIOR_PREC_A,
            prec_b: PRIOR_PREC_B,
            n_hints: 0,
            last_check: None,
            last_min_price_minor: None,
            checks: 0,
            cost_loads: PRIOR_COST_LOADS,
        }
    }
}

impl FamilyState {
    /// Posterior mean change rate, per day.
    pub fn change_rate(&self) -> f64 {
        positive_ratio(self.chg_a, self.chg_b, PRIOR_CHG_A / PRIOR_CHG_B)
    }

    /// Posterior mean P(a change is a drop).
    pub fn drop_probability(&self) -> f64 {
        beta_mean(self.deal_a, self.deal_b, 0.5)
    }

    /// Posterior mean hint precision.
    pub fn hint_precision(&self) -> f64 {
        beta_mean(
            self.prec_a,
            self.prec_b,
            PRIOR_PREC_A / (PRIOR_PREC_A + PRIOR_PREC_B),
        )
    }

    /// Days since the last check, `None` if never checked. Never negative
    /// (a `last_check` in the future counts as "just now").
    pub fn elapsed_days(&self, now: DateTime<Utc>) -> Option<f64> {
        self.last_check.map(|last| days_between(last, now).max(0.0))
    }

    /// The expected page loads of one check, at least one.
    pub fn expected_loads(&self) -> u32 {
        let cost = if self.cost_loads.is_finite() {
            self.cost_loads
        } else {
            PRIOR_COST_LOADS
        };
        cost.ceil().clamp(1.0, f64::from(u32::MAX)) as u32
    }
}

fn positive_ratio(a: f64, b: f64, fallback: f64) -> f64 {
    let r = a / b;
    if r.is_finite() && r > 0.0 {
        r
    } else {
        fallback
    }
}

fn beta_mean(a: f64, b: f64, fallback: f64) -> f64 {
    let m = a / (a + b);
    if m.is_finite() && (0.0..=1.0).contains(&m) {
        m
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn priors() {
        let s = FamilyState::default();
        assert_eq!(s.change_rate(), 1.0);
        assert_eq!(s.drop_probability(), 0.5);
        assert!((s.hint_precision() - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(s.expected_loads(), 3);
        assert_eq!(s.elapsed_days(Utc::now()), None);
    }

    #[test]
    fn degenerate_posteriors_fall_back() {
        let s = FamilyState {
            chg_a: 0.0,
            chg_b: 0.0,
            deal_a: 0.0,
            deal_b: 0.0,
            cost_loads: f64::NAN,
            ..FamilyState::default()
        };
        assert_eq!(s.change_rate(), 1.0);
        assert_eq!(s.drop_probability(), 0.5);
        assert_eq!(s.expected_loads(), 3);
    }

    #[test]
    fn elapsed_is_never_negative() {
        let now = Utc
            .with_ymd_and_hms(2026, 10, 6, 12, 0, 0)
            .single()
            .expect("valid instant");
        let s = FamilyState {
            last_check: Some(now + chrono::TimeDelta::hours(1)),
            ..FamilyState::default()
        };
        assert_eq!(s.elapsed_days(now), Some(0.0));
        let s = FamilyState {
            last_check: Some(now - chrono::TimeDelta::hours(36)),
            ..FamilyState::default()
        };
        assert_eq!(s.elapsed_days(now), Some(1.5));
    }
}
