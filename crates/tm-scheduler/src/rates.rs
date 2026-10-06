//! Change-rate estimation from "did it change since the last check?" bits
//! with irregular gaps (research, "Crawl revisit scheduling").
//!
//! A family's price is modelled as changing at the events of a Poisson
//! process with rate Δ per day. A check `a` days after the previous one
//! sees a change with probability `1 − e^{−Δa}`; it cannot tell one change
//! from several. Counting changes and dividing by time underestimates Δ;
//! these estimators do not.

/// One check: the gap since the previous check (days) and whether it saw
/// a change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub gap_days: f64,
    pub changed: bool,
}

impl Interval {
    pub fn new(gap_days: f64, changed: bool) -> Self {
        Self { gap_days, changed }
    }
}

/// Cho & Garcia-Molina (ACM TODS 2003) for equal gaps:
/// `r̂ = −ln((n − X + 0.5)/(n + 0.5)) / I`, with `n` checks, `X` that saw a
/// change, and `I` days between checks. Finite even when every check saw a
/// change. Returns 0 for `n = 0` or a non-positive interval.
pub fn cho_garcia_molina(n: u32, changes: u32, interval_days: f64) -> f64 {
    if n == 0 || !interval_days.is_finite() || interval_days <= 0.0 {
        return 0.0;
    }
    let n = f64::from(n);
    let x = f64::from(changes).min(n);
    (-((n - x + 0.5) / (n + 0.5)).ln() / interval_days).max(0.0)
}

/// The interval-censored maximum-likelihood estimate (LambdaCrawl, Kolobov
/// et al., NeurIPS 2019): the root of
///
/// ```text
/// Σ_changed a/(e^{aΔ} − 1) = Σ_unchanged a
/// ```
///
/// smoothed with half a changed and half an unchanged pseudo-check at the
/// mean gap (the "+0.5" of Cho & Garcia-Molina), so the estimate is finite
/// and positive when every or no check saw a change. With equal gaps `I`
/// it reduces to `ln((n + 1)/(n − X + 0.5)) / I`. Solved by bisection on
/// `ln Δ`. Gaps that are not finite and positive are ignored; returns 0
/// when none are left.
pub fn poisson_mle(intervals: &[Interval]) -> f64 {
    let valid: Vec<Interval> = intervals
        .iter()
        .copied()
        .filter(|i| i.gap_days.is_finite() && i.gap_days > 0.0)
        .collect();
    if valid.is_empty() {
        return 0.0;
    }
    let mean_gap =
        valid.iter().map(|i| i.gap_days).sum::<f64>() / valid.len() as f64;
    let unchanged: f64 = valid
        .iter()
        .filter(|i| !i.changed)
        .map(|i| i.gap_days)
        .sum::<f64>()
        + 0.5 * mean_gap;
    // g(Δ) = Σ_changed a/(e^{aΔ}−1) − Σ_unchanged a, decreasing in Δ,
    // +∞ at 0 and −Σ_unchanged < 0 at ∞.
    let g = |rate: f64| -> f64 {
        let term = |a: f64| {
            let d = (a * rate).exp_m1();
            if d.is_infinite() { 0.0 } else { a / d }
        };
        valid
            .iter()
            .filter(|i| i.changed)
            .map(|i| term(i.gap_days))
            .sum::<f64>()
            + 0.5 * term(mean_gap)
            - unchanged
    };
    let (mut lo, mut hi) = ((1e-12f64).ln(), (1e9f64).ln());
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if g(mid.exp()) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo < 1e-12 {
            break;
        }
    }
    (0.5 * (lo + hi)).exp()
}

