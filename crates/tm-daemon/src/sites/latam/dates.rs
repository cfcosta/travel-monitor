//! Which departures a LATAM check reads (DECISIONS.md 0082, 0084, 0085).
//! Pure.
//!
//! There is no award calendar: every award is a search under the user's
//! own account, one page load, or one click on the results page's date
//! strip (seven days around the searched one, 0084). So a check reads
//! one date, its *anchor*, and may click up to [`STRIP_CLICKS`] other
//! days of the strip, each counted and spaced like a page load.
//!
//! - **Cheap** (the default, and cash checks): the anchor is the
//!   departure whose cash fare is lowest for the leg (Google Flights'
//!   legs in the database, LATAM's own cash when present, LATAM's cached
//!   calendar from an earlier check), among those this family has not
//!   read in [`READ_AGAIN_HOURS`]. Strip days are clicked when their
//!   cash is low, within [`LOW_CASH`] of the leg's cheapest: where an
//!   award deal would be, if its bucket is open (latam-pass.md,
//!   "Measuring cash against miles").
//! - **Spot**: one date a week per route ([`SPOT_DAYS`]), rotating over
//!   a weekly grid of the horizon by whichever was read longest ago, so
//!   the award series has a baseline and the silent discounts that cash
//!   does not show are caught. It clicks at most [`SPOT_CLICKS`] of the
//!   nearest days.
//! - **Every**: every departure in range, the one read longest ago
//!   first, and the strip's other days around it.
//!
//! The cash calendar is only ever a ranking: it is cached and a few
//! percent high, so it is never recorded (0053).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Days, NaiveDate, TimeDelta, Utc};
use tm_domain::AwardDates;

/// The most strip days one check clicks (0084).
pub const STRIP_CLICKS: u32 = 6;
/// The strip shows the searched day and this many on each side
/// (latam-pass.md, "Measuring cash against miles" §4).
pub const STRIP_REACH: u64 = 3;
/// A departure the family read this recently is not chosen again by
/// the schedule.
pub const READ_AGAIN_HOURS: i64 = 72;
/// A strip day's cash is low when it is at most this much above the
/// leg's cheapest known cash.
pub const LOW_CASH: f64 = 1.15;
/// A program's spot check comes once a week per family.
pub const SPOT_DAYS: i64 = 7;
/// Strip clicks of a spot check, the nearest days.
pub const SPOT_CLICKS: u32 = 2;

/// How a check chooses its dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pick {
    /// Where cash is low.
    Cheap,
    /// A rotating date of the weekly grid.
    Spot,
    /// Every departure, the one read longest ago first.
    Every,
}

impl Pick {
    /// How a program's setting (0083) picks, with the weekly spot check
    /// `spot_due`.
    pub fn of(dates: AwardDates, spot_due: bool) -> Self {
        match dates {
            AwardDates::Every => Pick::Every,
            AwardDates::CheapTripsAndWeekly if spot_due => Pick::Spot,
            AwardDates::CheapTripsAndWeekly | AwardDates::CheapTrips => {
                Pick::Cheap
            }
        }
    }
}

/// What the choice is made from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    /// The first and last departures worth reading.
    pub first: NaiveDate,
    pub last: NaiveDate,
    /// The leg's cash fare per departure, minor units (lower is
    /// better).
    pub cash: BTreeMap<NaiveDate, i64>,
    /// When this family last read each departure.
    pub read: BTreeMap<NaiveDate, DateTime<Utc>>,
}

impl Known {
    fn in_range(&self, d: NaiveDate) -> bool {
        d >= self.first && d <= self.last
    }

    /// Read within [`READ_AGAIN_HOURS`] of `now`.
    pub fn read_recently(&self, d: NaiveDate, now: DateTime<Utc>) -> bool {
        self.read
            .get(&d)
            .is_some_and(|at| now - *at < TimeDelta::hours(READ_AGAIN_HOURS))
    }

