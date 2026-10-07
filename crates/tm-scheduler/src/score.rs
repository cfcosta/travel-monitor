//! The greedy value index (DECISIONS.md 0028; research, "Proposed
//! scheduler").
//!
//! ```text
//! Δ     ~ Gamma(chg_a, chg_b)                    (Thompson) or its mean
//! q     = max(Δ · P(drop), ε)                    deals per day
//! p     = hint precision
//! τ     = days since the last check
//! τe    = τ + n_hints · (−ln(1 − p)) / q
//! x     = q · τe
//! score = w · (1 − e^{−x}(1 + x)) / (q · c)
//! ```
//!
//! `1 − e^{−x}(1 + x)` is the Azar et al. (PNAS 2018) crawl value, and
//! the Whittle index of Tripathi & Modiano (arXiv:1908.10438) for the
//! penalty "a deal appeared and is still unseen". It rises with τ and
//! saturates at `w / (q · c)`, so families that change faster than we can
//! follow are worth *less* per check, not more. Hints add effective time
//! as in Busa-Fekete et al. (arXiv:2502.02430).

use chrono::{DateTime, Utc};
use rand::Rng;

use crate::{Params, state::FamilyState};

/// The cost floor (page loads), so a family whose checks failed before
/// loading anything is not scored as free.
pub const MIN_COST: f64 = 0.25;
/// Hint precision is capped so `−ln(1 − p)` stays finite (≤ 6.9).
pub const MAX_HINT_PRECISION: f64 = 0.999;

/// `1 − e^{−x}(1 + x)`: the probability that at least one deal appeared
/// in `x` expected deals, weighted for how long it went unseen. Increasing
/// in `x`, 0 at 0 and → 1 as `x → ∞`. Accurate for tiny `x` (series) and
/// exact 1 for huge or infinite `x`.
pub fn value_fraction(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    if x < 1e-2 {
        // x²/2 − x³/3 + x⁴/8 − x⁵/30: error below x⁶/144 ≈ 1e-14 · x².
        let x2 = x * x;
        return x2 * (0.5 - x / 3.0 + x2 / 8.0 - x2 * x / 30.0);
    }
    if x > 745.0 {
        return 1.0;
    }
    (-(-x).exp_m1() - x * (-x).exp()).clamp(0.0, 1.0)
}

/// The rate at which deals appear, `max(Δ · P(drop), ε)`.
pub fn deal_rate(change_rate: f64, drop_probability: f64, epsilon: f64) -> f64 {
    let eps = if epsilon.is_finite() && epsilon > 0.0 {
        epsilon
    } else {
        1e-9
    };
    let q = change_rate * drop_probability.clamp(0.0, 1.0);
    if q.is_finite() { q.max(eps) } else { eps }
}

/// The extra days `n_hints` are worth: `n · (−ln(1 − p)) / q`.
pub fn hint_days(n_hints: u32, precision: f64, q: f64) -> f64 {
    if n_hints == 0 || q <= 0.0 {
        return 0.0;
    }
    let p = if precision.is_finite() {
        precision.clamp(0.0, MAX_HINT_PRECISION)
    } else {
        0.0
    };
    f64::from(n_hints) * -(-p).ln_1p() / q
}

/// The index for explicit inputs. `tau_days = ∞` (never checked) gives
/// the saturated value `w / (q · c)`.
pub fn index(weight: f64, cost: f64, q: f64, tau_effective_days: f64) -> f64 {
    let w = if weight.is_finite() {
        weight.max(0.0)
    } else {
        0.0
    };
    let c = if cost.is_finite() {
        cost.max(MIN_COST)
    } else {
        MIN_COST
    };
    let s = w * value_fraction(q * tau_effective_days) / (q * c);
    if s.is_finite() { s } else { 0.0 }
}

/// The score of `state` at `now` for a given change rate Δ (per day).
pub fn score_with_rate(
    state: &FamilyState,
    weight: f64,
    now: DateTime<Utc>,
    params: &Params,
    change_rate: f64,
) -> f64 {
    let q =
        deal_rate(change_rate, state.drop_probability(), params.epsilon_rate);
    let tau = state.elapsed_days(now).unwrap_or(f64::INFINITY);
    let tau_e = tau + hint_days(state.n_hints, state.hint_precision(), q);
    index(weight, state.cost_loads, q, tau_e)
}

/// The score of `state` at `now`, sampling Δ from its posterior when
/// `params.thompson` is set, using the posterior mean otherwise.
pub fn score<R: Rng + ?Sized>(
    state: &FamilyState,
    weight: f64,
    now: DateTime<Utc>,
    params: &Params,
    rng: &mut R,
) -> f64 {
    let rate = if params.thompson {
        sample_change_rate(state, rng)
    } else {
        state.change_rate()
    };
    score_with_rate(state, weight, now, params, rate)
}

