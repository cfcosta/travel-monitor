//! From one program's award observations to what the views show
//! (DECISIONS.md 0023, 0083): the row's award line, the detail's tiles
//! and brands, and the calendar's marks. Pure, and shared by every
//! backend: the fake builds observations from the LATAM Pass fixture and
//! goes through the same code.
//!
//! Each award series (a leg one way, or the round trip) is judged
//! against its own baselines in miles; cash appears only for scale.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Duration, NaiveDate, TimeDelta, Utc};
use tm_domain::{
    Cabin,
    Currency,
    Deal,
    DealStatus,
    Leg,
    Money,
    Observation,
    QueryId,
    Settings,
};
use tm_scheduler::{
    BaselineKey,
    DtdBucket,
    PricePoint,
    awards::{UsualRate, award_points, cheapest_by_dates, is_unusual},
    baseline::compute_by_key,
};

use super::{
    AwardQuote,
    AwardSummary,
    AwardTrip,
    MilesRow,
    RouteAwards,
    RowStatus,
    Trip,
    detail::bucket_label,
};
use crate::grid::LegPrices;

/// The window award baselines are computed over (0030: 60 days).
const BASELINE_DAYS: i64 = 60;

/// What a backend reads about one program's awards on a route × cabin.
#[derive(Debug, Clone, Copy)]
pub struct AwardInputs<'a> {
    pub program: &'a MilesRow,
    pub cabin: Cabin,
    /// The latest check's cells of the outbound, return and round-trip
    /// families.
    pub outbound: &'a [Observation],
    pub inbound: &'a [Observation],
    pub round_trip: &'a [Observation],
    /// Other cabins' outbound cells: the same loads read every cabin.
    pub other_cabins: &'a [(Cabin, Observation)],
    /// The outbound family's observations of the last 60 days.
    pub history: &'a [Observation],
    /// The newest open award deal of this program, route and cabin.
    pub deal: Option<&'a Deal>,
    pub checked_at: Option<DateTime<Utc>>,
}

/// An observation of `program` as a quote.
pub fn quote(
    program: &MilesRow,
    cabin: Cabin,
    leg: Option<Leg>,
    o: &Observation,
) -> Option<AwardQuote> {
    let detail = o.award.as_ref();
    Some(AwardQuote {
        program: program.program.clone(),
        unit: program.unit,
        price: o.award_price()?,
        fare: detail.and_then(|d| d.fare.clone()),
        brand: detail.and_then(|d| d.brand.clone()),
        cabin,
        flights: o.flights.clone(),
        stops: o.stops,
        seats: detail.and_then(|d| d.seats),
        departure: o.departure,
        return_date: o.return_date,
        leg,
        observed_at: o.observed_at,
    })
}

/// The quote an award deal stands for, when its observation is gone.
fn deal_quote(program: &MilesRow, deal: &Deal) -> Option<AwardQuote> {
    Some(AwardQuote {
        program: program.program.clone(),
        unit: program.unit,
        price: deal.award_price()?,
        fare: None,
        brand: deal.brand.clone(),
        cabin: deal.cabin,
        flights: None,
        stops: None,
        seats: deal.seats,
        departure: deal.departure,
        return_date: deal.return_date,
        leg: deal.leg,
        observed_at: deal.found_at,
    })
}

/// The cheapest current award of each date of a family's cells, from
/// `today` on.
fn current(
    cells: &[Observation],
    today: NaiveDate,
) -> BTreeMap<tm_scheduler::awards::AwardKey, &Observation> {
    cheapest_by_dates(cells.iter().filter(|c| c.departure >= today))
}

fn cheapest_of<'a>(
    map: &BTreeMap<tm_scheduler::awards::AwardKey, &'a Observation>,
) -> Option<&'a Observation> {
    map.values().copied().min_by_key(|o| {
        (o.miles, o.price.as_ref().map(|p| p.minor), o.departure)
    })
}

/// The program's usual rate, as its row carries it.
pub fn usual_of(program: &MilesRow) -> Option<UsualRate> {
    program.usual_rate.map(|median| UsualRate {
        median,
        n: program.rate_checks,
    })
}

fn brl(minor: i64) -> Money {
    Money::new(minor, Currency::brl())
}

/// The past prices of the outbound series and their baselines.
struct Series {
    points: Vec<PricePoint>,
    baselines: BTreeMap<BaselineKey, tm_scheduler::Baseline>,
}