    /// The departures read recently, for the strip.
    pub fn recent(&self, now: DateTime<Utc>) -> BTreeSet<NaiveDate> {
        self.read
            .keys()
            .copied()
            .filter(|d| self.read_recently(*d, now))
            .collect()
    }

    /// The leg's cheapest known cash in range.
    pub fn cheapest_cash(&self) -> Option<i64> {
        self.cash
            .iter()
            .filter(|(d, _)| self.in_range(**d))
            .map(|(_, p)| *p)
            .min()
    }

    /// The day read longest ago among `days` (never read first, then
    /// the earliest).
    fn stalest(
        &self,
        days: impl Iterator<Item = NaiveDate>,
    ) -> Option<NaiveDate> {
        days.filter(|d| self.in_range(*d))
            .min_by_key(|d| (self.read.get(d).copied(), *d))
    }
}

/// The anchor of a check, and how it was chosen: a cheap pick with no
/// cash to go on, or with every cheap day read recently, becomes a spot
/// check.
pub fn anchor(
    pick: Pick,
    known: &Known,
    now: DateTime<Utc>,
) -> Option<(NaiveDate, Pick)> {
    if known.first > known.last {
        return None;
    }
    match pick {
        Pick::Cheap => known
            .cash
            .iter()
            .filter(|(d, _)| {
                known.in_range(**d) && !known.read_recently(**d, now)
            })
            .min_by_key(|(d, p)| (**p, **d))
            .map(|(d, _)| (*d, Pick::Cheap))
            .or_else(|| anchor(Pick::Spot, known, now)),
        Pick::Spot => {
            let span = (known.last - known.first).num_days();
            let grid = (0..=span / SPOT_DAYS).filter_map(|k| {
                known
                    .first
                    .checked_add_days(Days::new((k * SPOT_DAYS) as u64))
            });
            known.stalest(grid).map(|d| (d, Pick::Spot))
        }
        Pick::Every => {
            let span = (known.last - known.first).num_days();
            let days = (0..=span).filter_map(|k| {
                known.first.checked_add_days(Days::new(k as u64))
            });
            known.stalest(days).map(|d| (d, Pick::Every))
        }
    }
}

/// The strip's days around `anchor`, the anchor left out.
pub fn strip_days(anchor: NaiveDate) -> Vec<NaiveDate> {
    (1..=STRIP_REACH)
        .flat_map(|k| {
            [
                anchor.checked_sub_days(Days::new(k)),
                anchor.checked_add_days(Days::new(k)),
            ]
        })
        .flatten()
        .collect()
}

/// What a check may click on the strip once its page has loaded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Strip {
    /// Cut to [`STRIP_CLICKS`] and to what the budget has left.
    pub max_clicks: u32,
    pub pick: Option<Pick>,
    pub first: NaiveDate,
    pub last: NaiveDate,
    /// The leg's known cash per departure (the database's), merged with
    /// the calendar the page itself fetched.
    pub cash: BTreeMap<NaiveDate, i64>,
    /// Departures the family read recently: not clicked.
    pub skip: BTreeSet<NaiveDate>,
}

impl Strip {
    /// No clicks: a dated check (the app's "Check award", a deal's
    /// confirmation) or a cash check.
    pub fn none() -> Self {
        Self::default()
    }

    /// The plan for a check of `pick` whose anchor came from `known`.
    pub fn plan(
        pick: Pick,
        known: &Known,
        now: DateTime<Utc>,
        budget: u32,
    ) -> Self {
        let wanted = match pick {
            Pick::Spot => SPOT_CLICKS,
            Pick::Cheap | Pick::Every => STRIP_CLICKS,
        };
        Self {
            max_clicks: wanted.min(budget),
            pick: Some(pick),
            first: known.first,
            last: known.last,
            cash: known.cash.clone(),
            skip: known.recent(now),
        }
    }

