//! Trips from legs, and the learned round-trip discount (DECISIONS.md
//! 0042, 0043).
//!
//! Every route tracks its outbound and return legs one way, per day. A
//! trip of `n` days departing on `d` is estimated as
//! `(outbound[d] + return[d + n]) × (1 − discount)`, for every `n` in the
//! trip range (5–10 days). The discount is learned per route from real
//! round-trip cells on the same dates; once the round trip has matched the
//! sum of its legs (within ±1%) for `additive_checks_to_stop` checks in a
//! row, the route is *additive*: round-trip tracking stops and the
//! discount is 0. A later gap (a confirmation that does not match) resumes
//! it.

use std::collections::BTreeMap;

use chrono::{NaiveDate, TimeDelta};
use serde::{Deserialize, Serialize};
use tm_domain::TripRange;

use crate::Params;

/// The largest discount a route may learn. Measured discounts are 6–21%
/// (research, google-flights.md); anything beyond half is a parsing error.
pub const MAX_DISCOUNT: f64 = 0.5;

/// One estimated (or real) trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trip {
    pub departure: NaiveDate,
    pub return_date: NaiveDate,
    pub days: u16,
    /// The estimated round-trip price, minor units.
    pub price_minor: i64,
    /// `outbound + return`, before the discount.
    pub legs_sum_minor: i64,
}

/// Every trip in `range` whose two legs have a price, ordered by
/// departure, then length. `discount` is a fraction (0.1 is 10% off the
/// sum of the legs), clamped to `[0, MAX_DISCOUNT]`.
pub fn build_trips(
    outbound: &BTreeMap<NaiveDate, i64>,
    ret: &BTreeMap<NaiveDate, i64>,
    range: TripRange,
    discount: f64,
) -> Vec<Trip> {
    let factor = 1.0 - clamp_discount(discount);
    let mut out = Vec::new();
    for (&departure, &out_price) in outbound {
        for days in range.min_days..=range.max_days {
            let Some(return_date) =
                departure.checked_add_signed(TimeDelta::days(i64::from(days)))
            else {
                continue;
            };
            let Some(&ret_price) = ret.get(&return_date) else {
                continue;
            };
            let legs = out_price.saturating_add(ret_price);
            out.push(Trip {
                departure,
                return_date,
                days,
                price_minor: (legs as f64 * factor).round() as i64,
                legs_sum_minor: legs,
            });
        }
    }
    out
}

fn clamp_discount(d: f64) -> f64 {
    if d.is_finite() {
        d.clamp(0.0, MAX_DISCOUNT)
    } else {
        0.0
    }
}

/// A real round-trip cell next to the legs for the same dates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundTripPair {
    pub departure: NaiveDate,
    pub return_date: NaiveDate,
    pub round_trip_minor: i64,
    pub legs_sum_minor: i64,
    /// `1 − round_trip / legs_sum`.
    pub discount: f64,
}

/// Pair each round-trip cell (`(departure, return) → price`) with the
/// legs on the same dates. Cells without both legs are skipped.
pub fn compare_round_trips(
    round_trips: &BTreeMap<(NaiveDate, NaiveDate), i64>,
    outbound: &BTreeMap<NaiveDate, i64>,
    ret: &BTreeMap<NaiveDate, i64>,
) -> Vec<RoundTripPair> {
    round_trips
        .iter()
        .filter_map(|(&(departure, return_date), &rt)| {
            let legs = outbound
                .get(&departure)?
                .saturating_add(*ret.get(&return_date)?);
            (legs > 0 && rt > 0).then(|| RoundTripPair {
                departure,
                return_date,
                round_trip_minor: rt,
                legs_sum_minor: legs,
                discount: 1.0 - rt as f64 / legs as f64,
            })
        })
        .collect()
}

