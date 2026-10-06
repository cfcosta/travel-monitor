//! Trip prices from legs and round trips (DECISIONS.md 0042, 0043), laid
//! out as the detail card's departure × length grid and the route
//! detail's six-month calendar.
//!
//! A trip of `n` days departing on `d` costs the confirmed round trip for
//! exactly those dates when one was read, else the estimate
//! `(outbound[d] + return[d + n]) × (1 − discount)`. Prices are BRL minor
//! units.

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, Duration, NaiveDate};

/// The latest one-way price per day for both legs of a route × cabin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LegPrices {
    /// Origin → destination, by departure day.
    pub outbound: BTreeMap<NaiveDate, i64>,
    /// Destination → origin, by departure day.
    pub inbound: BTreeMap<NaiveDate, i64>,
}

/// Everything a trip price is made from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TripInputs {
    pub legs: LegPrices,
    /// Round trips read as such, by (departure, return).
    pub round_trips: HashMap<(NaiveDate, NaiveDate), i64>,
    /// The route's learned round-trip discount (0.10 = 10% under the
    /// two legs); 0 on routes where round trips are additive.
    pub discount: f64,
}

/// A trip's price and whether a round-trip search confirmed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripPrice {
    pub minor: i64,
    pub confirmed: bool,
}

impl TripInputs {
    pub fn price(&self, departure: NaiveDate, days: u16) -> Option<TripPrice> {
        let return_date = departure + Duration::days(i64::from(days));
        if let Some(&minor) = self.round_trips.get(&(departure, return_date)) {
            return Some(TripPrice {
                minor,
                confirmed: true,
            });
        }
        let out = self.legs.outbound.get(&departure)?;
        let back = self.legs.inbound.get(&return_date)?;
        let estimate = (out + back) as f64 * (1.0 - self.discount);
        Some(TripPrice {
            minor: estimate.round() as i64,
            confirmed: false,
        })
    }
}

/// One cell of the departure × length grid.
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    pub departure: NaiveDate,
    pub days: u16,
    pub price: Option<TripPrice>,
    /// Within 3% of the grid's cheapest cell: drawn as a deal.
    pub best: bool,
    /// The trip the card is about.
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GridRow {
    pub departure: NaiveDate,
    pub cells: Vec<GridCell>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TripGrid {
    pub lengths: Vec<u16>,
    pub rows: Vec<GridRow>,
}

/// How close to the cheapest cell still counts as "as good" (the
/// designs highlight cells within 3%).
pub const BEST_SLACK: f64 = 1.03;

/// The grid around `selected` (departure, days): departures `±span`
/// days, lengths `min_days..=max_days`. Departures before `today` stay
/// empty.
pub fn build_grid(
    inputs: &TripInputs,
    selected: (NaiveDate, u16),
    span: i64,
    min_days: u16,
    max_days: u16,
    today: NaiveDate,
) -> TripGrid {
    let lengths: Vec<u16> = (min_days..=max_days).collect();
    let mut rows: Vec<GridRow> = (-span..=span)
        .map(|offset| {
            let departure = selected.0 + Duration::days(offset);
            let cells = lengths
                .iter()
                .map(|&days| GridCell {
                    departure,
                    days,
                    price: (departure >= today)
                        .then(|| inputs.price(departure, days))
                        .flatten(),
                    best: false,
                    selected: (departure, days) == selected,
                })
                .collect();
            GridRow { departure, cells }
        })
        .collect();
    let cheapest = rows
        .iter()
        .flat_map(|row| &row.cells)
        .filter_map(|cell| cell.price.map(|p| p.minor))
        .min();
    if let Some(cheapest) = cheapest {
        let limit = (cheapest as f64 * BEST_SLACK).round() as i64;
        for cell in rows.iter_mut().flat_map(|row| &mut row.cells) {
            cell.best = cell.price.is_some_and(|p| p.minor <= limit);
        }
    }
    TripGrid { lengths, rows }
}

/// How a departure compares with the usual price for its days to
/// departure (the heatmap's legend).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Past,
    /// No price, or no baseline to compare with.
    Unknown,
    Deal,
    Below,
    Typical,
    Above,
}