fn series(inputs: &AwardInputs<'_>, now: DateTime<Utc>) -> Series {
    let family = inputs
        .history
        .first()
        .or(inputs.outbound.first())
        .map(|o| o.family);
    let points = family
        .map(|f| award_points(inputs.history, f, &HashSet::new()))
        .unwrap_or_default();
    let baselines =
        compute_by_key(&points, now, TimeDelta::days(BASELINE_DAYS));
    Series { points, baselines }
}

/// The row's award line: the cheapest award on sale (the outbound's,
/// else the return's), its own "% vs usual", whether it is a miles deal,
/// the measured value of a mile, and the row's own dates in awards.
/// `None` when the program priced nothing on this route and cabin.
pub fn summary(
    inputs: &AwardInputs<'_>,
    cash: &LegPrices,
    row_trip: Option<&Trip>,
    settings: &Settings,
    now: DateTime<Utc>,
) -> Option<AwardSummary> {
    let today = now.date_naive();
    let program = inputs.program;
    let out = current(inputs.outbound, today);
    let back = current(inputs.inbound, today);
    let rt = current(inputs.round_trip, today);
    let deal = inputs.deal.filter(|d| {
        d.departure >= today
            && matches!(d.status, DealStatus::Confirmed | DealStatus::Candidate)
    });
    let best = match deal {
        Some(d) => {
            let cells = match (d.leg, d.return_date) {
                (Some(Leg::Return), _) => &back,
                (None, Some(_)) => &rt,
                _ => &out,
            };
            cells
                .get(&(d.departure, d.return_date))
                .filter(|o| o.miles == d.miles)
                .and_then(|o| quote(program, inputs.cabin, d.leg, o))
                .or_else(|| deal_quote(program, d))
        }
        None => cheapest_of(&out)
            .and_then(|o| quote(program, inputs.cabin, Some(Leg::Outbound), o))
            .or_else(|| {
                cheapest_of(&back).and_then(|o| {
                    quote(program, inputs.cabin, Some(Leg::Return), o)
                })
            }),
    }?;
    let s = series(inputs, now);
    let (baseline_miles, change) = match deal {
        Some(d) => (d.baseline_miles, d.change),
        None if best.leg == Some(Leg::Outbound) => {
            // Like cash, a bucket needs weeks of checks first (0030).
            let baseline = BaselineKey::of(now, best.departure)
                .and_then(|k| s.baselines.get(&k))
                .filter(|b| b.median > 0.0)
                .filter(|b| {
                    b.weeks_of_history >= f64::from(settings.min_history_weeks)
                });
            (
                baseline.map(|b| b.median.round() as i64),
                baseline.map(|b| best.price.miles as f64 / b.median - 1.0),
            )
        }
        None => (None, None),
    };
    let mut daily: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for p in &s.points {
        daily
            .entry(p.observed_at.date_naive())
            .and_modify(|m| *m = (*m).min(p.price_minor))
            .or_insert(p.price_minor);
    }
    let rate = best.rate();
    let usual = usual_of(program);
    let unusual = match (rate, &usual) {
        (Some(r), Some(u)) => is_unusual(r, u, settings.unusual_rate),
        _ => false,
    };
    let cash_minor = match best.leg {
        Some(Leg::Outbound) => cash.outbound.get(&best.departure).copied(),
        Some(Leg::Return) => cash.inbound.get(&best.departure).copied(),
        None => None,
    };
    let trip = row_trip.map(|t| {
        let pick =
            |cells: &BTreeMap<tm_scheduler::awards::AwardKey, &Observation>,
             dates,
             leg| {
                cells
                    .get(&dates)
                    .and_then(|o| quote(program, inputs.cabin, leg, o))
            };
        AwardTrip {
            departure: t.departure,
            return_date: t.return_date,
            outbound: pick(&out, (t.departure, None), Some(Leg::Outbound)),
            inbound: pick(&back, (t.return_date, None), Some(Leg::Return)),
            round_trip: pick(&rt, (t.departure, Some(t.return_date)), None),
        }
    });
    Some(AwardSummary {
        program: program.program.clone(),
        name: program.name.clone(),
        best,
        trip: trip.filter(AwardTrip::any),
        baseline_miles,
        change,
        status: match deal.map(|d| d.status) {
            Some(DealStatus::Confirmed) => RowStatus::Deal,
            Some(DealStatus::Candidate) => RowStatus::Confirming,
            _ => RowStatus::Watching,
        },
        deal: deal.and_then(|d| d.id),
        found_at: deal.map(|d| d.confirmed_at.unwrap_or(d.found_at)),
        history_days: daily.len(),
        history: daily.into_values().map(|m| m as f64).collect(),
        rate,
        usual_rate: usual.map(|u| u.median),
        unusual,
        // The deal's own cash, as seen when it was found, else today's.
        cash: deal.and_then(|d| d.cash.clone()).or(cash_minor.map(brl)),
        value_per_1000: program.settings.value_per_1000.clone(),
        buy_per_1000: program.settings.buy_per_1000.clone(),
    })
}

