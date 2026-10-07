//! Posterior updates after a check (DECISIONS.md 0028).
//!
//! **Material change.** A check "saw a change" when the minimum price over
//! the family's window moved by more than `material_change` (5%) since the
//! previous check, or when the caller says so (`any_material_change`, for
//! example a cell other than the minimum moving by more than 5%). It "saw
//! a drop" when that minimum fell by more than 5%, or the caller says so.
//! A missing minimum on either side (no prices shown) is neither.
//!
//! **Change rate.** Gamma(`chg_a`, `chg_b`) on Δ, updated online by EM for
//! the interval-censored Poisson likelihood ([`crate::rates`]): the gap
//! `a` is added to `chg_b`, and a changed check adds the expected number
//! of changes behind it, `m/(1 − e^{−m})` with `m = Δ̂·a`, to `chg_a`. Its
//! fixed point is the censored MLE, so long gaps do not bias Δ down the
//! way counting one change per changed check would. Exposure beyond
//! `rate_memory_days` is discounted (both parameters scaled down, keeping
//! the mean), so the rate can follow a departure getting closer.
//!
//! **Which checks teach the rate.** A check whose timing was driven by a
//! signal correlated with a change (a hint, a fan-out, a confirmation, a
//! manual request, or a scheduled pick that carried hints) would bias Δ
//! and P(drop) upward, so it updates neither. Random, bootstrap, overdue
//! and hint-free scheduled checks do: their timing depends only on what
//! was already observed, which the likelihood can ignore.
//!
//! **Hint precision.** If hints were pending, the check confirms them when
//! it saw a drop ([`crate::hints::update_hint_precision`]). Pending hints
//! are then reset.
//!
//! **Cost.** `cost_loads` is an EWMA of page loads per check with weight
//! `max(cost_alpha, 1/(checks + 1))`, so the first checks average and
//! later ones decay.

use chrono::{DateTime, Utc};
use tm_domain::QueryReason;

use crate::{
    Params,
    hints::update_hint_precision,
    rates::expected_changes_given_changed,
    state::FamilyState,
};

/// What one check of a family found.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckSummary {
    /// The minimum price over the window, `None` if no cell had a price.
    pub min_price_minor: Option<i64>,
    /// The caller saw a material change (see the module docs).
    pub any_material_change: bool,
    /// The caller saw a material drop.
    pub drop: bool,
    /// Page loads the check used.
    pub page_loads: u32,
    pub reason: QueryReason,
}

/// How the window minimum moved between two checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MinMove {
    pub changed: bool,
    pub dropped: bool,
}

/// Compare two window minimums: a change is a relative move strictly
/// greater than `threshold`.
pub fn min_moved(
    previous: Option<i64>,
    current: Option<i64>,
    threshold: f64,
) -> MinMove {
    let (Some(prev), Some(cur)) = (previous, current) else {
        return MinMove::default();
    };
    if prev <= 0 {
        return MinMove::default();
    }
    let rel = (cur - prev) as f64 / prev as f64;
    MinMove {
        changed: rel.abs() > threshold,
        dropped: rel < -threshold,
    }
}

/// [`update_after_check_with`] with default parameters (none of the
/// update parameters are user settings).
pub fn update_after_check(
    state: &mut FamilyState,
    summary: &CheckSummary,
    now: DateTime<Utc>,
) {
    update_after_check_with(state, summary, now, &Params::default());
}