    /// The days to click, in order, among the days the page `shown`
    /// (`strip_days` of the anchor when it is not known), with the cash
    /// `calendar` the page fetched filling the gaps in [`Strip::cash`].
    pub fn clicks(
        &self,
        anchor: NaiveDate,
        shown: &[NaiveDate],
        calendar: &BTreeMap<NaiveDate, i64>,
    ) -> Vec<NaiveDate> {
        let Some(pick) = self.pick else {
            return Vec::new();
        };
        // The database's cash wins over the cached calendar.
        let mut merged = calendar.clone();
        merged.extend(self.cash.iter().map(|(d, p)| (*d, *p)));
        let price = |d: &NaiveDate| merged.get(d).copied();
        let distance = |d: &NaiveDate| (*d - anchor).num_days().abs();
        let mut days: Vec<NaiveDate> = shown
            .iter()
            .copied()
            .filter(|d| {
                *d != anchor
                    && *d >= self.first
                    && *d <= self.last
                    && !self.skip.contains(d)
            })
            .collect();
        days.sort_unstable();
        days.dedup();
        match pick {
            Pick::Cheap => {
                let cheapest = merged
                    .iter()
                    .filter(|(d, _)| **d >= self.first && **d <= self.last)
                    .map(|(_, p)| *p)
                    .min();
                let Some(cheapest) = cheapest else {
                    return Vec::new();
                };
                let bar = (cheapest as f64 * LOW_CASH).round() as i64;
                days.retain(|d| price(d).is_some_and(|p| p <= bar));
                days.sort_by_key(|d| (price(d), distance(d), *d));
            }
            Pick::Spot | Pick::Every => {
                days.sort_by_key(|d| (distance(d), *d));
            }
        }
        let cap = match pick {
            Pick::Spot => SPOT_CLICKS,
            Pick::Cheap | Pick::Every => STRIP_CLICKS,
        };
        days.truncate(self.max_clicks.min(cap) as usize);
        days
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).expect("a date")
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 7, 12, 0, 0)
            .single()
            .expect("a time")
    }

    fn known() -> Known {
        Known {
            first: d(10, 8),
            last: d(12, 31),
            cash: [
                (d(11, 10), 190_000),
                (d(11, 12), 200_000),
                (d(11, 20), 150_000),
            ]
            .into(),
            read: BTreeMap::new(),
        }
    }

    #[test]
    fn the_cheapest_unread_cash_day_is_the_anchor() {
        let mut k = known();
        assert_eq!(
            anchor(Pick::Cheap, &k, now()),
            Some((d(11, 20), Pick::Cheap))
        );
        // Read yesterday: the next cheapest.
        k.read.insert(d(11, 20), now() - TimeDelta::hours(20));
        assert_eq!(
            anchor(Pick::Cheap, &k, now()),
            Some((d(11, 10), Pick::Cheap))
        );
        // Read four days ago: due again.
        k.read.insert(d(11, 20), now() - TimeDelta::days(4));
        assert_eq!(
            anchor(Pick::Cheap, &k, now()),
            Some((d(11, 20), Pick::Cheap))
        );
        // Outside the range, cash does not count.
        k.first = d(11, 21);
        assert_eq!(
            anchor(Pick::Cheap, &k, now()).map(|a| a.1),
            Some(Pick::Spot)
        );
    }

    #[test]
    fn spot_checks_rotate_over_the_weekly_grid() {
        let mut k = Known {
            cash: BTreeMap::new(),
            ..known()
        };
        // No cash: a spot check, the first grid day never read.
        assert_eq!(
            anchor(Pick::Cheap, &k, now()),
            Some((d(10, 8), Pick::Spot))
        );
        k.read.insert(d(10, 8), now());
        assert_eq!(
            anchor(Pick::Spot, &k, now()),
            Some((d(10, 15), Pick::Spot))
        );
        for (i, day) in
            [d(10, 15), d(10, 22), d(10, 29)].into_iter().enumerate()
        {
            k.read.insert(day, now() - TimeDelta::days(10 - i as i64));
        }
        // Every grid day after Oct 29 is unread: Nov 5 next.
        assert_eq!(anchor(Pick::Spot, &k, now()), Some((d(11, 5), Pick::Spot)));
        assert_eq!(Pick::of(AwardDates::CheapTripsAndWeekly, true), Pick::Spot);
        assert_eq!(
            Pick::of(AwardDates::CheapTripsAndWeekly, false),
            Pick::Cheap
        );
        assert_eq!(Pick::of(AwardDates::CheapTrips, true), Pick::Cheap);
        assert_eq!(Pick::of(AwardDates::Every, false), Pick::Every);
        let empty = Known {
            first: d(11, 2),
            last: d(11, 1),
            ..Known::default()
        };
        assert_eq!(anchor(Pick::Every, &empty, now()), None);
    }

    #[test]
    fn every_reads_the_stalest_day() {
        let mut k = known();
        assert_eq!(
            anchor(Pick::Every, &k, now()),
            Some((d(10, 8), Pick::Every))
        );
        k.read.insert(d(10, 8), now());
        assert_eq!(
            anchor(Pick::Every, &k, now()),
            Some((d(10, 9), Pick::Every))
        );
    }

    #[test]
    fn cheap_strips_click_the_low_days_cheapest_first() {
        let k = Known {
            cash: [
                (d(11, 7), 300_000),
                (d(11, 9), 205_000),
                (d(11, 10), 180_000),
                (d(11, 11), 230_000),
            ]
            .into(),
            ..known()
        };
        let mut strip = Strip::plan(Pick::Cheap, &k, now(), 6);
        // The page's calendar fills Nov 12 and 13 (Nov 9 is the
        // database's); Nov 8 has nothing.
        let calendar =
            [(d(11, 9), 100), (d(11, 12), 190_000), (d(11, 13), 200_000)]
                .into();
        let shown = strip_days(d(11, 10));
        assert_eq!(shown.len(), 6);
        // Low is within 15% of R$1,800: Nov 12 (1,900), Nov 13 (2,000)
        // and Nov 9 (2,050); Nov 11 (2,300) and Nov 7 are not.
        assert_eq!(
            strip.clicks(d(11, 10), &shown, &calendar),
            vec![d(11, 12), d(11, 13), d(11, 9)]
        );
        strip.max_clicks = 1;
        assert_eq!(strip.clicks(d(11, 10), &shown, &calendar), vec![d(11, 12)]);
        strip.max_clicks = 6;
        strip.skip.insert(d(11, 12));
        assert_eq!(
            strip.clicks(d(11, 10), &shown, &calendar),
            vec![d(11, 13), d(11, 9)]
        );
        assert!(
            Strip::none()
                .clicks(d(11, 10), &shown, &calendar)
                .is_empty()
        );
    }

    #[test]
    fn spot_and_every_strips_click_the_nearest_days() {
        let k = known();
        let spot = Strip::plan(Pick::Spot, &k, now(), 6);
        let shown = strip_days(d(11, 10));
        let none = BTreeMap::new();
        assert_eq!(
            spot.clicks(d(11, 10), &shown, &none),
            vec![d(11, 9), d(11, 11)]
        );
        let every = Strip::plan(Pick::Every, &k, now(), 4);
        assert_eq!(
            every.clicks(d(11, 10), &shown, &none),
            vec![d(11, 9), d(11, 11), d(11, 8), d(11, 12)]
        );
        // Days before the first departure are never clicked.
        let early = strip_days(d(10, 9));
        assert_eq!(
            every.clicks(d(10, 9), &early, &none),
            vec![d(10, 8), d(10, 10), d(10, 11), d(10, 12)]
        );
        // A spent budget clicks nothing.
        let spent = Strip::plan(Pick::Every, &k, now(), 0);
        assert!(spent.clicks(d(11, 10), &shown, &none).is_empty());
    }
}
