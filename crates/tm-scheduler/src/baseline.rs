//! Deal baselines per days-to-departure bucket and season (DECISIONS.md
//! 0030).
//!
//! Fares rise steeply in the last 21 days and barely move beyond about 90,
//! so a fare is compared only with fares the same number of days out:
//! buckets with edges at 3, 7, 14, 21, 45, 90 and 180 days. Departures
//! near Brazilian holidays (Póvoa & Oliveira: +12% on holiday eves) and in
//! peak season are kept apart from regular ones.
//!
//! The caller groups by route × cabin × trip length × point of sale (and
//! leg or round trip, 0042); this module groups by bucket and season and
//! computes the median and median absolute deviation over the last
//! 60 days.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// Bucket edges in days to departure.
pub const BUCKET_EDGES: [i64; 7] = [3, 7, 14, 21, 45, 90, 180];
/// A departure up to this many days before a holiday (and on it) is a
/// holiday departure: it covers the eve and the usual bridge day.
pub const HOLIDAY_LEAD_DAYS: i64 = 3;

/// Days to departure, bucketed by [`BUCKET_EDGES`].
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DtdBucket {
    /// 0–2 days.
    D0,
    /// 3–6.
    D3,
    /// 7–13.
    D7,
    /// 14–20.
    D14,
    /// 21–44.
    D21,
    /// 45–89.
    D45,
    /// 90–179.
    D90,
    /// 180 and more.
    D180,
}

impl DtdBucket {
    pub const ALL: [DtdBucket; 8] = [
        DtdBucket::D0,
        DtdBucket::D3,
        DtdBucket::D7,
        DtdBucket::D14,
        DtdBucket::D21,
        DtdBucket::D45,
        DtdBucket::D90,
        DtdBucket::D180,
    ];

    /// The bucket for `days` to departure; `None` once departed.
    pub fn of(days: i64) -> Option<Self> {
        if days < 0 {
            return None;
        }
        let i = BUCKET_EDGES.iter().take_while(|&&e| days >= e).count();
        Some(Self::ALL[i])
    }

    /// The bucket's day range, `[lower, upper)`; `upper` is `None` for the
    /// last.
    pub fn range(self) -> (i64, Option<i64>) {
        let i = self as usize;
        let lower = if i == 0 { 0 } else { BUCKET_EDGES[i - 1] };
        (lower, BUCKET_EDGES.get(i).copied())
    }
}

/// What kind of departure date it is.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    Regular,
    /// On or up to [`HOLIDAY_LEAD_DAYS`] before a Brazilian national
    /// holiday, or within carnival (Saturday to Ash Wednesday).
    Holiday,
    /// Brazilian school holidays: 20 December to 31 January, and July.
    /// Takes precedence over [`Season::Holiday`].
    Peak,
}

impl Season {
    pub fn of(departure: NaiveDate) -> Self {
        if is_peak(departure) {
            Season::Peak
        } else if is_holiday_departure(departure) {
            Season::Holiday
        } else {
            Season::Regular
        }
    }
}

/// Late December to January, and July.
pub fn is_peak(date: NaiveDate) -> bool {
    matches!((date.month(), date.day()), (12, 20..) | (1, _) | (7, _))
}

/// Easter Sunday (anonymous Gregorian algorithm).
pub fn easter(year: i32) -> NaiveDate {
    let a = year.rem_euclid(19);
    let b = year.div_euclid(100);
    let c = year.rem_euclid(100);
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15).rem_euclid(30);
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k).rem_euclid(7);
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32)
        .unwrap_or(NaiveDate::MIN)
}

/// A named holiday period, inclusive on both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holiday {
    pub name: &'static str,
    pub start: NaiveDate,
    pub end: NaiveDate,
}

