//! From stored observations to trip prices, sparklines and baselines.
//! Pure: the store backend feeds it rows, the tests feed it by hand.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use tm_domain::{FamilyKind, Leg, Observation, TripRange};
use tm_scheduler::{BaselineKey, PricePoint};

use crate::grid::{TripInputs, TripPrice};

/// The trip inputs of one family set's cells (the latest per family,
/// or one day's observations): legs by departure, round trips by date
/// pair. A later observation of the same cell wins.
pub fn trip_inputs<'a>(
    cells: impl IntoIterator<Item = (FamilyKind, &'a Observation)>,
    discount: f64,
) -> TripInputs {
    let mut inputs = TripInputs {
        discount,
        ..Default::default()
    };
    let mut seen: BTreeMap<(u8, NaiveDate, Option<NaiveDate>), DateTime<Utc>> =
        BTreeMap::new();
    for (kind, obs) in cells {
        let Some(price) = &obs.price else {
            continue;
        };
        let slot = match kind {
            FamilyKind::Leg { leg: Leg::Outbound } => 0,
            FamilyKind::Leg { leg: Leg::Return } => 1,
            FamilyKind::RoundTrip { .. } => 2,
            FamilyKind::Explore => continue,
        };
        let key = (slot, obs.departure, obs.return_date);
        if seen.get(&key).is_some_and(|at| *at > obs.observed_at) {
            continue;
        }
        seen.insert(key, obs.observed_at);
        match (slot, obs.return_date) {
            (0, _) => {
                inputs.legs.outbound.insert(obs.departure, price.minor);
            }
            (1, _) => {
                inputs.legs.inbound.insert(obs.departure, price.minor);
            }
            (_, Some(ret)) => {
                inputs.round_trips.insert((obs.departure, ret), price.minor);
            }
            _ => {}
        }
    }
    inputs
}

/// Every departure that has a price of some kind.
fn departures(inputs: &TripInputs) -> Vec<NaiveDate> {
    let mut ds: Vec<NaiveDate> = inputs
        .legs
        .outbound
        .keys()
        .copied()
        .chain(inputs.round_trips.keys().map(|(d, _)| *d))
        .collect();
    ds.sort();
    ds.dedup();
    ds
}

/// The cheapest trip in range departing `from..=until`.
pub fn best_trip(
    inputs: &TripInputs,
    from: NaiveDate,
    until: NaiveDate,
    range: TripRange,
) -> Option<(NaiveDate, u16, TripPrice)> {
    departures(inputs)
        .into_iter()
        .filter(|d| *d >= from && *d <= until)
        .flat_map(|d| {
            (range.min_days..=range.max_days)
                .filter_map(move |n| Some((d, n, inputs.price(d, n)?)))
        })
        .min_by_key(|(d, n, p)| (p.minor, *d, *n))
}

/// What a route × cabin's history says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    /// The cheapest trip in range seen each day, oldest first.
    pub daily_min: Vec<f64>,
    /// Trips of the tracked round-trip length, for baselines.
    pub points: Vec<PricePoint>,
    /// The cheapest trip ever seen and the day it was seen.
    pub lowest: Option<(i64, NaiveDate)>,
}

/// Groups `observations` by the day they were made and prices every
/// trip of each day.
pub fn history(
    observations: &[(FamilyKind, Observation)],
    discount: f64,
    range: TripRange,
) -> History {
    let mut days: BTreeMap<NaiveDate, Vec<(FamilyKind, &Observation)>> =
        BTreeMap::new();
    for (kind, obs) in observations {
        days.entry(obs.observed_at.date_naive())
            .or_default()
            .push((*kind, obs));
    }
    let mut out = History::default();
    for (day, cells) in days {
        let at = cells
            .iter()
            .map(|(_, o)| o.observed_at)
            .max()
            .unwrap_or_default();
        let inputs = trip_inputs(cells, discount);
        let mut day_min: Option<i64> = None;
        for d in departures(&inputs).into_iter().filter(|d| *d >= day) {
            for n in range.min_days..=range.max_days {
                let Some(p) = inputs.price(d, n) else {
                    continue;
                };
                day_min = Some(day_min.map_or(p.minor, |m| m.min(p.minor)));
                if n == range.round_trip_days {
                    out.points.push(PricePoint {
                        observed_at: at,
                        departure: d,
                        price_minor: p.minor,
                    });
                }
            }
        }
        if let Some(m) = day_min {
            out.daily_min.push(m as f64);
            if out.lowest.is_none_or(|(l, _)| m < l) {
                out.lowest = Some((m, day));
            }
        }
    }
    out
}