/// `ratio` is price ÷ baseline. A deal is at least `threshold` under it
/// (settings' `deal_threshold`); "below" is more than 6% under, "above"
/// more than 8% over.
pub fn tone(ratio: f64, threshold: f64) -> Tone {
    if ratio <= 1.0 - threshold {
        Tone::Deal
    } else if ratio < 0.94 {
        Tone::Below
    } else if ratio > 1.08 {
        Tone::Above
    } else {
        Tone::Typical
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarDay {
    pub date: NaiveDate,
    pub price: Option<TripPrice>,
    pub baseline: Option<i64>,
    pub tone: Tone,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarWeek {
    /// The month's name on the first week that starts in it.
    pub month: Option<&'static str>,
    /// Monday to Sunday.
    pub days: Vec<CalendarDay>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calendar {
    pub days: u16,
    pub weeks: Vec<CalendarWeek>,
}

impl Calendar {
    pub fn day(&self, date: NaiveDate) -> Option<&CalendarDay> {
        self.weeks
            .iter()
            .flat_map(|w| &w.days)
            .find(|d| d.date == date)
    }

    /// The cheapest future departure.
    pub fn cheapest(&self) -> Option<&CalendarDay> {
        self.weeks
            .iter()
            .flat_map(|w| &w.days)
            .filter(|d| d.tone != Tone::Past)
            .filter_map(|d| d.price.map(|p| (p.minor, d)))
            .min_by_key(|(minor, _)| *minor)
            .map(|(_, d)| d)
    }
}

/// The Monday on or before `date`.
pub fn week_start(date: NaiveDate) -> NaiveDate {
    date - Duration::days(i64::from(date.weekday().num_days_from_monday()))
}

/// `weeks` weeks of departures for trips of `days` days, from the week
/// of `today`, each coloured against `baseline(departure)`.
pub fn build_calendar(
    inputs: &TripInputs,
    today: NaiveDate,
    weeks: usize,
    days: u16,
    baseline: impl Fn(NaiveDate) -> Option<i64>,
    threshold: f64,
    selected: Option<NaiveDate>,
) -> Calendar {
    let start = week_start(today);
    let mut last_month = None;
    let weeks = (0..weeks)
        .map(|w| {
            let first = start + Duration::days(7 * w as i64);
            let month = (last_month != Some(first.month())).then(|| {
                last_month = Some(first.month());
                crate::format::month(first)
            });
            let days = (0..7)
                .map(|d| {
                    let date = first + Duration::days(d);
                    let past = date < today;
                    let price =
                        (!past).then(|| inputs.price(date, days)).flatten();
                    let base = (!past).then(|| baseline(date)).flatten();
                    let tone = match (past, price, base) {
                        (true, _, _) => Tone::Past,
                        (false, Some(p), Some(b)) if b > 0 => {
                            tone(p.minor as f64 / b as f64, threshold)
                        }
                        _ => Tone::Unknown,
                    };
                    CalendarDay {
                        date,
                        price,
                        baseline: base,
                        tone,
                        selected: selected == Some(date),
                    }
                })
                .collect();
            CalendarWeek { month, days }
        })
        .collect();
    Calendar { days, weeks }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).expect("date")
    }

    fn inputs() -> TripInputs {
        let mut legs = LegPrices::default();
        for i in 0..60 {
            let day = d(10, 1) + Duration::days(i);
            legs.outbound.insert(day, 200_000 + i * 1000);
            legs.inbound.insert(day, 180_000);
        }
        let mut round_trips = HashMap::new();
        round_trips.insert((d(11, 10), d(11, 17)), 300_000);
        TripInputs {
            legs,
            round_trips,
            discount: 0.10,
        }
    }

    #[test]
    fn a_round_trip_beats_the_estimate() {
        let i = inputs();
        assert_eq!(
            i.price(d(11, 10), 7),
            Some(TripPrice {
                minor: 300_000,
                confirmed: true
            })
        );
        // Oct 5 is day 4: (204,000 + 180,000) × 0.9.
        assert_eq!(
            i.price(d(10, 5), 7),
            Some(TripPrice {
                minor: 345_600,
                confirmed: false
            })
        );
        // No return leg 60 days out.
        assert_eq!(i.price(d(11, 28), 7), None);
    }

    #[test]
    fn grid_is_departures_by_lengths() {
        let grid = build_grid(&inputs(), (d(11, 10), 7), 2, 5, 10, d(10, 6));
        assert_eq!(grid.lengths, vec![5, 6, 7, 8, 9, 10]);
        assert_eq!(grid.rows.len(), 5);
        assert_eq!(grid.rows[0].departure, d(11, 8));
        assert_eq!(grid.rows[4].departure, d(11, 12));
        let selected: Vec<_> = grid
            .rows
            .iter()
            .flat_map(|r| &r.cells)
            .filter(|c| c.selected)
            .collect();
        assert_eq!(selected.len(), 1);
        assert_eq!((selected[0].departure, selected[0].days), (d(11, 10), 7));
        assert!(selected[0].price.is_some_and(|p| p.confirmed));
        // The confirmed 300,000 is the cheapest, so it is "best"; an
        // estimate of ~ 400,000 is not.
        assert!(selected[0].best);
        assert!(!grid.rows[0].cells[0].best);
        assert!(grid.rows[0].cells[0].price.is_some_and(|p| !p.confirmed));
    }

    #[test]
    fn grid_leaves_the_past_empty() {
        let grid = build_grid(&inputs(), (d(10, 6), 7), 2, 5, 6, d(10, 6));
        assert!(grid.rows[0].cells.iter().all(|c| c.price.is_none()));
        assert!(grid.rows[2].cells.iter().all(|c| c.price.is_some()));
    }

    #[test]
    fn tones_follow_the_legend() {
        assert_eq!(tone(0.80, 0.20), Tone::Deal);
        assert_eq!(tone(0.81, 0.20), Tone::Below);
        assert_eq!(tone(1.0, 0.20), Tone::Typical);
        assert_eq!(tone(1.09, 0.20), Tone::Above);
    }

    #[test]
    fn calendar_weeks_start_on_monday() {
        // 2026-10-06 is a Tuesday.
        let today = d(10, 6);
        let cal = build_calendar(
            &inputs(),
            today,
            8,
            7,
            |_| Some(400_000),
            0.20,
            Some(d(11, 10)),
        );
        assert_eq!(cal.weeks.len(), 8);
        assert_eq!(cal.weeks[0].days[0].date, d(10, 5));
        assert_eq!(cal.weeks[0].days[0].tone, Tone::Past);
        assert_ne!(cal.weeks[0].days[1].tone, Tone::Past);
        assert_eq!(cal.weeks[0].month, Some("Oct"));
        assert_eq!(cal.weeks[1].month, None);
        // The week of Nov 2 is the first to start in November.
        let nov = cal.weeks.iter().position(|w| w.month == Some("Nov"));
        assert_eq!(nov, Some(4));
        let sel = cal.day(d(11, 10)).expect("Nov 10");
        assert!(sel.selected);
        // 300,000 against 400,000 is 25% under: a deal.
        assert_eq!(sel.tone, Tone::Deal);
        assert_eq!(cal.cheapest().map(|d| d.date), Some(d(11, 10)));
    }

    #[test]
    fn calendar_without_baseline_is_unknown() {
        let cal =
            build_calendar(&inputs(), d(10, 6), 1, 7, |_| None, 0.2, None);
        assert!(
            cal.weeks[0].days[1..]
                .iter()
                .all(|d| d.tone == Tone::Unknown)
        );
    }
}