/// Fold one successful check into the family's state. Call it only for
/// checks that loaded (`QueryOutcome::Ok`); a blocked or failed check
/// taught nothing about prices.
pub fn update_after_check_with(
    state: &mut FamilyState,
    summary: &CheckSummary,
    now: DateTime<Utc>,
    params: &Params,
) {
    let moved = min_moved(
        state.last_min_price_minor,
        summary.min_price_minor,
        params.material_change,
    );
    let dropped = summary.drop || moved.dropped;
    let changed = summary.any_material_change || moved.changed || dropped;
    let had_hints = state.n_hints > 0;

    if let Some(gap) = state.elapsed_days(now)
        && gap > 0.0
        && teaches_rate(&summary.reason, had_hints)
    {
        if changed {
            let m = state.change_rate() * gap;
            state.chg_a += expected_changes_given_changed(m);
            if dropped {
                state.deal_a += 1.0;
            } else {
                state.deal_b += 1.0;
            }
        }
        state.chg_b += gap;
        forget_gamma(state, params.rate_memory_days);
        let (a, b) =
            forget_beta(state.deal_a, state.deal_b, params.beta_memory);
        state.deal_a = a;
        state.deal_b = b;
    }

    if had_hints {
        update_hint_precision(state, dropped);
        let (a, b) =
            forget_beta(state.prec_a, state.prec_b, params.beta_memory);
        state.prec_a = a;
        state.prec_b = b;
    }
    state.n_hints = 0;

    let alpha = params
        .cost_alpha
        .max(1.0 / (f64::from(state.checks) + 1.0))
        .clamp(0.0, 1.0);
    let cost = if state.cost_loads.is_finite() {
        state.cost_loads
    } else {
        f64::from(summary.page_loads)
    };
    state.cost_loads =
        (1.0 - alpha) * cost + alpha * f64::from(summary.page_loads);

    state.checks = state.checks.saturating_add(1);
    if state.last_check.is_none_or(|last| last < now) {
        state.last_check = Some(now);
    }
    if summary.min_price_minor.is_some() {
        state.last_min_price_minor = summary.min_price_minor;
    }
}

/// Whether a check with this reason may update Δ and P(drop).
pub fn teaches_rate(reason: &QueryReason, had_hints: bool) -> bool {
    match reason {
        QueryReason::Random | QueryReason::Bootstrap | QueryReason::Overdue => {
            true
        }
        QueryReason::Scheduled { .. } => !had_hints,
        QueryReason::Hint
        | QueryReason::FanOut
        | QueryReason::Confirmation
        | QueryReason::Manual => false,
    }
}

fn forget_gamma(state: &mut FamilyState, memory_days: f64) {
    if memory_days > 0.0 && state.chg_b > memory_days {
        let f = memory_days / state.chg_b;
        state.chg_a *= f;
        state.chg_b = memory_days;
    }
}

