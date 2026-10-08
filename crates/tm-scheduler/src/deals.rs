//! The deal rule (DECISIONS.md 0018, 0030).
//!
//! A fare is a deal candidate, against its bucket's [`Baseline`], when the
//! bucket has at least `min_history_weeks` of observations and either:
//!
//! - **below the median:** `change = price/median − 1 ≤ −deal_threshold`
//!   (20%) **and**, when the spread is not zero, the robust z-score
//!   `(median − price)/(1.4826 · MAD) ≥ deal_min_z` (2). 1.4826 · MAD
//!   estimates the standard deviation of normal data; on a route whose
//!   prices swing widely, 20% under the median is not unusual and is not
//!   flagged. A zero MAD (more than half the prices identical) leaves the
//!   flat threshold alone.
//! - **all-time low:** strictly lower than every earlier price in the
//!   bucket. The baseline must therefore be computed from history that
//!   excludes the fare being evaluated.
//!
//! A favorite destination's series are judged with
//! [`Params::for_deals`]: 10% under the median instead of 20% (0094),
//! the robust-z test unchanged.
//!
//! When both hold the reason is [`DealReason::AllTimeLow`]. `change` is
//! always relative to the median. Candidates are confirmed by a second
//! query before any alert (0031); that is the daemon's job.

use tm_domain::DealReason;

use crate::{Params, baseline::Baseline};

/// Scales a MAD to a standard deviation under normality.
pub const MAD_TO_SD: f64 = 1.4826;

/// Tolerance on the threshold comparison, for float rounding.
pub const CHANGE_EPSILON: f64 = 1e-9;

/// The robust z-score of `price` under `baseline`: how many robust
/// standard deviations it is *below* the median. `None` when MAD is 0.
pub fn robust_z(price_minor: i64, baseline: &Baseline) -> Option<f64> {
    (baseline.mad > 0.0).then(|| {
        (baseline.median - price_minor as f64) / (MAD_TO_SD * baseline.mad)
    })
}