/// Brazil's national holidays for `year`, plus carnival (Saturday to Ash
/// Wednesday), Good Friday and Corpus Christi, which are observed almost
/// everywhere. Sorted by start date.
pub fn holidays(year: i32) -> Vec<Holiday> {
    let date =
        |m, d| NaiveDate::from_ymd_opt(year, m, d).unwrap_or(NaiveDate::MIN);
    let day = |name, d: NaiveDate| Holiday {
        name,
        start: d,
        end: d,
    };
    let e = easter(year);
    let mut out = vec![
        day("Confraternização Universal", date(1, 1)),
        Holiday {
            name: "Carnaval",
            start: e - TimeDelta::days(50),
            end: e - TimeDelta::days(46),
        },
        day("Sexta-feira Santa", e - TimeDelta::days(2)),
        day("Tiradentes", date(4, 21)),
        day("Dia do Trabalho", date(5, 1)),
        day("Corpus Christi", e + TimeDelta::days(60)),
        day("Independência", date(9, 7)),
        day("Nossa Senhora Aparecida", date(10, 12)),
        day("Finados", date(11, 2)),
        day("Proclamação da República", date(11, 15)),
        day("Consciência Negra", date(11, 20)),
        day("Natal", date(12, 25)),
    ];
    out.sort_by_key(|h| h.start);
    out
}

/// Whether a departure falls on, or up to [`HOLIDAY_LEAD_DAYS`] before, a
/// holiday period.
pub fn is_holiday_departure(departure: NaiveDate) -> bool {
    let year = departure.year();
    [year, year + 1].into_iter().flat_map(holidays).any(|h| {
        departure >= h.start - TimeDelta::days(HOLIDAY_LEAD_DAYS)
            && departure <= h.end
    })
}

/// Which baseline a fare belongs to.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
pub struct BaselineKey {
    pub bucket: DtdBucket,
    pub season: Season,
}

impl BaselineKey {
    /// The key of a fare observed at `observed_at` for `departure`;
    /// `None` once departed.
    pub fn of(
        observed_at: DateTime<Utc>,
        departure: NaiveDate,
    ) -> Option<Self> {
        let days = (departure - observed_at.date_naive()).num_days();
        Some(Self {
            bucket: DtdBucket::of(days)?,
            season: Season::of(departure),
        })
    }
}

/// One observed price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricePoint {
    pub observed_at: DateTime<Utc>,
    pub departure: NaiveDate,
    pub price_minor: i64,
}

impl PricePoint {
    pub fn key(&self) -> Option<BaselineKey> {
        BaselineKey::of(self.observed_at, self.departure)
    }
}

/// A bucket's reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// Median price over the window, minor units.
    pub median: f64,
    /// Median absolute deviation from the median over the window.
    pub mad: f64,
    /// Prices in the window.
    pub n: usize,
    /// Weeks from the earliest observation to now.
    pub weeks_of_history: f64,
    /// The lowest price ever observed, window or not.
    pub all_time_low: i64,
}

impl Baseline {
    /// The baseline of `points` (one bucket's prices, observed before
    /// `now`) over `window` back from `now`. Points after `now` are
    /// ignored. `None` when no point falls in the window.
    pub fn compute(
        points: &[PricePoint],
        now: DateTime<Utc>,
        window: TimeDelta,
    ) -> Option<Self> {
        let past: Vec<&PricePoint> =
            points.iter().filter(|p| p.observed_at <= now).collect();
        let since = now - window;
        let mut recent: Vec<f64> = past
            .iter()
            .filter(|p| p.observed_at >= since)
            .map(|p| p.price_minor as f64)
            .collect();
        let median = median_of(&mut recent)?;
        let mut deviations: Vec<f64> =
            recent.iter().map(|x| (x - median).abs()).collect();
        let mad = median_of(&mut deviations).unwrap_or(0.0);
        let earliest = past.iter().map(|p| p.observed_at).min()?;
        let all_time_low = past.iter().map(|p| p.price_minor).min()?;
        Some(Self {
            median,
            mad,
            n: recent.len(),
            weeks_of_history: crate::days_between(earliest, now) / 7.0,
            all_time_low,
        })
    }
}

