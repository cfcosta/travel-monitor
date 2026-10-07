//! Award series and the value of a mile (DECISIONS.md 0023, 0030,
//! 0083).
//!
//! An award series is one program × route × cabin × family kind, as for
//! cash (0042): each leg one way, and the round trip, which LATAM Pass
//! prices well below its two one-ways (GRU–MIA: 144,134 miles against
//! 224,676). Its price for a date (or date pair) is the cheapest award
//! the program offered (fewest miles, then lowest taxes), which on LATAM
//! Pass is the cabin's cheapest brand (LIGHT in economy), as the cash
//! series is the cheapest fare. Its baselines are
//! [`crate::Baseline`]s over miles, kept apart from cash ones: miles and
//! reais never share a baseline (0023).
//!
//! The value of a mile is measured, never set: every award carries the
//! program's own fare before taxes, and [`implied_rate`] is that fare
//! over the miles. It is not a constant: LATAM Pass prices most awards
//! at R$28.80 per 1,000 miles but silently discounts some routes (CGH →
//! SDU economy, the GRU–MIA nonstops' round-trip outbound), which show
//! as a rate above usual. A program's usual rate is the median of its recent
//! checks ([`usual_rate`]); an award far from it is flagged
//! ([`is_unusual`]). Setting an award against a cash price
//! ([`rate_against_cash`], [`cost_at`], [`buy_miles`]) is for display
//! only.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use tm_domain::{AwardPrice, Currency, FamilyId, Money, Observation, QueryId};

use crate::baseline::PricePoint;

/// A program's usual rate is the median of this many days of checks.
pub const RATE_WINDOW_DAYS: i64 = 30;

/// An award whose implied rate is this far from its program's usual
/// rate is unusual, unless the settings say otherwise (0083: 15%).
pub const UNUSUAL_RATE: f64 = 0.15;

/// Checks a program's usual rate needs before awards are flagged
/// against it.
pub const MIN_RATE_CHECKS: usize = 3;

/// Tolerance on threshold comparisons, for float rounding.
const EPSILON: f64 = 1e-9;

/// An award's implied value of a mile, in BRL per 1,000 miles: the
/// program's own fare before taxes over its miles (LATAM Pass,
/// 2026-10-07: R$1,838.82 for 63,848 miles is R$28.80). `None` without
/// miles or for a fare that is not in BRL.
pub fn implied_rate(miles: i64, fare: &Money) -> Option<f64> {
    (miles > 0 && fare.minor > 0 && fare.currency.as_str() == "BRL")
        .then(|| fare.as_f64() / miles as f64 * 1000.0)
}

/// What an award's miles are worth against a cash price for the same
/// leg and date, in BRL per 1,000 miles: the cash the miles stand in
/// for, less the taxes paid on top, over the miles. Shown next to the
/// award; never used for baselines or deals (0023). `None` when the
/// cash does not cover the taxes, or currencies differ.
pub fn rate_against_cash(award: &AwardPrice, cash: &Money) -> Option<f64> {
    if award.miles <= 0 || cash.currency != award.taxes.currency {
        return None;
    }
    let saved = cash.minor - award.taxes.minor;
    (saved > 0).then(|| saved as f64 / 100.0 / award.miles as f64 * 1000.0)
}

/// What an award costs in reais with its miles at `per_1000` (the
/// user's value of a mile, or what the program sells them at): miles ×
/// `per_1000` ÷ 1,000 + taxes, rounded to the cent. `None` when the
/// two are in different currencies or the miles are negative.
pub fn cost_at(award: &AwardPrice, per_1000: &Money) -> Option<Money> {
    if award.miles < 0 || per_1000.currency != award.taxes.currency {
        return None;
    }
    let miles = i128::from(award.miles) * i128::from(per_1000.minor);
    // Round half away from zero, to the cent.
    let part = (miles + miles.signum() * 500) / 1000;
    let minor = i64::try_from(part).ok()?.checked_add(award.taxes.minor)?;
    Some(Money::new(minor, award.taxes.currency.clone()))
}