/// Brands of one date, fewest miles first, then lowest taxes.
fn brands(
    program: &MilesRow,
    cabin: Cabin,
    leg: Option<Leg>,
    cells: impl IntoIterator<Item = (Cabin, Observation)>,
) -> Vec<AwardQuote> {
    let mut out: Vec<AwardQuote> = cells
        .into_iter()
        .filter_map(|(c, o)| {
            quote(program, if leg.is_some() { c } else { cabin }, leg, &o)
        })
        .collect();
    out.sort_by_key(|q| {
        (q.price.miles, q.price.taxes.minor, q.flights.clone())
    });
    out
}

/// The route detail's award block for `departure` and trips of `days`
/// days: the tiles of the outbound series and the brands of each leg
/// and of the round trip.
pub fn route_awards(
    inputs: &AwardInputs<'_>,
    cash: &LegPrices,
    departure: Option<NaiveDate>,
    days: u16,
    settings: &Settings,
    now: DateTime<Utc>,
) -> RouteAwards {
    let today = now.date_naive();
    let program = inputs.program;
    let cheapest = summary(inputs, cash, None, settings, now).map(|s| s.best);
    let s = series(inputs, now);
    let lowest_ever = s
        .points
        .iter()
        .min_by_key(|p| (p.price_minor, p.observed_at))
        .map(|p| (p.price_minor, p.observed_at.date_naive()));
    let usual = cheapest
        .as_ref()
        .filter(|q| q.leg == Some(Leg::Outbound))
        .and_then(|q| {
            let key = BaselineKey::of(now, q.departure)?;
            let b = s.baselines.get(&key).filter(|b| {
                b.weeks_of_history >= f64::from(settings.min_history_weeks)
            })?;
            let bucket = DtdBucket::of((q.departure - today).num_days())?;
            Some((b.median.round() as i64, bucket_label(bucket)))
        });
    let checks = inputs
        .history
        .iter()
        .map(|o| o.query)
        .collect::<HashSet<QueryId>>()
        .len();
    let known: BTreeSet<NaiveDate> = inputs
        .outbound
        .iter()
        .chain(inputs.inbound)
        .filter(|o| o.departure >= today && o.miles.is_some())
        .map(|o| o.departure)
        .collect();
    let departure = departure.or(cheapest.as_ref().map(|q| q.departure));
    let ret = departure.map(|d| d + Duration::days(i64::from(days)));
    let on = |cells: &[Observation],
              date: Option<NaiveDate>,
              rt: Option<NaiveDate>| {
        cells
            .iter()
            .filter(|o| Some(o.departure) == date && o.return_date == rt)
            .map(|o| (inputs.cabin, o.clone()))
            .collect::<Vec<_>>()
    };
    let same_load = inputs
        .other_cabins
        .iter()
        .filter(|(_, o)| {
            Some(o.departure) == departure && o.return_date.is_none()
        })
        .cloned();
    RouteAwards {
        program: program.program.clone(),
        name: program.name.clone(),
        unit: program.unit,
        login: program.login,
        cheapest,
        lowest_ever,
        usual,
        checks,
        checked_at: inputs.checked_at,
        dates_known: known.len(),
        dates_total: usize::from(settings.horizon_days),
        outbound: brands(
            program,
            inputs.cabin,
            Some(Leg::Outbound),
            on(inputs.outbound, departure, None),
        ),
        inbound: brands(
            program,
            inputs.cabin,
            Some(Leg::Return),
            on(inputs.inbound, ret, None),
        ),
        round_trip: match departure {
            Some(_) => brands(
                program,
                inputs.cabin,
                None,
                on(inputs.round_trip, departure, ret),
            ),
            None => Vec::new(),
        },
        same_load: brands(
            program,
            inputs.cabin,
            Some(Leg::Outbound),
            same_load,
        ),
        usual_rate: usual_of(program),
        unusual_threshold: settings.unusual_rate,
        cash_outbound: departure
            .and_then(|d| cash.outbound.get(&d).copied())
            .map(brl),
        value_per_1000: program.settings.value_per_1000.clone(),
        buy_per_1000: program.settings.buy_per_1000.clone(),
    }
}