/// `E[N | N ≥ 1]` for `N ~ Poisson(m)`: `m / (1 − e^{−m})`, 1 as `m → 0`.
/// Used by the online update ([`crate::update`]): crediting a changed
/// check with this many changes, and dividing total changes by total
/// exposure, has the MLE above as its fixed point (it is EM for the
/// censored likelihood).
pub fn expected_changes_given_changed(m: f64) -> f64 {
    if m.is_nan() || m <= 1e-12 {
        return 1.0;
    }
    if m > 700.0 {
        return m;
    }
    m / -(-m).exp_m1()
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};
    use rand::{Rng, SeedableRng, rngs::StdRng};

    use super::*;

    #[test]
    fn cho_edges() {
        assert_eq!(cho_garcia_molina(0, 0, 1.0), 0.0);
        assert_eq!(cho_garcia_molina(10, 0, 0.0), 0.0);
        assert_eq!(cho_garcia_molina(10, 0, 1.0), 0.0);
        let all = cho_garcia_molina(10, 10, 1.0);
        assert!(all.is_finite() && all > 2.0);
        // Changes beyond n are clamped.
        assert_eq!(cho_garcia_molina(10, 12, 1.0), all);
    }

    #[test]
    fn mle_closed_form_for_equal_gaps() {
        let (n, x, gap) = (20usize, 7usize, 0.5);
        let obs: Vec<_> = (0..n).map(|i| Interval::new(gap, i < x)).collect();
        let want = ((n as f64 + 1.0) / (n as f64 - x as f64 + 0.5)).ln() / gap;
        let got = poisson_mle(&obs);
        assert!((got / want - 1.0).abs() < 1e-9, "{got} vs {want}");
    }

    #[test]
    fn mle_edges() {
        assert_eq!(poisson_mle(&[]), 0.0);
        assert_eq!(poisson_mle(&[Interval::new(f64::NAN, true)]), 0.0);
        let all = poisson_mle(&[Interval::new(1.0, true); 5]);
        assert!(all.is_finite() && all > 1.0);
        let none = poisson_mle(&[Interval::new(1.0, false); 5]);
        assert!(none > 0.0 && none < 0.2);
    }

    #[test]
    fn expected_changes_limits() {
        assert_eq!(expected_changes_given_changed(0.0), 1.0);
        assert!((expected_changes_given_changed(1e-6) - 1.0).abs() < 1e-5);
        assert_eq!(expected_changes_given_changed(1000.0), 1000.0);
        assert!(
            (expected_changes_given_changed(1.0)
                - 1.0 / (1.0 - (-1.0f64).exp()))
            .abs()
                < 1e-12
        );
    }

    /// Simulate checks at random gaps of a Poisson process with `rate`.
    fn simulate(rate: f64, n: usize, rng: &mut StdRng) -> Vec<Interval> {
        (0..n)
            .map(|_| {
                let gap = rng.random_range(0.1..3.0);
                let p_change = 1.0 - (-rate * gap).exp();
                Interval::new(gap, rng.random::<f64>() < p_change)
            })
            .collect()
    }

    #[test]
    fn mle_beats_naive_counting() {
        let mut rng = StdRng::seed_from_u64(11);
        let obs = simulate(2.0, 3000, &mut rng);
        let naive = obs.iter().filter(|i| i.changed).count() as f64
            / obs.iter().map(|i| i.gap_days).sum::<f64>();
        let mle = poisson_mle(&obs);
        assert!((mle - 2.0).abs() < 0.15, "{mle}");
        assert!(naive < 1.0, "naive {naive} should underestimate");
    }

    #[hegel::test(test_cases = 40)]
    fn mle_recovers_a_known_rate(tc: TestCase) {
        let rate: f64 = tc.draw(gs::floats().min_value(0.05).max_value(5.0));
        let seed: u64 = tc.draw(gs::integers::<u64>());
        let obs = simulate(rate, 4000, &mut StdRng::seed_from_u64(seed));
        let mle = poisson_mle(&obs);
        assert!(
            (mle / rate - 1.0).abs() < 0.15,
            "rate {rate}, estimated {mle}"
        );
    }

    #[hegel::test(test_cases = 100)]
    fn mle_is_finite_and_positive(tc: TestCase) {
        let obs: Vec<Interval> = tc
            .draw(
                gs::vecs(gs::tuples!(
                    gs::floats::<f64>().min_value(1e-6).max_value(1e4),
                    gs::booleans()
                ))
                .max_size(30),
            )
            .into_iter()
            .map(|(g, c)| Interval::new(g, c))
            .collect();
        let mle = poisson_mle(&obs);
        if obs.is_empty() {
            assert_eq!(mle, 0.0);
        } else {
            assert!(mle.is_finite() && mle > 0.0, "{mle}");
        }
    }
}