/// Whether `price_minor` is a deal, why, and its change against the
/// median (`−0.25` is 25% under).
pub fn evaluate(
    price_minor: i64,
    baseline: &Baseline,
    params: &Params,
) -> Option<(DealReason, f64)> {
    // Written so a NaN in the baseline means "no deal".
    let enough_history = baseline.weeks_of_history >= params.min_history_weeks;
    let has_median = baseline.median.is_finite() && baseline.median > 0.0;
    if !enough_history || !has_median || price_minor <= 0 {
        return None;
    }
    let change = price_minor as f64 / baseline.median - 1.0;
    if price_minor < baseline.all_time_low {
        return Some((DealReason::AllTimeLow, change));
    }
    // The epsilon keeps 800/1000 − 1 = −0.19999… a 20% drop.
    let far_enough = change <= -params.deal_threshold + CHANGE_EPSILON;
    let unusual =
        robust_z(price_minor, baseline).is_none_or(|z| z >= params.deal_min_z);
    (far_enough && unusual).then_some((DealReason::BelowMedian, change))
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    fn base(median: f64, mad: f64, weeks: f64, low: i64) -> Baseline {
        Baseline {
            median,
            mad,
            n: 30,
            weeks_of_history: weeks,
            all_time_low: low,
        }
    }

    #[test]
    fn needs_history() {
        let p = Params::default();
        assert_eq!(evaluate(100, &base(1000.0, 10.0, 3.9, 500), &p), None);
        assert!(evaluate(100, &base(1000.0, 10.0, 4.0, 500), &p).is_some());
    }

    #[test]
    fn below_median_needs_threshold_and_z() {
        let p = Params::default();
        // 20% under, tight spread: deal.
        let tight = base(1000.0, 20.0, 8.0, 700);
        let (reason, change) = evaluate(800, &tight, &p).expect("a deal");
        assert_eq!(reason, DealReason::BelowMedian);
        assert!((change + 0.2).abs() < 1e-12);
        // 19.9% under: not.
        assert_eq!(evaluate(801, &tight, &p), None);
        // 25% under, wide spread (z = 250/(1.4826·100) ≈ 1.69): not.
        let wide = base(1000.0, 100.0, 8.0, 500);
        assert_eq!(evaluate(750, &wide, &p), None);
        // 30% under the same spread (z ≈ 2.02): deal.
        assert_eq!(
            evaluate(700, &wide, &p).map(|d| d.0),
            Some(DealReason::BelowMedian)
        );
        // Zero spread: the flat threshold alone.
        let flat = base(1000.0, 0.0, 8.0, 700);
        assert!(evaluate(800, &flat, &p).is_some());
        assert_eq!(robust_z(800, &flat), None);
    }

    /// A favorite's drop needs only 10% (0094); the spread still has
    /// to make it unusual.
    #[test]
    fn favorites_are_deals_at_a_smaller_drop() {
        let p = Params::default();
        let fav = p.for_deals(true);
        let tight = base(1000.0, 20.0, 8.0, 700);
        // 12% under: a deal for a favorite only.
        assert_eq!(evaluate(880, &tight, &p), None);
        let (reason, change) = evaluate(880, &tight, &fav).expect("a deal");
        assert_eq!(reason, DealReason::BelowMedian);
        assert!((change + 0.12).abs() < 1e-12);
        // 10% exactly is enough, 9.9% is not.
        assert!(evaluate(900, &tight, &fav).is_some());
        assert_eq!(evaluate(901, &tight, &fav), None);
        // A wide spread still rules out a 15% drop (z ≈ 1.01).
        let wide = base(1000.0, 100.0, 8.0, 500);
        assert_eq!(evaluate(850, &wide, &fav), None);
        // And too little history is too little for favorites too.
        assert_eq!(evaluate(850, &base(1000.0, 20.0, 3.0, 500), &fav), None);
    }

    #[test]
    fn all_time_low_wins() {
        let p = Params::default();
        let b = base(1000.0, 300.0, 8.0, 960);
        let (reason, change) = evaluate(950, &b, &p).expect("a deal");
        assert_eq!(reason, DealReason::AllTimeLow);
        assert!((change + 0.05).abs() < 1e-12);
        // Equal to the low is not a new low.
        assert_eq!(evaluate(960, &b, &p), None);
    }

    #[test]
    fn degenerate_baselines() {
        let p = Params::default();
        assert_eq!(evaluate(1, &base(0.0, 0.0, 8.0, 5), &p), None);
        assert_eq!(evaluate(0, &base(1000.0, 0.0, 8.0, 5), &p), None);
        assert_eq!(evaluate(1, &base(f64::NAN, 0.0, 8.0, 5), &p), None);
        assert_eq!(evaluate(1, &base(1000.0, 0.0, f64::NAN, 5), &p), None);
    }

    #[hegel::test(test_cases = 300)]
    fn a_deal_is_never_above_the_median_unless_a_new_low(tc: TestCase) {
        let price: i64 =
            tc.draw(gs::integers::<i64>().min_value(1).max_value(1_000_000));
        let median: f64 =
            tc.draw(gs::floats().min_value(1.0).max_value(1_000_000.0));
        let mad: f64 =
            tc.draw(gs::floats().min_value(0.0).max_value(500_000.0));
        let low: i64 =
            tc.draw(gs::integers::<i64>().min_value(1).max_value(1_000_000));
        let b = base(median, mad, 10.0, low);
        let p = Params::default();
        match evaluate(price, &b, &p) {
            Some((DealReason::BelowMedian, change)) => {
                assert!(change <= -p.deal_threshold + CHANGE_EPSILON);
                assert!(price >= low);
            }
            Some((DealReason::AllTimeLow, _)) => assert!(price < low),
            None => assert!(price >= low),
        }
        // A favorite's lower threshold keeps every deal the global
        // threshold finds (0094).
        if evaluate(price, &b, &p).is_some() {
            assert!(evaluate(price, &b, &p.for_deals(true)).is_some());
        }
        // Lowering the price never turns a deal into a non-deal.
        if evaluate(price, &b, &p).is_some() && price > 1 {
            assert!(evaluate(price - 1, &b, &p).is_some());
        }
    }
}