/// One Thompson sample of Δ from Gamma(`chg_a`, `chg_b`). Falls back to
/// the posterior mean when the posterior is degenerate.
pub fn sample_change_rate<R: Rng + ?Sized>(
    state: &FamilyState,
    rng: &mut R,
) -> f64 {
    let (a, b) = (state.chg_a, state.chg_b);
    if !(a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0) {
        return state.change_rate();
    }
    let s = sample_gamma(a, rng) / b;
    if s.is_finite() && s >= 0.0 {
        s
    } else {
        state.change_rate()
    }
}

/// A Gamma(`shape`, 1) sample (Marsaglia & Tsang 2000; shapes below one
/// by the `U^{1/a}` boost). `shape` must be positive and finite.
pub fn sample_gamma<R: Rng + ?Sized>(shape: f64, rng: &mut R) -> f64 {
    if shape < 1.0 {
        let u = open_unit(rng);
        return sample_gamma(shape + 1.0, rng) * u.powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        let x = standard_normal(rng);
        let v = 1.0 + c * x;
        if v <= 0.0 {
            continue;
        }
        let v = v * v * v;
        let u = open_unit(rng);
        let x2 = x * x;
        if u < 1.0 - 0.0331 * x2 * x2
            || u.ln() < 0.5 * x2 + d * (1.0 - v + v.ln())
        {
            return d * v;
        }
    }
}

/// Uniform on (0, 1].
fn open_unit<R: Rng + ?Sized>(rng: &mut R) -> f64 {
    1.0 - rng.random::<f64>()
}