/// The median discount of `pairs`, robust to a few odd cells.
pub fn median_discount(pairs: &[RoundTripPair]) -> Option<f64> {
    let mut ds: Vec<f64> = pairs
        .iter()
        .map(|p| p.discount)
        .filter(|d| d.is_finite())
        .collect();
    if ds.is_empty() {
        return None;
    }
    ds.sort_by(f64::total_cmp);
    let n = ds.len();
    Some(if n % 2 == 1 {
        ds[n / 2]
    } else {
        0.5 * (ds[n / 2 - 1] + ds[n / 2])
    })
}

/// A route's learned round-trip discount and its additive state.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RoundTripDiscount {
    /// EWMA of `1 − round_trip / legs_sum`.
    pub ewma: f64,
    /// Observations so far.
    pub n: u32,
    /// Checks in a row whose round trip matched the legs.
    pub additive_streak: u32,
    /// Round-trip tracking has stopped for this route.
    pub additive: bool,
}

impl RoundTripDiscount {
    /// The discount to price trips with: 0 when additive or unknown.
    pub fn discount(&self) -> f64 {
        if self.additive || self.n == 0 {
            0.0
        } else {
            clamp_discount(self.ewma)
        }
    }

    /// Whether the route still needs round-trip queries.
    pub fn tracks_round_trips(&self) -> bool {
        !self.additive
    }

    /// Learn from one check: a real round-trip price and the sum of its
    /// legs on the same dates (or the per-check aggregate, see
    /// [`RoundTripDiscount::observe_pairs`]). Non-positive inputs are
    /// ignored.
    pub fn observe(
        &mut self,
        round_trip_minor: i64,
        legs_sum_minor: i64,
        params: &Params,
    ) {
        if round_trip_minor <= 0 || legs_sum_minor <= 0 {
            return;
        }
        let d = 1.0 - round_trip_minor as f64 / legs_sum_minor as f64;
        self.observe_discount(d, params);
    }

    /// Learn from one check's pairs through their median discount.
    pub fn observe_pairs(&mut self, pairs: &[RoundTripPair], params: &Params) {
        if let Some(d) = median_discount(pairs) {
            self.observe_discount(d, params);
        }
    }

    /// Learn from one check's discount.
    pub fn observe_discount(&mut self, discount: f64, params: &Params) {
        if !discount.is_finite() {
            return;
        }
        let alpha = params
            .discount_alpha
            .max(1.0 / (f64::from(self.n) + 1.0))
            .clamp(0.0, 1.0);
        self.ewma = (1.0 - alpha) * self.ewma + alpha * discount;
        self.n = self.n.saturating_add(1);
        if discount.abs() <= params.additive_tolerance {
            self.additive_streak = self.additive_streak.saturating_add(1);
            if self.additive_streak >= u32::from(params.additive_checks_to_stop)
            {
                self.additive = true;
            }
        } else {
            self.additive_streak = 0;
            self.additive = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).expect("valid date")
    }

    #[test]
    fn trips_from_legs() {
        let out: BTreeMap<_, _> = [(d(11, 1), 1000), (d(11, 2), 1100)].into();
        let ret: BTreeMap<_, _> =
            [(d(11, 6), 900), (d(11, 8), 800), (d(11, 30), 1)].into();
        let range = TripRange {
            min_days: 5,
            max_days: 7,
            round_trip_days: 7,
        };
        let trips = build_trips(&out, &ret, range, 0.1);
        let got: Vec<_> = trips
            .iter()
            .map(|t| (t.departure, t.days, t.price_minor, t.legs_sum_minor))
            .collect();
        assert_eq!(
            got,
            vec![
                (d(11, 1), 5, 1710, 1900),
                (d(11, 1), 7, 1620, 1800),
                (d(11, 2), 6, 1710, 1900),
            ]
        );
        // Discount out of range is clamped.
        assert_eq!(build_trips(&out, &ret, range, -1.0)[0].price_minor, 1900);
        assert_eq!(
            build_trips(&out, &ret, range, f64::NAN)[0].price_minor,
            1900
        );
        assert_eq!(build_trips(&out, &ret, range, 0.9)[0].price_minor, 950);
    }