/// The 60-day baseline median for every departure from `today` to
/// `today + horizon` (0030), by the scheduler's bucket and season keys.
pub fn baselines_by_departure(
    points: &[PricePoint],
    now: DateTime<Utc>,
    horizon: u16,
) -> BTreeMap<NaiveDate, i64> {
    let by_key =
        tm_scheduler::baseline::compute_by_key(points, now, Duration::days(60));
    let today = now.date_naive();
    (0..=i64::from(horizon))
        .filter_map(|i| {
            let d = today + Duration::days(i);
            let b = by_key.get(&BaselineKey::of(now, d)?)?;
            Some((d, b.median.round() as i64))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use tm_domain::{FamilyId, Money, QueryId};

    use super::*;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).expect("date")
    }

    fn obs(
        at: DateTime<Utc>,
        dep: NaiveDate,
        ret: Option<NaiveDate>,
        whole: i64,
    ) -> Observation {
        Observation {
            id: None,
            query: QueryId(1),
            family: FamilyId(1),
            observed_at: at,
            departure: dep,
            return_date: ret,
            price: Some(Money::brl_whole(whole)),
            miles: None,
            carrier: None,
            stops: None,
            duration_minutes: None,
            seller: None,
        }
    }

    const OUT: FamilyKind = FamilyKind::Leg { leg: Leg::Outbound };
    const BACK: FamilyKind = FamilyKind::Leg { leg: Leg::Return };
    const RT: FamilyKind = FamilyKind::RoundTrip { days: 7 };

    #[test]
    fn cells_become_trip_inputs() {
        let at = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let later = at + Duration::hours(1);
        let rows = [
            (OUT, obs(at, d(11, 10), None, 2000)),
            (OUT, obs(later, d(11, 10), None, 1900)),
            (BACK, obs(at, d(11, 17), None, 2100)),
            (RT, obs(at, d(11, 11), Some(d(11, 18)), 3500)),
        ];
        let inputs = trip_inputs(rows.iter().map(|(k, o)| (*k, o)), 0.1);
        assert_eq!(inputs.legs.outbound.get(&d(11, 10)), Some(&190_000));
        assert_eq!(
            inputs.round_trips.get(&(d(11, 11), d(11, 18))),
            Some(&350_000)
        );
        let best =
            best_trip(&inputs, d(10, 6), d(12, 31), TripRange::default());
        // (1900 + 2100) × 0.9 = 3600 > the 3500 round trip.
        assert_eq!(
            best.map(|(dep, n, p)| (dep, n, p.minor)),
            Some((d(11, 11), 7, 350_000))
        );
    }

    #[test]
    fn history_has_a_point_per_day() {
        let day1 = Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
        let day2 = day1 + Duration::days(1);
        let rows = vec![
            (RT, obs(day1, d(11, 10), Some(d(11, 17)), 4000)),
            (RT, obs(day2, d(11, 10), Some(d(11, 17)), 3800)),
            (RT, obs(day2, d(11, 12), Some(d(11, 19)), 3900)),
        ];
        let h = history(&rows, 0.0, TripRange::default());
        assert_eq!(h.daily_min, vec![400_000.0, 380_000.0]);
        assert_eq!(h.points.len(), 3);
        assert_eq!(h.lowest, Some((380_000, day2.date_naive())));
        let now = day2 + Duration::hours(1);
        let base = baselines_by_departure(&h.points, now, 60);
        // Nov 10 and Nov 12 are 38–40 days out: one bucket, median 3900.
        assert_eq!(base.get(&d(11, 10)), Some(&390_000));
    }
}