/// An award paid with miles bought for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MilesPurchase {
    /// The miles at the program's price, plus the taxes.
    pub cost: Money,
    /// The cash fare less that cost: positive when buying miles beats
    /// paying cash, negative when it is dearer. `None` without a cash
    /// fare for the same leg and date.
    pub saving: Option<Money>,
}

/// What an award costs when its miles are bought at `price_per_1000`,
/// and how that compares with `cash`, the cash fare for the same leg
/// and date (0083). Buying miles plus taxes beats cash when the
/// program sells miles for less than the award's rate against cash.
pub fn buy_miles(
    award: &AwardPrice,
    price_per_1000: &Money,
    cash: Option<&Money>,
) -> Option<MilesPurchase> {
    let cost = cost_at(award, price_per_1000)?;
    let saving = cash
        .filter(|c| c.currency == cost.currency)
        .map(|c| Money::new(c.minor - cost.minor, c.currency.clone()));
    Some(MilesPurchase { cost, saving })
}

/// One award check's implied rate, in BRL per 1,000 miles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatePoint {
    pub observed_at: DateTime<Utc>,
    pub brl_per_1000: f64,
}

/// A program's usual rate: the median of its checks in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsualRate {
    pub median: f64,
    /// Checks in the window.
    pub n: usize,
}

/// The median rate of the checks within `window` before `now` (later
/// ones and non-finite rates are ignored). `None` without any.
pub fn usual_rate(
    points: &[RatePoint],
    now: DateTime<Utc>,
    window: TimeDelta,
) -> Option<UsualRate> {
    let since = now - window;
    let mut rates: Vec<f64> = points
        .iter()
        .filter(|p| p.observed_at <= now && p.observed_at >= since)
        .map(|p| p.brl_per_1000)
        .filter(|r| r.is_finite() && *r > 0.0)
        .collect();
    if rates.is_empty() {
        return None;
    }
    rates.sort_by(f64::total_cmp);
    let n = rates.len();
    let median = if n % 2 == 1 {
        rates[n / 2]
    } else {
        0.5 * (rates[n / 2 - 1] + rates[n / 2])
    };
    Some(UsualRate { median, n })
}

/// How far `rate` is from the usual one: `rate / usual − 1` (+0.42 is
/// 42% more reais per mile than usual).
pub fn rate_change(rate: f64, usual: &UsualRate) -> Option<f64> {
    (usual.median > 0.0 && rate.is_finite()).then(|| rate / usual.median - 1.0)
}

/// Whether `rate` is at least `threshold` away from the usual rate, in
/// either direction, once the usual rate rests on [`MIN_RATE_CHECKS`]
/// checks. A promotion shows as a rate well above usual (the miles buy
/// more); a devaluation as one well below.
pub fn is_unusual(rate: f64, usual: &UsualRate, threshold: f64) -> bool {
    usual.n >= MIN_RATE_CHECKS
        && rate_change(rate, usual)
            .is_some_and(|c| c.abs() >= threshold - EPSILON)
}

/// What an award series prices: a date for a one-way award, a date
/// pair for a round trip.
pub type AwardKey = (NaiveDate, Option<NaiveDate>);

/// The award series' price of each date (one way) or date pair (round
/// trip) among `cells`: the award with the fewest miles, then the
/// lowest taxes, then the earliest observed. Cells without miles are
/// left out.
pub fn cheapest_by_dates<'a>(
    cells: impl IntoIterator<Item = &'a Observation>,
) -> BTreeMap<AwardKey, &'a Observation> {
    let mut out: BTreeMap<AwardKey, &Observation> = BTreeMap::new();
    for c in cells {
        if c.miles.is_none_or(|m| m <= 0) {
            continue;
        }
        let key = |o: &Observation| {
            (
                o.miles.unwrap_or(i64::MAX),
                o.price.as_ref().map_or(i64::MAX, |p| p.minor),
                o.observed_at,
            )
        };
        out.entry((c.departure, c.return_date))
            .and_modify(|best| {
                if key(c) < key(best) {
                    *best = c;
                }
            })
            .or_insert(c);
    }
    out
}