    #[hegel::test(test_cases = 200)]
    fn trip_math(tc: TestCase) {
        let n_out = tc.draw(gs::integers::<usize>().max_value(20));
        let prices =
            || gs::integers::<i64>().min_value(1).max_value(10_000_000);
        let out: BTreeMap<_, _> = (0..n_out)
            .map(|i| (d(11, 1) + TimeDelta::days(i as i64), tc.draw(prices())))
            .collect();
        let ret: BTreeMap<_, _> = (0..40)
            .filter(|_| tc.draw(gs::booleans()))
            .map(|i| (d(11, 1) + TimeDelta::days(i), tc.draw(prices())))
            .collect();
        let min = tc.draw(gs::integers::<u16>().min_value(1).max_value(15));
        let max = tc.draw(gs::integers::<u16>().min_value(min).max_value(20));
        let range = TripRange {
            min_days: min,
            max_days: max,
            round_trip_days: 7,
        };
        let disc: f64 = tc.draw(gs::floats().min_value(0.0).max_value(0.5));
        let trips = build_trips(&out, &ret, range, disc);
        let mut expected = 0;
        for dep in out.keys() {
            for n in min..=max {
                if ret.contains_key(&(*dep + TimeDelta::days(i64::from(n)))) {
                    expected += 1;
                }
            }
        }
        assert_eq!(trips.len(), expected);
        for t in trips {
            assert!(t.days >= min && t.days <= max);
            assert_eq!(
                (t.return_date - t.departure).num_days(),
                i64::from(t.days)
            );
            assert_eq!(
                t.legs_sum_minor,
                out[&t.departure] + ret[&t.return_date]
            );
            let want = t.legs_sum_minor as f64 * (1.0 - disc);
            assert!((t.price_minor as f64 - want).abs() <= 0.5 + 1e-6);
            assert!(t.price_minor <= t.legs_sum_minor);
        }
    }

    #[test]
    fn compares_round_trip_cells() {
        let out: BTreeMap<_, _> = [(d(11, 1), 1000), (d(11, 2), 1000)].into();
        let ret: BTreeMap<_, _> = [(d(11, 8), 1000)].into();
        let rts: BTreeMap<_, _> =
            [((d(11, 1), d(11, 8)), 1800), ((d(11, 2), d(11, 9)), 1500)].into();
        let pairs = compare_round_trips(&rts, &out, &ret);
        assert_eq!(pairs.len(), 1);
        assert!((pairs[0].discount - 0.1).abs() < 1e-12);
        assert_eq!(median_discount(&pairs), Some(pairs[0].discount));
        assert_eq!(median_discount(&[]), None);
    }

    #[test]
    fn learns_a_discount() {
        let p = Params::default();
        let mut rt = RoundTripDiscount::default();
        assert_eq!(rt.discount(), 0.0);
        for _ in 0..30 {
            rt.observe(850, 1000, &p);
        }
        assert!((rt.discount() - 0.15).abs() < 1e-9);
        assert!(!rt.additive);
        // Garbage is ignored.
        rt.observe(0, 1000, &p);
        rt.observe(1000, -1, &p);
        assert_eq!(rt.n, 30);
    }

    #[test]
    fn additive_stop_rule_and_resume() {
        let p = Params::default(); // stop after 5
        let mut rt = RoundTripDiscount::default();
        for i in 1..=4 {
            rt.observe(1005, 1000, &p); // −0.5%: matches
            assert_eq!(rt.additive_streak, i);
            assert!(!rt.additive);
        }
        rt.observe(1000, 1000, &p);
        assert!(rt.additive);
        assert_eq!(rt.discount(), 0.0);
        assert!(!rt.tracks_round_trips());
        // A gap resumes tracking.
        rt.observe(900, 1000, &p);
        assert!(!rt.additive);
        assert_eq!(rt.additive_streak, 0);
        assert!(rt.tracks_round_trips());
        // A miss in the middle restarts the streak.
        for _ in 0..4 {
            rt.observe(1000, 1000, &p);
        }
        rt.observe(980, 1000, &p);
        rt.observe(1000, 1000, &p);
        assert_eq!(rt.additive_streak, 1);
        assert!(!rt.additive);
    }
}