/// Baselines for every bucket and season present in `points`.
pub fn compute_by_key(
    points: &[PricePoint],
    now: DateTime<Utc>,
    window: TimeDelta,
) -> BTreeMap<BaselineKey, Baseline> {
    let mut groups: BTreeMap<BaselineKey, Vec<PricePoint>> = BTreeMap::new();
    for p in points {
        if let Some(key) = p.key() {
            groups.entry(key).or_default().push(*p);
        }
    }
    groups
        .into_iter()
        .filter_map(|(k, ps)| {
            Baseline::compute(&ps, now, window).map(|b| (k, b))
        })
        .collect()
}

fn median_of(xs: &mut [f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    let n = xs.len();
    Some(if n % 2 == 1 {
        xs[n / 2]
    } else {
        0.5 * (xs[n / 2 - 1] + xs[n / 2])
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use hegel::{TestCase, generators as gs};

    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("valid date")
    }

    #[test]
    fn buckets() {
        let cases = [
            (-1, None),
            (0, Some(DtdBucket::D0)),
            (2, Some(DtdBucket::D0)),
            (3, Some(DtdBucket::D3)),
            (6, Some(DtdBucket::D3)),
            (7, Some(DtdBucket::D7)),
            (14, Some(DtdBucket::D14)),
            (20, Some(DtdBucket::D14)),
            (21, Some(DtdBucket::D21)),
            (45, Some(DtdBucket::D45)),
            (89, Some(DtdBucket::D45)),
            (90, Some(DtdBucket::D90)),
            (179, Some(DtdBucket::D90)),
            (180, Some(DtdBucket::D180)),
            (10_000, Some(DtdBucket::D180)),
        ];
        for (days, want) in cases {
            assert_eq!(DtdBucket::of(days), want, "{days}");
        }
        assert_eq!(DtdBucket::D0.range(), (0, Some(3)));
        assert_eq!(DtdBucket::D180.range(), (180, None));
    }

    #[hegel::test(test_cases = 200)]
    fn bucket_contains_its_days(tc: TestCase) {
        let days: i64 =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(5000));
        let b = DtdBucket::of(days).expect("not departed");
        let (lo, hi) = b.range();
        assert!(days >= lo && hi.is_none_or(|h| days < h));
    }

    #[test]
    fn easter_dates() {
        assert_eq!(easter(2024), d(2024, 3, 31));
        assert_eq!(easter(2025), d(2025, 4, 20));
        assert_eq!(easter(2026), d(2026, 4, 5));
        assert_eq!(easter(2027), d(2027, 3, 28));
    }

    #[test]
    fn holidays_2026_and_2027() {
        let h26 = holidays(2026);
        let find = |hs: &[Holiday], name: &str| {
            *hs.iter().find(|h| h.name == name).expect("holiday listed")
        };
        let carnival = find(&h26, "Carnaval");
        assert_eq!(
            (carnival.start, carnival.end),
            (d(2026, 2, 14), d(2026, 2, 18))
        );
        assert_eq!(find(&h26, "Sexta-feira Santa").start, d(2026, 4, 3));
        assert_eq!(find(&h26, "Corpus Christi").start, d(2026, 6, 4));
        assert_eq!(find(&h26, "Consciência Negra").start, d(2026, 11, 20));
        let h27 = holidays(2027);
        let carnival = find(&h27, "Carnaval");
        assert_eq!(
            (carnival.start, carnival.end),
            (d(2027, 2, 6), d(2027, 2, 10))
        );
        assert_eq!(find(&h27, "Sexta-feira Santa").start, d(2027, 3, 26));
        assert_eq!(find(&h27, "Corpus Christi").start, d(2027, 5, 27));
        assert_eq!(h27.len(), 12);
    }

    #[test]
    fn seasons() {
        assert_eq!(Season::of(d(2026, 2, 16)), Season::Holiday); // carnival Monday
        assert_eq!(Season::of(d(2026, 2, 11)), Season::Holiday); // eve window
        assert_eq!(Season::of(d(2026, 2, 10)), Season::Regular);
        assert_eq!(Season::of(d(2026, 2, 19)), Season::Regular);
        assert_eq!(Season::of(d(2026, 11, 13)), Season::Holiday); // before 15th
        assert_eq!(Season::of(d(2026, 12, 24)), Season::Peak);
        assert_eq!(Season::of(d(2027, 1, 1)), Season::Peak);
        assert_eq!(Season::of(d(2026, 7, 15)), Season::Peak);
        assert_eq!(Season::of(d(2026, 12, 19)), Season::Regular);
        assert_eq!(Season::of(d(2026, 8, 18)), Season::Regular);
        assert_eq!(Season::of(d(2026, 6, 1)), Season::Holiday); // Corpus Christi lead
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0)
            .single()
            .expect("valid instant")
    }

    fn point(days_ago: i64, price: i64) -> PricePoint {
        PricePoint {
            observed_at: now() - TimeDelta::days(days_ago),
            departure: d(2027, 3, 2),
            price_minor: price,
        }
    }

    #[test]
    fn median_mad_and_history() {
        let pts = vec![
            point(100, 500), // outside the window, sets history and low
            point(50, 1000),
            point(40, 1100),
            point(30, 1200),
            point(20, 1300),
            point(-1, 1), // in the future: ignored
        ];
        let b = Baseline::compute(&pts, now(), TimeDelta::days(60))
            .expect("points in window");
        assert_eq!(b.n, 4);
        assert_eq!(b.median, 1150.0);
        assert_eq!(b.mad, 100.0);
        assert_eq!(b.all_time_low, 500);
        assert!((b.weeks_of_history - 100.0 / 7.0).abs() < 1e-9);
        assert!(
            Baseline::compute(&[point(100, 5)], now(), TimeDelta::days(60))
                .is_none()
        );
        assert!(Baseline::compute(&[], now(), TimeDelta::days(60)).is_none());
    }

    #[test]
    fn groups_by_bucket_and_season() {
        let mk = |dep: NaiveDate, price| PricePoint {
            observed_at: now(),
            departure: dep,
            price_minor: price,
        };
        let pts = vec![
            mk(d(2026, 10, 8), 900),   // 2 days, regular
            mk(d(2026, 10, 9), 1000),  // 3 days, eve of 12 Oct → holiday
            mk(d(2026, 11, 25), 800),  // 50 days, regular
            mk(d(2026, 12, 26), 3000), // 81 days, peak
            mk(d(2026, 10, 1), 1),     // departed
        ];
        let by = compute_by_key(&pts, now(), TimeDelta::days(60));
        let keys: Vec<_> = by.keys().copied().collect();
        assert_eq!(
            keys,
            vec![
                BaselineKey {
                    bucket: DtdBucket::D0,
                    season: Season::Regular
                },
                BaselineKey {
                    bucket: DtdBucket::D3,
                    season: Season::Holiday
                },
                BaselineKey {
                    bucket: DtdBucket::D45,
                    season: Season::Regular
                },
                BaselineKey {
                    bucket: DtdBucket::D45,
                    season: Season::Peak
                },
            ]
        );
    }

    #[hegel::test(test_cases = 200)]
    fn median_and_low_are_bounded(tc: TestCase) {
        let prices: Vec<i64> = tc.draw(
            gs::vecs(gs::integers::<i64>().min_value(1).max_value(10_000_000))
                .min_size(1)
                .max_size(50),
        );
        let pts: Vec<_> = prices
            .iter()
            .enumerate()
            .map(|(i, &p)| point(i as i64 % 60, p))
            .collect();
        let b = Baseline::compute(&pts, now(), TimeDelta::days(60))
            .expect("points in window");
        let lo = *prices.iter().min().expect("non-empty");
        let hi = *prices.iter().max().expect("non-empty");
        assert!(b.median >= lo as f64 && b.median <= hi as f64);
        assert!(b.mad >= 0.0 && b.mad <= (hi - lo) as f64);
        assert_eq!(b.all_time_low, lo);
        assert_eq!(b.n, prices.len());
    }
}