/// Box–Muller.
fn standard_normal<R: Rng + ?Sized>(rng: &mut R) -> f64 {
    let u1 = open_unit(rng);
    let u2 = rng.random::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};
    use hegel::{TestCase, generators as gs};
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0)
            .single()
            .expect("valid instant")
    }

    fn mean_params() -> Params {
        Params {
            thompson: false,
            ..Params::default()
        }
    }

    #[test]
    fn value_fraction_edges() {
        assert_eq!(value_fraction(0.0), 0.0);
        assert_eq!(value_fraction(-1.0), 0.0);
        assert_eq!(value_fraction(f64::NAN), 0.0);
        assert_eq!(value_fraction(f64::INFINITY), 1.0);
        assert_eq!(value_fraction(1e6), 1.0);
        // Tiny x: x²/2.
        let x = 1e-8;
        assert!((value_fraction(x) / (x * x / 2.0) - 1.0).abs() < 1e-6);
        // The series and the closed form agree at the switch.
        let below = value_fraction(0.01 - 1e-12);
        let above = value_fraction(0.01);
        assert!((below / above - 1.0).abs() < 1e-9, "{below} vs {above}");
        // x = 1: 1 − 2/e.
        assert!(
            (value_fraction(1.0) - (1.0 - 2.0 / std::f64::consts::E)).abs()
                < 1e-15
        );
    }

    #[test]
    fn never_checked_scores_the_saturated_value() {
        let s = FamilyState::default();
        let p = mean_params();
        // q = 1 × ½, c = 8, w = 2 → 2 / (0.5 · 8).
        let got = score_with_rate(&s, 2.0, now(), &p, 1.0);
        assert!((got - 2.0 / 4.0).abs() < 1e-12);
    }

    #[test]
    fn volatile_families_saturate_lower() {
        let s = FamilyState {
            last_check: Some(now() - TimeDelta::days(30)),
            ..FamilyState::default()
        };
        let p = mean_params();
        let slow = score_with_rate(&s, 1.0, now(), &p, 0.5);
        let fast = score_with_rate(&s, 1.0, now(), &p, 20.0);
        assert!(slow > fast, "{slow} vs {fast}");
    }

    #[test]
    fn hints_add_effective_time() {
        let base = FamilyState {
            last_check: Some(now() - TimeDelta::hours(6)),
            ..FamilyState::default()
        };
        let hinted = FamilyState {
            n_hints: 2,
            ..base.clone()
        };
        let p = mean_params();
        assert!(
            score_with_rate(&hinted, 1.0, now(), &p, 1.0)
                > score_with_rate(&base, 1.0, now(), &p, 1.0)
        );
        // A precision of 1 is capped, not infinite.
        let sure = FamilyState {
            prec_a: 1e9,
            prec_b: 0.0,
            ..hinted
        };
        assert!(score_with_rate(&sure, 1.0, now(), &p, 1.0).is_finite());
    }

    #[test]
    fn zero_rate_and_zero_weight() {
        let s = FamilyState {
            last_check: Some(now() - TimeDelta::days(3)),
            ..FamilyState::default()
        };
        let p = mean_params();
        let got = score_with_rate(&s, 1.0, now(), &p, 0.0);
        assert!(got.is_finite() && got > 0.0);
        assert_eq!(score_with_rate(&s, 0.0, now(), &p, 1.0), 0.0);
        assert_eq!(score_with_rate(&s, f64::NAN, now(), &p, 1.0), 0.0);
    }

    #[test]
    fn gamma_sampler_matches_moments() {
        let mut rng = StdRng::seed_from_u64(7);
        for &(shape, rate) in
            &[(0.3, 1.0), (1.0, 2.0), (4.0, 0.5), (50.0, 10.0)]
        {
            let n = 40_000;
            let xs: Vec<f64> = (0..n)
                .map(|_| sample_gamma(shape, &mut rng) / rate)
                .collect();
            let mean = xs.iter().sum::<f64>() / n as f64;
            let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
                / (n - 1) as f64;
            let (m, v) = (shape / rate, shape / (rate * rate));
            assert!((mean / m - 1.0).abs() < 0.03, "mean {mean} vs {m}");
            assert!((var / v - 1.0).abs() < 0.08, "var {var} vs {v}");
        }
    }

    #[test]
    fn thompson_is_deterministic_given_the_seed() {
        let s = FamilyState::default();
        let p = Params::default();
        let a = score(&s, 1.0, now(), &p, &mut StdRng::seed_from_u64(1));
        let b = score(&s, 1.0, now(), &p, &mut StdRng::seed_from_u64(1));
        assert_eq!(a, b);
    }

    #[hegel::test(test_cases = 300)]
    fn score_is_monotone_in_elapsed_time(tc: TestCase) {
        let rate: f64 = tc.draw(gs::floats().min_value(0.0).max_value(100.0));
        let drop: f64 = tc.draw(gs::floats().min_value(0.0).max_value(1.0));
        let hints: u32 = tc.draw(gs::integers::<u32>().max_value(10));
        let weight: f64 = tc.draw(gs::floats().min_value(0.0).max_value(10.0));
        let cost: f64 = tc.draw(gs::floats().min_value(0.0).max_value(50.0));
        let h1: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(24 * 400));
        let h2: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(24 * 400));
        let (short, long) = (h1.min(h2), h1.max(h2));
        let base = FamilyState {
            deal_a: drop * 10.0,
            deal_b: (1.0 - drop) * 10.0,
            n_hints: hints,
            cost_loads: cost,
            ..FamilyState::default()
        };
        let at = |hours: i64| FamilyState {
            last_check: Some(now() - TimeDelta::hours(hours)),
            ..base.clone()
        };
        let p = mean_params();
        let s_short = score_with_rate(&at(short), weight, now(), &p, rate);
        let s_long = score_with_rate(&at(long), weight, now(), &p, rate);
        let s_never = score_with_rate(&base, weight, now(), &p, rate);
        assert!(
            s_short.is_finite() && s_long.is_finite() && s_never.is_finite()
        );
        assert!(s_short >= 0.0);
        assert!(s_long >= s_short, "{s_long} < {s_short}");
        assert!(s_never >= s_long * (1.0 - 1e-12), "{s_never} < {s_long}");
    }

    #[hegel::test(test_cases = 200)]
    fn score_is_finite_for_any_state(tc: TestCase) {
        let f = || gs::floats::<f64>().allow_nan(true).allow_infinity(true);
        let s = FamilyState {
            chg_a: tc.draw(f()),
            chg_b: tc.draw(f()),
            deal_a: tc.draw(f()),
            deal_b: tc.draw(f()),
            prec_a: tc.draw(f()),
            prec_b: tc.draw(f()),
            n_hints: tc.draw(gs::integers::<u32>()),
            cost_loads: tc.draw(f()),
            last_check: Some(
                now()
                    - TimeDelta::hours(
                        tc.draw(
                            gs::integers::<i64>()
                                .min_value(-1_000_000)
                                .max_value(1_000_000),
                        ),
                    ),
            ),
            ..FamilyState::default()
        };
        let mut rng = StdRng::seed_from_u64(tc.draw(gs::integers::<u64>()));
        let got = score(&s, tc.draw(f()), now(), &Params::default(), &mut rng);
        assert!(got.is_finite() && got >= 0.0, "{got}");
    }
}
