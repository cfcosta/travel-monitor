//! Turns what a backend reads about one route × cabin into the
//! [`RouteDetail`] the views draw. Shared by every backend, and pure.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{Money, Settings};
use tm_scheduler::DtdBucket;

use super::{ActivityRow, RouteDetail, RowKey, SellerRow, Trip};
use crate::grid::{TripInputs, build_calendar, build_grid};

/// `21–45 days`, `180+ days`: a days-to-departure bucket (0030), as
/// the scheduler draws them.
pub fn bucket_label(bucket: DtdBucket) -> String {
    match bucket.range() {
        (lo, Some(hi)) => format!("{lo}–{hi} days"),
        (lo, None) => format!("{lo}+ days"),
    }
}

/// The usual round-trip price for each departure date (its bucket's
/// 60-day median, 0030), BRL minor units.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Baselines {
    pub by_departure: BTreeMap<NaiveDate, i64>,
}

impl Baselines {
    pub fn at(&self, departure: NaiveDate) -> Option<i64> {
        self.by_departure.get(&departure).copied()
    }
}

/// What a backend reads about the selected row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetailInputs {
    pub trip: Option<Trip>,
    /// `trip` is a deal the daemon flagged: its day reads as a deal on
    /// the calendar whatever the bucket median says.
    pub deal: bool,
    pub trips: TripInputs,
    pub baselines: Baselines,
    pub sellers: Vec<SellerRow>,
    pub airline_direct: Option<(String, Money)>,
    pub url: Option<String>,
    pub lowest_ever: Option<(Money, NaiveDate, String)>,
    pub change_rate: Option<f64>,
    pub recent_checks: Vec<ActivityRow>,
    pub next_check: Option<DateTime<Utc>>,
}

/// Weeks the calendar covers: the horizon, from the week of today.
pub fn calendar_weeks(horizon_days: u16) -> usize {
    (usize::from(horizon_days) / 7 + 1).max(1)
}

pub fn assemble(
    key: RowKey,
    inputs: DetailInputs,
    settings: &Settings,
    today: NaiveDate,
    calendar_days: u16,
    departure: Option<NaiveDate>,
) -> RouteDetail {
    let grid = inputs
        .trip
        .as_ref()
        .map(|trip| {
            build_grid(
                &inputs.trips,
                (trip.departure, trip.days()),
                2,
                settings.trips.min_days,
                settings.trips.max_days,
                today,
            )
        })
        .unwrap_or_default();
    let baselines = inputs.baselines.clone();
    let calendar = build_calendar(
        &inputs.trips,
        today,
        calendar_weeks(settings.horizon_days),
        calendar_days,
        |d| baselines.at(d),
        settings.deal_threshold,
        departure.or(inputs.trip.as_ref().map(|t| t.departure)),
    );
    let mut calendar = calendar;
    if inputs.deal
        && let Some(trip) = &inputs.trip
        && trip.days() == calendar_days
    {
        for day in calendar.weeks.iter_mut().flat_map(|w| &mut w.days) {
            if day.date == trip.departure {
                day.tone = crate::grid::Tone::Deal;
            }
        }
    }
    let usual = inputs.trip.as_ref().and_then(|trip| {
        let bucket = DtdBucket::of((trip.departure - today).num_days())?;
        let minor = inputs.baselines.at(trip.departure)?;
        Some((
            Money::new(minor, trip.price.currency.clone()),
            bucket_label(bucket),
        ))
    });
    RouteDetail {
        key,
        trip: inputs.trip,
        grid,
        calendar,
        sellers: inputs.sellers,
        airline_direct: inputs.airline_direct,
        url: inputs.url,
        lowest_ever: inputs.lowest_ever,
        usual,
        change_rate: inputs.change_rate,
        recent_checks: inputs.recent_checks,
        next_check: inputs.next_check,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_labels() {
        assert_eq!(bucket_label(DtdBucket::D0), "0–3 days");
        assert_eq!(bucket_label(DtdBucket::D21), "21–45 days");
        assert_eq!(bucket_label(DtdBucket::D180), "180+ days");
    }

    #[test]
    fn six_months_is_twenty_six_weeks() {
        assert_eq!(calendar_weeks(180), 26);
        assert_eq!(calendar_weeks(0), 1);
    }
}