fn forget_beta(a: f64, b: f64, memory: f64) -> (f64, f64) {
    let total = a + b;
    if memory > 0.0 && total > memory {
        let f = memory / total;
        (a * f, b * f)
    } else {
        (a, b)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};
    use rand::{Rng, SeedableRng, rngs::StdRng};

    use super::*;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, 0, 0, 0)
            .single()
            .expect("valid instant")
    }

    fn summary(min: Option<i64>, reason: QueryReason) -> CheckSummary {
        CheckSummary {
            min_price_minor: min,
            any_material_change: false,
            drop: false,
            page_loads: 4,
            reason,
        }
    }

    #[test]
    fn material_change_is_strictly_more_than_five_percent() {
        assert_eq!(min_moved(Some(1000), Some(1050), 0.05), MinMove::default());
        assert_eq!(
            min_moved(Some(1000), Some(1051), 0.05),
            MinMove {
                changed: true,
                dropped: false
            }
        );
        assert_eq!(
            min_moved(Some(1000), Some(900), 0.05),
            MinMove {
                changed: true,
                dropped: true
            }
        );
        assert_eq!(min_moved(None, Some(900), 0.05), MinMove::default());
        assert_eq!(min_moved(Some(900), None, 0.05), MinMove::default());
        assert_eq!(min_moved(Some(0), Some(900), 0.05), MinMove::default());
    }

    #[test]
    fn first_check_sets_baseline_only() {
        let mut s = FamilyState::default();
        update_after_check(
            &mut s,
            &summary(Some(1000), QueryReason::Bootstrap),
            t0(),
        );
        assert_eq!(s.chg_a, 1.0);
        assert_eq!(s.chg_b, 1.0);
        assert_eq!(s.checks, 1);
        assert_eq!(s.last_check, Some(t0()));
        assert_eq!(s.last_min_price_minor, Some(1000));
        assert_eq!(s.cost_loads, 4.0); // first check: alpha = 1
    }

    #[test]
    fn drop_updates_posteriors_and_hints() {
        let mut s = FamilyState {
            last_check: Some(t0()),
            last_min_price_minor: Some(1000),
            n_hints: 2,
            checks: 3,
            ..FamilyState::default()
        };
        let now = t0() + TimeDelta::days(2);
        update_after_check(
            &mut s,
            &summary(Some(800), QueryReason::Random),
            now,
        );
        assert!(s.chg_a > 2.0); // expected changes for m = 2 is ~2.31
        assert_eq!(s.chg_b, 3.0);
        assert_eq!(s.deal_a, 2.0);
        assert_eq!(s.deal_b, 1.0);
        assert_eq!(s.prec_a, 2.0);
        assert_eq!(s.n_hints, 0);
        assert_eq!(s.last_min_price_minor, Some(800));
        // cost: alpha = max(0.3, 1/4) = 0.3 → 0.7·8 + 0.3·4
        assert!((s.cost_loads - 6.8).abs() < 1e-12);
    }

    #[test]
    fn hint_driven_checks_do_not_teach_the_rate() {
        let mut s = FamilyState {
            last_check: Some(t0()),
            last_min_price_minor: Some(1000),
            n_hints: 1,
            ..FamilyState::default()
        };
        let mut sum = summary(Some(1100), QueryReason::Hint);
        sum.any_material_change = true;
        update_after_check(&mut s, &sum, t0() + TimeDelta::days(1));
        assert_eq!((s.chg_a, s.chg_b), (1.0, 1.0));
        assert_eq!((s.deal_a, s.deal_b), (1.0, 1.0));
        // The hint was wrong: no drop.
        assert_eq!((s.prec_a, s.prec_b), (1.0, 3.0));
        assert_eq!(s.n_hints, 0);
        assert_eq!(s.last_check, Some(t0() + TimeDelta::days(1)));
    }

    #[test]
    fn missing_minimum_keeps_the_previous_one() {
        let mut s = FamilyState {
            last_check: Some(t0()),
            last_min_price_minor: Some(1000),
            ..FamilyState::default()
        };
        update_after_check(
            &mut s,
            &summary(None, QueryReason::Overdue),
            t0() + TimeDelta::days(1),
        );
        assert_eq!(s.last_min_price_minor, Some(1000));
        assert_eq!(s.chg_b, 2.0);
    }

    #[test]
    fn memory_keeps_the_mean() {
        let mut s = FamilyState {
            chg_a: 59.0,
            chg_b: 59.0,
            last_check: Some(t0()),
            ..FamilyState::default()
        };
        update_after_check(
            &mut s,
            &summary(None, QueryReason::Random),
            t0() + TimeDelta::days(2),
        );
        assert_eq!(s.chg_b, 60.0);
        assert!((s.chg_a - 59.0 * 60.0 / 61.0).abs() < 1e-9);
    }

    /// Online EM on simulated checks converges to the true rate, where
    /// counting one change per changed check would not.
    #[test]
    fn online_update_recovers_the_rate() {
        for &true_rate in &[0.2, 1.0, 3.0] {
            let mut rng = StdRng::seed_from_u64(5);
            let mut s = FamilyState::default();
            let mut now = t0();
            let mut price = 100_000i64;
            update_after_check(
                &mut s,
                &summary(Some(price), QueryReason::Random),
                now,
            );
            let mut estimates = Vec::new();
            for i in 0..3000 {
                let gap_h = rng.random_range(6..72);
                now += TimeDelta::hours(gap_h);
                let p = 1.0 - (-true_rate * gap_h as f64 / 24.0).exp();
                if rng.random::<f64>() < p {
                    // Every change moves the minimum by more than 5%.
                    price = if price > 100_000 { 80_000 } else { 120_000 };
                }
                update_after_check(
                    &mut s,
                    &summary(Some(price), QueryReason::Random),
                    now,
                );
                if i > 1000 {
                    estimates.push(s.change_rate());
                }
            }
            let mean = estimates.iter().sum::<f64>() / estimates.len() as f64;
            assert!(
                (mean / true_rate - 1.0).abs() < 0.2,
                "true {true_rate}, estimated {mean}"
            );
        }
    }
}