/// The award series' past prices for `family` (a leg or a round trip of
/// one length): one point per check and date, at the cheapest award's
/// miles, without the checks in
/// `exclude` (the ones being judged, 0030). Feed them to
/// [`crate::baseline::compute_by_key`] for the award baselines.
pub fn award_points(
    history: &[Observation],
    family: FamilyId,
    exclude: &HashSet<QueryId>,
) -> Vec<PricePoint> {
    let mut by_query: BTreeMap<QueryId, Vec<&Observation>> = BTreeMap::new();
    for o in history {
        if o.family == family && !exclude.contains(&o.query) {
            by_query.entry(o.query).or_default().push(o);
        }
    }
    let mut out: Vec<PricePoint> = by_query
        .values()
        .flat_map(|cells| {
            cheapest_by_dates(cells.iter().copied())
                .into_values()
                .filter_map(|o| {
                    Some(PricePoint {
                        observed_at: o.observed_at,
                        departure: o.departure,
                        price_minor: o.miles?,
                    })
                })
        })
        .collect();
    out.sort_by_key(|p| (p.observed_at, p.departure));
    out
}

/// BRL per 1,000 miles as money, rounded to the cent (R$28.80 is 2880).
pub fn rate_money(brl_per_1000: f64) -> Option<Money> {
    (brl_per_1000.is_finite() && brl_per_1000 > 0.0).then(|| {
        Money::new((brl_per_1000 * 100.0).round() as i64, Currency::brl())
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use hegel::{TestCase, generators as gs};
    use tm_domain::AwardDetail;

    use super::*;

    fn brl(minor: i64) -> Money {
        Money::new(minor, Currency::brl())
    }

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0)
            .single()
            .unwrap_or_default()
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap_or_default()
    }

    fn award_cell(
        query: i64,
        day: u32,
        dep: NaiveDate,
        miles: i64,
        taxes: i64,
    ) -> Observation {
        Observation {
            id: None,
            query: QueryId(query),
            family: FamilyId(1),
            observed_at: at(day),
            departure: dep,
            return_date: None,
            price: Some(brl(taxes)),
            miles: Some(miles),
            carrier: None,
            stops: None,
            duration_minutes: None,
            seller: None,
            flights: None,
            award: Some(AwardDetail {
                fare: None,
                brand: None,
                fare_basis: None,
                seats: None,
            }),
        }
    }

    /// The GRU → LIS fixture: LA8146 LIGHT, 63,848 miles + R$68.61
    /// against R$1,838.82 before taxes and R$1,907.43 in cash.
    #[test]
    fn the_fixture_is_worth_28_80_per_1000() {
        let rate = implied_rate(63_848, &brl(183_882)).expect("a rate");
        assert!((rate - 28.80).abs() < 0.005, "{rate}");
        let award = AwardPrice::new(63_848, brl(6861));
        let vs_cash = rate_against_cash(&award, &brl(190_743)).expect("rate");
        assert!((vs_cash - 28.80).abs() < 0.005, "{vs_cash}");
        // At the user's R$20 per 1,000: 63,848 × 0.02 + 68.61 = 1,345.57.
        assert_eq!(cost_at(&award, &brl(2000)), Some(brl(134_557)));
        // Bought at R$17.50 per 1,000, the trip costs R$1,185.95 and
        // saves R$721.48 on the cash fare.
        let bought =
            buy_miles(&award, &brl(1750), Some(&brl(190_743))).expect("buys");
        assert_eq!(bought.cost, brl(118_595));
        assert_eq!(bought.saving, Some(brl(72_148)));
        // Bought dearer than the award's rate, cash wins.
        let dear =
            buy_miles(&award, &brl(3500), Some(&brl(190_743))).expect("buys");
        assert!(dear.saving.is_some_and(|s| s.minor < 0));
        assert_eq!(
            buy_miles(&award, &brl(1750), None).map(|b| b.saving),
            Some(None)
        );
    }

    #[test]
    fn rates_need_reais_and_miles() {
        assert_eq!(implied_rate(0, &brl(100)), None);
        assert_eq!(
            implied_rate(1000, &Money::from_whole(10, Currency::new("USD"))),
            None
        );
        let award = AwardPrice::new(10_000, brl(50_000));
        assert_eq!(rate_against_cash(&award, &brl(40_000)), None);
        assert_eq!(
            rate_against_cash(
                &award,
                &Money::from_whole(900, Currency::new("USD"))
            ),
            None
        );
        assert_eq!(
            cost_at(&award, &Money::from_whole(1, Currency::new("USD"))),
            None
        );
        assert_eq!(rate_money(28.8), Some(brl(2880)));
        assert_eq!(rate_money(f64::NAN), None);
    }

    #[test]
    fn the_usual_rate_is_the_recent_median() {
        let points: Vec<RatePoint> =
            [(1, 28.8), (2, 28.7), (3, 41.0), (20, 28.9), (30, 99.0)]
                .into_iter()
                .map(|(d, r)| RatePoint {
                    observed_at: at(d),
                    brl_per_1000: r,
                })
                .collect();
        // Oct 30 is after `now`: left out. The window covers Oct 1–21.
        let usual =
            usual_rate(&points, at(21), TimeDelta::days(RATE_WINDOW_DAYS))
                .expect("usual");
        assert_eq!(usual.n, 4);
        assert!((usual.median - 28.85).abs() < 1e-9);
        assert!(is_unusual(41.0, &usual, UNUSUAL_RATE));
        assert!(is_unusual(20.0, &usual, UNUSUAL_RATE));
        assert!(!is_unusual(29.5, &usual, UNUSUAL_RATE));
        let thin = UsualRate { median: 28.8, n: 2 };
        assert!(!is_unusual(41.0, &thin, UNUSUAL_RATE));
        assert!(usual_rate(&points, at(21), TimeDelta::days(0)).is_none());
    }

    #[test]
    fn the_series_takes_the_cheapest_award_of_each_check() {
        let history = vec![
            award_cell(1, 1, date(10), 71_786, 6861),
            award_cell(1, 1, date(10), 63_848, 14_749),
            award_cell(1, 1, date(10), 63_848, 6861),
            award_cell(1, 1, date(11), 79_896, 28_722),
            award_cell(2, 2, date(10), 60_000, 6861),
            Observation {
                return_date: Some(date(17)),
                ..award_cell(2, 2, date(10), 1, 1)
            },
        ];
        let picked = cheapest_by_dates(&history[..4]);
        let nov10 = picked.get(&(date(10), None)).expect("Nov 10");
        assert_eq!(
            (nov10.miles, nov10.price.as_ref().map(|p| p.minor)),
            (Some(63_848), Some(6861))
        );
        let points = award_points(&history, FamilyId(1), &HashSet::new());
        let miles: Vec<i64> = points.iter().map(|p| p.price_minor).collect();
        // The round trip of the second check is its own price.
        assert_eq!(miles, vec![63_848, 79_896, 60_000, 1]);
        let both = cheapest_by_dates(&history);
        assert!(both.contains_key(&(date(10), Some(date(17)))));
        let without = award_points(&history, FamilyId(1), &[QueryId(2)].into());
        assert_eq!(without.len(), 2);
        assert!(
            award_points(&history, FamilyId(9), &HashSet::new()).is_empty()
        );
    }

    #[hegel::test(test_cases = 300)]
    fn buying_miles_costs_the_miles_plus_the_taxes(tc: TestCase) {
        let miles: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(2_000_000));
        let taxes: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(500_000));
        let price: i64 =
            tc.draw(gs::integers::<i64>().min_value(1).max_value(20_000));
        let cash: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(10_000_000));
        let award = AwardPrice::new(miles, brl(taxes));
        let bought =
            buy_miles(&award, &brl(price), Some(&brl(cash))).expect("BRL");
        // Within half a cent of the exact cost.
        let exact = miles as f64 * price as f64 / 1000.0 + taxes as f64;
        assert!(
            (bought.cost.minor as f64 - exact).abs() <= 0.5,
            "{bought:?} {exact}"
        );
        assert_eq!(bought.saving, Some(brl(cash - bought.cost.minor)));
        // A dearer price never makes it cheaper.
        let dearer = cost_at(&award, &brl(price + 1)).expect("BRL");
        assert!(dearer.minor >= bought.cost.minor);
        // Bought below the award's rate against cash, it saves money.
        if let Some(rate) = rate_against_cash(&award, &brl(cash))
            && (price as f64 / 100.0) < rate - 0.01
        {
            // Less the cost's rounding, half a cent.
            assert!(
                bought.saving.as_ref().is_some_and(|s| s.minor >= -1),
                "{bought:?} {rate}"
            );
        }
    }

    #[hegel::test(test_cases = 300)]
    fn the_implied_rate_gives_back_the_fare(tc: TestCase) {
        let miles: i64 =
            tc.draw(gs::integers::<i64>().min_value(1).max_value(2_000_000));
        let fare: i64 =
            tc.draw(gs::integers::<i64>().min_value(1).max_value(50_000_000));
        let rate = implied_rate(miles, &brl(fare)).expect("a rate");
        let back = cost_at(
            &AwardPrice::new(miles, brl(0)),
            &brl((rate * 100.0).round() as i64),
        )
        .expect("BRL");
        // Rounding the rate to the cent moves the fare by at most half a
        // cent per 1,000 miles, plus the cost's own rounding.
        let slack = miles as f64 / 2000.0 + 1.0;
        assert!(
            (back.minor - fare).abs() as f64 <= slack,
            "{back:?} {fare} {rate}"
        );
    }

    #[hegel::test(test_cases = 200)]
    fn the_usual_rate_lies_within_its_checks(tc: TestCase) {
        let rates: Vec<(i64, f64)> = tc.draw(
            gs::vecs(gs::tuples!(
                gs::integers::<i64>().min_value(0).max_value(60),
                gs::floats::<f64>().min_value(1.0).max_value(100.0)
            ))
            .max_size(40),
        );
        let now = at(1) + TimeDelta::days(60);
        let points: Vec<RatePoint> = rates
            .iter()
            .map(|&(d, r)| RatePoint {
                observed_at: at(1) + TimeDelta::days(d),
                brl_per_1000: r,
            })
            .collect();
        let window = TimeDelta::days(RATE_WINDOW_DAYS);
        let inside: Vec<f64> = points
            .iter()
            .filter(|p| p.observed_at >= now - window)
            .map(|p| p.brl_per_1000)
            .collect();
        match usual_rate(&points, now, window) {
            None => assert!(inside.is_empty()),
            Some(u) => {
                assert_eq!(u.n, inside.len());
                let lo = inside.iter().copied().fold(f64::INFINITY, f64::min);
                let hi =
                    inside.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                assert!(u.median >= lo && u.median <= hi);
                assert!(!is_unusual(u.median, &u, UNUSUAL_RATE));
            }
        }
    }

    #[hegel::test(test_cases = 200)]
    fn the_series_price_is_never_above_an_award_of_its_date(tc: TestCase) {
        let cells: Vec<(u32, i64, i64)> = tc.draw(
            gs::vecs(gs::tuples!(
                gs::integers::<u32>().min_value(1).max_value(5),
                gs::integers::<i64>().min_value(1).max_value(1_000_000),
                gs::integers::<i64>().min_value(0).max_value(100_000)
            ))
            .max_size(30),
        );
        let obs: Vec<Observation> = cells
            .iter()
            .map(|&(d, m, t)| award_cell(1, 1, date(d), m, t))
            .collect();
        let picked = cheapest_by_dates(&obs);
        for o in &obs {
            let best = picked
                .get(&(o.departure, None))
                .expect("every date is priced");
            assert!(best.miles <= o.miles);
        }
        assert_eq!(
            picked.len(),
            obs.iter()
                .map(|o| o.departure)
                .collect::<HashSet<_>>()
                .len()
        );
    }
}