/// Which dates have an award, for the calendar's and the strip's marks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AwardMarks {
    pub outbound: BTreeSet<NaiveDate>,
    pub inbound: BTreeSet<NaiveDate>,
    pub round_trips: BTreeSet<(NaiveDate, NaiveDate)>,
}

impl AwardMarks {
    pub fn of(inputs: &AwardInputs<'_>) -> Self {
        let dates = |cells: &[Observation]| {
            cells
                .iter()
                .filter(|o| o.miles.is_some() && o.return_date.is_none())
                .map(|o| o.departure)
                .collect()
        };
        Self {
            outbound: dates(inputs.outbound),
            inbound: dates(inputs.inbound),
            round_trips: inputs
                .round_trip
                .iter()
                .filter_map(|o| Some((o.departure, o.return_date?)))
                .collect(),
        }
    }

    /// A trip of `days` days departing `departure`: both ways priced
    /// (both legs, or the round trip), one leg, or none.
    pub fn mark(
        &self,
        departure: NaiveDate,
        days: u16,
    ) -> crate::grid::AwardMark {
        use crate::grid::AwardMark;
        let ret = departure + Duration::days(i64::from(days));
        let out = self.outbound.contains(&departure);
        let back = self.inbound.contains(&ret);
        if self.round_trips.contains(&(departure, ret)) || (out && back) {
            AwardMark::BothLegs
        } else if out || back {
            AwardMark::OneLeg
        } else {
            AwardMark::None
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use tm_domain::{AwardDetail, FamilyId, MilesUnit, ProgramSettings, Seats};

    use super::*;
    use crate::data::Login;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, 15, 0, 0).unwrap()
    }

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).expect("date")
    }

    fn latam() -> MilesRow {
        MilesRow {
            program: "latam_pass".into(),
            name: "LATAM Pass".into(),
            login: Login::LoggedIn,
            last_checked: None,
            session_until: None,
            settings: ProgramSettings {
                active: true,
                ..ProgramSettings::default()
            },
            has_reader: true,
            unit: MilesUnit::Miles,
            usual_rate: Some(28.8),
            rate_checks: 12,
        }
    }

    fn cell(
        q: i64,
        at: DateTime<Utc>,
        dep: NaiveDate,
        ret: Option<NaiveDate>,
        miles: i64,
        fare: i64,
    ) -> Observation {
        Observation {
            id: None,
            query: QueryId(q),
            family: FamilyId(7),
            observed_at: at,
            departure: dep,
            return_date: ret,
            price: Some(brl(6861)),
            miles: Some(miles),
            carrier: None,
            stops: Some(0),
            duration_minutes: Some(580),
            seller: None,
            flights: Some("LA8146@2026-11-10T17:40".into()),
            award: Some(AwardDetail {
                fare: Some(brl(fare)),
                brand: Some("LIGHT".into()),
                fare_basis: None,
                seats: Some(Seats {
                    count: 9,
                    low: false,
                }),
            }),
        }
    }

    #[test]
    fn a_row_leads_with_the_cheapest_outbound_award() {
        let outbound = vec![
            cell(1, now(), d(11, 10), None, 63_848, 183_882),
            cell(1, now(), d(11, 10), None, 71_786, 206_743),
            cell(1, now(), d(11, 12), None, 79_896, 230_101),
        ];
        // A discounted award: 30% more reais per mile than usual.
        let inbound = vec![cell(2, now(), d(11, 17), None, 50_000, 187_200)];
        let program = latam();
        let inputs = AwardInputs {
            program: &program,
            cabin: Cabin::Economy,
            outbound: &outbound,
            inbound: &inbound,
            round_trip: &[],
            other_cabins: &[],
            history: &outbound,
            deal: None,
            checked_at: Some(now()),
        };
        let mut cash = LegPrices::default();
        cash.outbound.insert(d(11, 10), 190_743);
        let trip = Trip {
            departure: d(11, 10),
            return_date: d(11, 17),
            price: Money::brl_whole(3932),
            carrier: None,
            stops: None,
            duration_minutes: None,
            estimated: false,
        };
        let s =
            summary(&inputs, &cash, Some(&trip), &Settings::default(), now())
                .expect("priced");
        assert_eq!(s.best.price.text(s.best.unit), "63,848 mi + R$69");
        assert_eq!(s.best.route_label().as_deref(), Some("LA8146 nonstop"));
        assert!(s.rate.is_some_and(|r| (r - 28.80).abs() < 0.01));
        assert!(!s.unusual);
        assert_eq!(s.cash, Some(brl(190_743)));
        assert!(
            s.rate_against_cash()
                .is_some_and(|r| (r - 28.80).abs() < 0.01)
        );
        assert_eq!(s.status, RowStatus::Watching);
        // One day of history: no baseline yet in miles.
        assert_eq!(s.history_days, 1);
        let t = s.trip.expect("row dates");
        assert!(t.complete());
        assert_eq!(t.price().map(|p| p.miles), Some(113_848));

        let unusual = AwardInputs {
            outbound: &[],
            ..inputs
        };
        let s = summary(&unusual, &cash, None, &Settings::default(), now())
            .expect("return");
        assert_eq!(s.best.leg, Some(Leg::Return));
        assert!(s.unusual, "{:?}", s.rate);

        let marks = AwardMarks::of(&inputs);
        assert_eq!(marks.mark(d(11, 10), 7), crate::grid::AwardMark::BothLegs);
        assert_eq!(marks.mark(d(11, 12), 7), crate::grid::AwardMark::OneLeg);
        assert_eq!(marks.mark(d(11, 20), 7), crate::grid::AwardMark::None);

        let detail = route_awards(
            &inputs,
            &cash,
            Some(d(11, 10)),
            7,
            &Settings::default(),
            now(),
        );
        assert_eq!(detail.outbound.len(), 2);
        assert_eq!(detail.inbound.len(), 1);
        assert_eq!(detail.dates_known, 3);
        assert_eq!(detail.cash_outbound, Some(brl(190_743)));
        assert_eq!(detail.lowest_ever.map(|l| l.0), Some(63_848));
        assert!(!detail.unusual(&detail.outbound[0]));
    }

    #[test]
    fn a_miles_deal_sets_the_row_status() {
        let outbound = vec![cell(1, now(), d(11, 10), None, 263_848, 759_882)];
        let program = latam();
        let deal = Deal {
            id: Some(tm_domain::DealId(9)),
            route: tm_domain::RouteId(1),
            cabin: Cabin::Business,
            pos: tm_domain::PosId(1),
            series: tm_domain::Series::Award {
                program: "latam_pass".into(),
            },
            leg: Some(Leg::Outbound),
            departure: d(11, 10),
            return_date: None,
            price: brl(6861),
            effective_brl: brl(6861),
            baseline: None,
            miles: Some(263_848),
            baseline_miles: Some(338_000),
            brand: None,
            seats: None,
            cash: None,
            reason: tm_domain::DealReason::BelowMedian,
            change: Some(-0.22),
            estimated: false,
            status: DealStatus::Confirmed,
            found_at: now(),
            confirmed_at: None,
            seen: false,
            carrier: None,
            seller: None,
            sellers: Vec::new(),
            url: None,
        };
        let inputs = AwardInputs {
            program: &program,
            cabin: Cabin::Business,
            outbound: &outbound,
            inbound: &[],
            round_trip: &[],
            other_cabins: &[],
            history: &outbound,
            deal: Some(&deal),
            checked_at: None,
        };
        let s = summary(
            &inputs,
            &LegPrices::default(),
            None,
            &Settings::default(),
            now(),
        )
        .expect("priced");
        assert_eq!(s.status, RowStatus::Deal);
        assert_eq!(s.change, Some(-0.22));
        assert_eq!(s.baseline_miles, Some(338_000));
        assert_eq!(s.deal, Some(tm_domain::DealId(9)));
        assert_eq!(s.best.brand.as_deref(), Some("LIGHT"));
    }
}
