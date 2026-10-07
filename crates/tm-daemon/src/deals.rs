//! From cells to deals (DECISIONS.md 0030, 0031, 0033, 0034, 0042, 0043).
//!
//! The pure half of the pipeline: leg and round-trip cells become trips
//! of 5–10 days; each trip is compared with the baseline of its own
//! series (point of sale × trip length × estimated or real ×
//! days-to-departure bucket × season), rebuilt from the last 60 days of
//! observations without the query being judged; the best few become
//! candidates; a confirmation's results and booking page decide whether
//! a candidate holds and what it costs in effective BRL.
//!
//! Award series (0083) have their own half at the end: one program's
//! cheapest award per date (a leg) or date pair (its round trip), in
//! miles, against that series' own baselines, never against cash
//! (0023).

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{
    Cabin,
    Currency,
    Deal,
    DealReason,
    DealStatus,
    FamilyId,
    Leg,
    Money,
    Observation,
    PointOfSale,
    QueryId,
    RouteId,
    SellerQuote,
    Series,
    TripRange,
    itinerary_label,
};
use tm_scheduler::{
    BaselineKey,
    FxTable,
    Params,
    PricePoint,
    Trip,
    awards::AwardDates,
    baseline::compute_by_key,
    effective::effective,
    evaluate,
    trips::build_trips,
};

use crate::sources::Confirmation;

/// A confirmation holds when the round trip costs at most this much
/// more than the candidate (0031).
pub const HOLD_TOLERANCE: f64 = 1.03;
/// A confirmed deal expires once its dates cost this much more.
pub const EXPIRE_RISE: f64 = 1.10;
/// Candidates confirmed per route × cabin × point of sale and check: a
/// confirmation is three page loads.
pub const CONFIRMS_PER_GROUP: usize = 2;
/// A rejected candidate is not tried again for the same dates sooner.
pub const REJECT_MEMORY_HOURS: i64 = 24;

/// One price per departure: a leg's cells, BRL minor units. Cells
/// without a price, or with a return date, are left out.
pub fn leg_prices(cells: &[Observation]) -> BTreeMap<NaiveDate, i64> {
    let mut out: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for c in cells {
        if c.return_date.is_some() {
            continue;
        }
        if let Some(p) =
            c.price.as_ref().filter(|p| p.currency.as_str() == "BRL")
        {
            out.entry(c.departure)
                .and_modify(|v| *v = (*v).min(p.minor))
                .or_insert(p.minor);
        }
    }
    out
}

/// One price per date pair: a round trip's cells, BRL minor units.
pub fn round_trip_prices(
    cells: &[Observation],
) -> BTreeMap<(NaiveDate, NaiveDate), i64> {
    let mut out: BTreeMap<(NaiveDate, NaiveDate), i64> = BTreeMap::new();
    for c in cells {
        let (Some(r), Some(p)) = (c.return_date, c.price.as_ref()) else {
            continue;
        };
        if p.currency.as_str() != "BRL" {
            continue;
        }
        out.entry((c.departure, r))
            .and_modify(|v| *v = (*v).min(p.minor))
            .or_insert(p.minor);
    }
    out
}

/// The real round-trip cells as trips (not estimated).
pub fn real_trips(rt: &BTreeMap<(NaiveDate, NaiveDate), i64>) -> Vec<Trip> {
    rt.iter()
        .filter_map(|(&(d, r), &p)| {
            let days = u16::try_from((r - d).num_days()).ok()?;
            Some(Trip {
                departure: d,
                return_date: r,
                days,
                price_minor: p,
                legs_sum_minor: 0,
            })
        })
        .collect()
}

/// The families of one route × cabin × point of sale on Google Flights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub outbound: FamilyId,
    pub ret: FamilyId,
    /// The round-trip family and its trip length, when it exists.
    pub round_trip: Option<(FamilyId, u16)>,
}

/// A cash series: trips of one length, estimated from legs or real.
pub type TripSeries = (u16, bool);

/// Past trip prices per series, rebuilt from `history` (the route's
/// observations) without the queries in `exclude` (the ones being
/// judged, 0030: a fare is compared with history that excludes it).
/// Legs observed on the same day are paired with that day's discount;
/// each day contributes one price per trip.
pub fn history_points(
    history: &[Observation],
    group: &Group,
    range: TripRange,
    discount: f64,
    exclude: &HashSet<QueryId>,
) -> BTreeMap<TripSeries, Vec<PricePoint>> {
    type Day = BTreeMap<NaiveDate, (DateTime<Utc>, i64)>;
    let mut days: BTreeMap<NaiveDate, (Day, Day)> = BTreeMap::new();
    let mut out: BTreeMap<TripSeries, Vec<PricePoint>> = BTreeMap::new();
    for o in history {
        if exclude.contains(&o.query) {
            continue;
        }
        let Some(price) =
            o.price.as_ref().filter(|p| p.currency.as_str() == "BRL")
        else {
            continue;
        };
        if let Some((rt, len)) = group.round_trip
            && o.family == rt
        {
            if o.return_date
                .is_some_and(|r| (r - o.departure).num_days() == i64::from(len))
            {
                out.entry((len, false)).or_default().push(PricePoint {
                    observed_at: o.observed_at,
                    departure: o.departure,
                    price_minor: price.minor,
                });
            }
            continue;
        }
        let side = if o.family == group.outbound {
            0
        } else if o.family == group.ret {
            1
        } else {
            continue;
        };
        let entry = days.entry(o.observed_at.date_naive()).or_default();
        let map = if side == 0 {
            &mut entry.0
        } else {
            &mut entry.1
        };
        map.entry(o.departure)
            .and_modify(|(at, p)| {
                if o.observed_at >= *at {
                    *at = o.observed_at;
                    *p = price.minor;
                }
            })
            .or_insert((o.observed_at, price.minor));
    }
    for (out_day, ret_day) in days.values() {
        if out_day.is_empty() || ret_day.is_empty() {
            continue;
        }
        let latest = out_day
            .values()
            .chain(ret_day.values())
            .map(|(at, _)| *at)
            .max();
        let Some(observed_at) = latest else {
            continue;
        };
        let o: BTreeMap<NaiveDate, i64> =
            out_day.iter().map(|(d, (_, p))| (*d, *p)).collect();
        let r: BTreeMap<NaiveDate, i64> =
            ret_day.iter().map(|(d, (_, p))| (*d, *p)).collect();
        for t in build_trips(&o, &r, range, discount) {
            out.entry((t.days, true)).or_default().push(PricePoint {
                observed_at,
                departure: t.departure,
                price_minor: t.price_minor,
            });
        }
    }
    out
}

/// A trip that beats its baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub trip: Trip,
    pub estimated: bool,
    pub reason: DealReason,
    pub change: f64,
    /// The bucket median, minor units.
    pub baseline_minor: i64,
}

/// Every current trip that is a deal against its series' baseline, best
/// (most negative change) first. Trips departing outside
/// `first..=last` are ignored.
pub fn find(
    trips: &[(Trip, bool)],
    points: &BTreeMap<TripSeries, Vec<PricePoint>>,
    now: DateTime<Utc>,
    params: &Params,
    first: NaiveDate,
    last: NaiveDate,
) -> Vec<Found> {
    let mut baselines = BTreeMap::new();
    let mut out = Vec::new();
    for &(trip, estimated) in trips {
        if trip.departure < first || trip.departure > last {
            continue;
        }
        let series = (trip.days, estimated);
        let by_key = baselines.entry(series).or_insert_with(|| {
            points
                .get(&series)
                .map(|p| compute_by_key(p, now, params.baseline_window))
                .unwrap_or_default()
        });
        let Some(key) = BaselineKey::of(now, trip.departure) else {
            continue;
        };
        let Some(baseline) = by_key.get(&key) else {
            continue;
        };
        if let Some((reason, change)) =
            evaluate(trip.price_minor, baseline, params)
        {
            out.push(Found {
                trip,
                estimated,
                reason,
                change,
                baseline_minor: baseline.median.round() as i64,
            });
        }
    }
    out.sort_by(|a, b| {
        a.change
            .total_cmp(&b.change)
            .then(a.trip.departure.cmp(&b.trip.departure))
            .then(a.trip.days.cmp(&b.trip.days))
    });
    out
}

/// Whether a departure already has a cash deal: an open one, or one
/// rejected recently. One deal per departure: the app shows the
/// departure × return grid around it (0043). Award deals are their own
/// series and never stand in for a cash one (0023).
pub fn known(
    deals: &[Deal],
    (route, cabin, pos): (RouteId, Cabin, tm_domain::PosId),
    departure: NaiveDate,
    now: DateTime<Utc>,
) -> bool {
    deals.iter().any(|d| {
        d.series == Series::Cash
            && d.route == route
            && d.cabin == cabin
            && d.pos == pos
            && d.departure == departure
            && still_open(d, now)
    })
}

/// Open, or rejected too recently to try again.
fn still_open(deal: &Deal, now: DateTime<Utc>) -> bool {
    match deal.status {
        DealStatus::Candidate | DealStatus::Confirmed => true,
        DealStatus::Rejected => {
            now - deal.found_at < chrono::TimeDelta::hours(REJECT_MEMORY_HOURS)
        }
        DealStatus::Expired => false,
    }
}

/// The new candidates worth confirming: not already known, one per
/// departure, at most `max`.
pub fn select(
    found: Vec<Found>,
    deals: &[Deal],
    group: (RouteId, Cabin, tm_domain::PosId),
    now: DateTime<Utc>,
    max: usize,
) -> Vec<Found> {
    let mut departures = HashSet::new();
    found
        .into_iter()
        .filter(|f| !known(deals, group, f.trip.departure, now))
        .filter(|f| departures.insert(f.trip.departure))
        .take(max)
        .collect()
}

/// A foreign point of sale's price as effective BRL before any seller
/// is known: assume the seller charges in its own currency (0038).
pub fn estimated_effective(
    price: &Money,
    pos: &PointOfSale,
    params: &Params,
) -> Money {
    if pos.is_home {
        return price.clone();
    }
    let minor =
        price.minor as f64 * (1.0 + params.iof) * (1.0 + params.card_spread);
    Money::new(minor.round() as i64, Currency::brl())
}

/// The candidate deal for a found trip.
pub fn candidate(
    found: &Found,
    route: RouteId,
    cabin: Cabin,
    pos: &PointOfSale,
    now: DateTime<Utc>,
    params: &Params,
    url: Option<String>,
) -> Deal {
    let price = Money::new(found.trip.price_minor, Currency::brl());
    Deal {
        id: None,
        route,
        cabin,
        pos: pos.id,
        series: Series::Cash,
        leg: None,
        departure: found.trip.departure,
        return_date: Some(found.trip.return_date),
        effective_brl: estimated_effective(&price, pos, params),
        price,
        baseline: Some(Money::new(found.baseline_minor, Currency::brl())),
        miles: None,
        baseline_miles: None,
        brand: None,
        seats: None,
        cash: None,
        reason: found.reason,
        change: Some(found.change),
        estimated: found.estimated,
        status: DealStatus::Candidate,
        found_at: now,
        confirmed_at: None,
        seen: false,
        carrier: None,
        seller: None,
        sellers: Vec::new(),
        url,
    }
}

/// What a confirmation decided.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub held: bool,
    /// The confirmed round-trip price (display currency, BRL).
    pub price: Money,
    /// The cheapest seller's effective BRL cost, or the price.
    pub effective: Money,
    pub seller: Option<String>,
    pub quotes: Vec<SellerQuote>,
    /// BRL per unit, derived from the booking page's
    /// display-versus-quoted pairs (0038), one per currency.
    pub rates: Vec<(Currency, f64)>,
}

/// Judges a candidate of `candidate_minor` against a confirmation.
/// `None` when the results page showed no price.
pub fn judge(
    candidate_minor: i64,
    confirmation: &Confirmation,
    pos: &PointOfSale,
    fx: &FxTable,
    params: &Params,
) -> Option<Verdict> {
    let cheapest = confirmation.cheapest?;
    let price = Money::brl_whole(cheapest);
    let held = (price.minor as f64) <= candidate_minor as f64 * HOLD_TOLERANCE;
    let quotes: Vec<SellerQuote> = confirmation
        .offers
        .iter()
        .filter_map(|o| o.to_quote(pos.id, &Currency::brl()))
        .collect();
    let rates = derived_rates(&quotes);
    let mut fx = fx.clone();
    for (currency, rate) in &rates {
        fx.brl_per_unit.insert(currency.as_str().to_owned(), *rate);
    }
    let best = quotes
        .iter()
        .filter_map(|q| {
            let cost = match &q.quoted {
                Some(quoted) if quoted.currency.as_str() != "BRL" => {
                    effective(quoted, &fx, params)?
                }
                _ => q.price.clone(),
            };
            Some((cost, q))
        })
        .min_by_key(|(cost, _)| cost.minor);
    let (effective, seller) = match best {
        Some((cost, q)) => (cost, Some(q.seller.clone())),
        None => (estimated_effective(&price, pos, params), None),
    };
    Some(Verdict {
        held,
        price,
        effective,
        seller,
        quotes,
        rates,
    })
}

/// The median BRL-per-unit rate of each quoted foreign currency.
pub fn derived_rates(quotes: &[SellerQuote]) -> Vec<(Currency, f64)> {
    let mut by: BTreeMap<Currency, Vec<f64>> = BTreeMap::new();
    for q in quotes {
        let Some(quoted) = &q.quoted else { continue };
        if quoted.currency.as_str() == "BRL"
            || q.price.currency.as_str() != "BRL"
            || quoted.minor <= 0
        {
            continue;
        }
        by.entry(quoted.currency.clone())
            .or_default()
            .push(q.price.minor as f64 / quoted.minor as f64);
    }
    by.into_iter()
        .filter_map(|(c, mut rs)| {
            rs.sort_by(f64::total_cmp);
            let n = rs.len();
            let m = if n == 0 {
                return None;
            } else if n % 2 == 1 {
                rs[n / 2]
            } else {
                0.5 * (rs[n / 2 - 1] + rs[n / 2])
            };
            Some((c, m))
        })
        .collect()
}

/// Whether a confirmed deal is gone: departed, or its dates now cost
/// [`EXPIRE_RISE`] times more (estimated or real, whichever matches).
/// `current` holds cash prices: an award deal only expires here once
/// departed ([`award_expired`] weighs its miles).
pub fn expired(
    deal: &Deal,
    current: &BTreeMap<(NaiveDate, NaiveDate), i64>,
    today: NaiveDate,
) -> bool {
    if deal.departure < today {
        return true;
    }
    let (Series::Cash, Some(return_date)) = (&deal.series, deal.return_date)
    else {
        return false;
    };
    current
        .get(&(deal.departure, return_date))
        .is_some_and(|&p| p as f64 > deal.price.minor as f64 * EXPIRE_RISE)
}

// ---------------------------------------------------------------------
// Award series (0083)
// ---------------------------------------------------------------------

/// The award families of one route × cabin × program, each a series of
/// its own: both legs one way, and the round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwardGroup {
    pub outbound: FamilyId,
    pub ret: FamilyId,
    pub round_trip: Option<FamilyId>,
}

impl AwardGroup {
    /// The leg a family prices, `None` for the round trip (or another
    /// family).
    pub fn leg_of(&self, family: FamilyId) -> Option<Leg> {
        if family == self.outbound {
            Some(Leg::Outbound)
        } else if family == self.ret {
            Some(Leg::Return)
        } else {
            None
        }
    }
}

/// An award that beats its series' baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundAward {
    /// The cheapest award of its date (or date pair) in the check.
    pub award: Observation,
    pub reason: DealReason,
    /// Its miles against the bucket median, `−0.22` for 22% under.
    pub change: f64,
    /// The bucket median, in miles.
    pub baseline_miles: i64,
}

/// Every current award that is a deal against the baseline of its own
/// series (`points`, from [`tm_scheduler::awards::award_points`] over
/// the same family, without the check being judged), best first. Awards
/// departing outside `first..=last` are ignored.
pub fn find_awards(
    current: &BTreeMap<AwardDates, &Observation>,
    points: &[PricePoint],
    now: DateTime<Utc>,
    params: &Params,
    first: NaiveDate,
    last: NaiveDate,
) -> Vec<FoundAward> {
    let baselines = compute_by_key(points, now, params.baseline_window);
    let mut out: Vec<FoundAward> = current
        .values()
        .filter(|o| o.departure >= first && o.departure <= last)
        .filter_map(|o| {
            let miles = o.miles?;
            let key = BaselineKey::of(now, o.departure)?;
            let baseline = baselines.get(&key)?;
            let (reason, change) = evaluate(miles, baseline, params)?;
            Some(FoundAward {
                award: (*o).clone(),
                reason,
                change,
                baseline_miles: baseline.median.round() as i64,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.change
            .total_cmp(&b.change)
            .then(a.award.departure.cmp(&b.award.departure))
    });
    out
}

/// Whether an award series already has a deal for these dates: open,
/// or rejected recently.
pub fn known_award(
    deals: &[Deal],
    series: &Series,
    (route, cabin, leg): (RouteId, Cabin, Option<Leg>),
    (departure, return_date): AwardDates,
    now: DateTime<Utc>,
) -> bool {
    deals.iter().any(|d| {
        &d.series == series
            && d.route == route
            && d.cabin == cabin
            && d.leg == leg
            && d.departure == departure
            && d.return_date == return_date
            && still_open(d, now)
    })
}

/// The new award deals worth recording for one series: not already
/// known, at most `max`.
pub fn select_awards(
    found: Vec<FoundAward>,
    deals: &[Deal],
    series: &Series,
    group: (RouteId, Cabin, Option<Leg>),
    now: DateTime<Utc>,
    max: usize,
) -> Vec<FoundAward> {
    found
        .into_iter()
        .filter(|f| {
            let dates = (f.award.departure, f.award.return_date);
            !known_award(deals, series, group, dates, now)
        })
        .take(max)
        .collect()
}

/// An award's flights for a deal or a notice: `LA8148 nonstop`,
/// `LA4776/LA8150, 1 stop`.
pub fn award_flights(award: &Observation) -> Option<String> {
    let label = itinerary_label(award.flights.as_deref()?);
    // The label's time is the departure's; a notice names the flights.
    let flights = label.split(' ').next().unwrap_or(&label).to_owned();
    Some(match award.stops {
        Some(0) => format!("{flights} nonstop"),
        Some(1) => format!("{flights}, 1 stop"),
        Some(n) => format!("{flights}, {n} stops"),
        None => flights,
    })
}

/// The deal for a found award of `program` (0083): its leg, or `None`
/// for the round trip; its taxes as the price; `cash` is the cash fare
/// of the same leg (or trip) and dates, for scale.
#[allow(clippy::too_many_arguments, reason = "one deal's identity")]
pub fn award_candidate(
    found: &FoundAward,
    program: &str,
    leg: Option<Leg>,
    route: RouteId,
    cabin: Cabin,
    pos: &PointOfSale,
    cash: Option<Money>,
    now: DateTime<Utc>,
) -> Deal {
    let a = &found.award;
    let taxes = a
        .price
        .clone()
        .unwrap_or_else(|| Money::new(0, Currency::brl()));
    let detail = a.award.as_ref();
    Deal {
        id: None,
        route,
        cabin,
        pos: pos.id,
        series: Series::Award {
            program: program.to_owned(),
        },
        leg,
        departure: a.departure,
        return_date: if leg.is_some() { None } else { a.return_date },
        price: taxes.clone(),
        effective_brl: taxes,
        baseline: None,
        miles: a.miles,
        baseline_miles: Some(found.baseline_miles),
        brand: detail.and_then(|d| d.brand.clone()),
        seats: detail.and_then(|d| d.seats),
        cash,
        reason: found.reason,
        change: Some(found.change),
        estimated: false,
        status: DealStatus::Candidate,
        found_at: now,
        confirmed_at: None,
        seen: false,
        carrier: award_flights(a),
        seller: None,
        sellers: Vec::new(),
        url: None,
    }
}

/// Whether a confirmed award deal is gone: departed, or its dates now
/// cost [`EXPIRE_RISE`] times more miles in its own series (`current`,
/// the latest check's cheapest award per date).
pub fn award_expired(
    deal: &Deal,
    current: &BTreeMap<AwardDates, &Observation>,
    today: NaiveDate,
) -> bool {
    if deal.departure < today {
        return true;
    }
    let Some(miles) = deal.miles else {
        return false;
    };
    current
        .get(&(deal.departure, deal.return_date))
        .and_then(|o| o.miles)
        .is_some_and(|m| m as f64 > miles as f64 * EXPIRE_RISE)
}

#[cfg(test)]
mod tests {
    use chrono::{Days, TimeZone};
    use tm_domain::{CountryCode, PosId};
    use tm_gflights::rpc::{Seller, SellerOffer};

    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0)
            .single()
            .unwrap_or_default()
    }

    fn date(m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, d).unwrap_or_default()
    }

    fn obs(
        query: i64,
        family: i64,
        observed: DateTime<Utc>,
        dep: NaiveDate,
        ret: Option<NaiveDate>,
        whole: i64,
    ) -> Observation {
        Observation {
            id: None,
            query: QueryId(query),
            family: FamilyId(family),
            observed_at: observed,
            departure: dep,
            return_date: ret,
            price: Some(Money::brl_whole(whole)),
            miles: None,
            carrier: None,
            stops: None,
            duration_minutes: None,
            seller: None,
            flights: None,
            award: None,
        }
    }

    fn pos(home: bool) -> PointOfSale {
        PointOfSale {
            id: PosId(if home { 1 } else { 2 }),
            country: CountryCode::new(if home { "BR" } else { "US" }),
            currency: Currency::brl(),
            is_home: home,
            active: true,
        }
    }

    const GROUP: Group = Group {
        outbound: FamilyId(1),
        ret: FamilyId(2),
        round_trip: Some((FamilyId(3), 7)),
    };

    /// Five weeks of steady legs (R$1,000 each way), then a drop on the
    /// outbound: the new 5–10 day trips are deals; the old ones are not.
    #[test]
    fn synthetic_observations_become_deals() {
        let range = TripRange::default();
        let dep0 = date(12, 1);
        let mut history = Vec::new();
        let mut q = 0;
        for day in 0..36_u64 {
            let observed = at(1, 12) - chrono::TimeDelta::days(36)
                + chrono::TimeDelta::days(day as i64);
            q += 1;
            for i in 0..20_u64 {
                let d = dep0 + Days::new(i);
                history.push(obs(
                    q,
                    1,
                    observed,
                    d,
                    None,
                    1_000 + (day as i64 % 3) * 10,
                ));
                history.push(obs(q + 1000, 2, observed, d, None, 1_000));
            }
            history.push(obs(
                q + 2000,
                3,
                observed,
                dep0,
                Some(dep0 + Days::new(7)),
                1_900,
            ));
        }
        // The check being judged: the outbound dropped 40%.
        let now = at(6, 12);
        let current_query = QueryId(9_999);
        let mut current_out = Vec::new();
        let mut current_ret = Vec::new();
        for i in 0..20_u64 {
            let d = dep0 + Days::new(i);
            let mut o =
                obs(9_999, 1, now, d, None, if i < 2 { 600 } else { 1_000 });
            o.query = current_query;
            history.push(o.clone());
            current_out.push(o);
            current_ret.push(obs(9_998, 2, now, d, None, 1_000));
        }
        let exclude: HashSet<QueryId> = [current_query, QueryId(9_998)].into();
        let points = history_points(&history, &GROUP, range, 0.0, &exclude);
        assert!(points.contains_key(&(7, true)));
        assert!(points.contains_key(&(7, false)));
        assert!(!points.contains_key(&(4, true)));

        let out = leg_prices(&current_out);
        let ret = leg_prices(&current_ret);
        let trips: Vec<(Trip, bool)> = build_trips(&out, &ret, range, 0.0)
            .into_iter()
            .map(|t| (t, true))
            .collect();
        let params = Params::default();
        let found =
            find(&trips, &points, now, &params, date(10, 7), date(12, 31));
        assert!(!found.is_empty());
        for f in &found {
            assert!(f.trip.departure < dep0 + Days::new(2), "{f:?}");
            assert_eq!(f.trip.price_minor, 160_000);
            assert!(f.change < -0.19, "{f:?}");
        }
        assert_eq!(found[0].reason, DealReason::AllTimeLow);

        // Dedupe and limit.
        let picked = select(
            found.clone(),
            &[],
            (RouteId(1), Cabin::Economy, PosId(1)),
            now,
            2,
        );
        assert_eq!(picked.len(), 2);
        assert_ne!(picked[0].trip.departure, picked[1].trip.departure);
        let deal = candidate(
            &picked[0],
            RouteId(1),
            Cabin::Economy,
            &pos(true),
            now,
            &params,
            None,
        );
        assert_eq!(deal.status, DealStatus::Candidate);
        assert_eq!(deal.price, Money::brl_whole(1_600));
        assert_eq!(deal.effective_brl, deal.price);
        assert!(deal.estimated);
        let again = select(
            found,
            std::slice::from_ref(&deal),
            (RouteId(1), Cabin::Economy, PosId(1)),
            now,
            2,
        );
        assert_eq!(again.len(), 1);
        assert!(again.iter().all(|f| f.trip.departure != deal.departure));
    }

    #[test]
    fn too_little_history_finds_nothing() {
        let now = at(6, 12);
        let mut history = Vec::new();
        for i in 0..10_u64 {
            let d = date(12, 1) + Days::new(i);
            history.push(obs(1, 1, at(5, 12), d, None, 1_000));
            history.push(obs(2, 2, at(5, 12), d, None, 1_000));
        }
        let points = history_points(
            &history,
            &GROUP,
            TripRange::default(),
            0.0,
            &HashSet::new(),
        );
        let trip = Trip {
            departure: date(12, 1),
            return_date: date(12, 8),
            days: 7,
            price_minor: 50_000,
            legs_sum_minor: 50_000,
        };
        let found = find(
            &[(trip, true)],
            &points,
            now,
            &Params::default(),
            date(10, 7),
            date(12, 31),
        );
        assert!(found.is_empty());
    }

    fn offer(
        name: &str,
        airline: bool,
        price: i64,
        quoted: Option<(&str, i64)>,
    ) -> SellerOffer {
        SellerOffer {
            sellers: vec![Seller {
                code: name.to_uppercase(),
                name: name.into(),
                is_airline: airline,
            }],
            price: Some(price),
            exact: None,
            quoted: quoted.map(|(c, a)| Money::from_whole(a, Currency::new(c))),
            flights: vec![],
        }
    }

    #[test]
    fn a_confirmation_holds_within_three_percent() {
        let params = Params::default();
        let conf = Confirmation {
            cheapest: Some(3_700),
            carrier: Some("TP".into()),
            search_url: "s".into(),
            booking_url: Some("b".into()),
            offers: vec![
                offer("TAP Air Portugal", true, 4_122, None),
                offer("Flightnetwork", false, 3_728, Some(("USD", 746))),
                offer("BudgetAir", false, 3_739, Some(("USD", 748))),
            ],
        };
        let v =
            judge(360_000, &conf, &pos(false), &FxTable::default(), &params)
                .expect("a verdict");
        assert!(v.held, "3,700 ≤ 3,600 × 1.03");
        assert_eq!(v.quotes.len(), 3);
        assert_eq!(v.rates.len(), 1);
        assert_eq!(v.rates[0].0, Currency::new("USD"));
        assert!((v.rates[0].1 - 4.9984).abs() < 0.001, "{:?}", v.rates);
        // The USD seller costs about 3,728 × 1.035 × 1.01 = 3,897; TAP
        // charges BRL.
        assert!(
            (3_896..=3_899).contains(&v.effective.whole()),
            "{:?}",
            v.effective
        );
        assert_eq!(v.seller.as_deref(), Some("Flightnetwork"));

        let v =
            judge(350_000, &conf, &pos(false), &FxTable::default(), &params)
                .expect("a verdict");
        assert!(!v.held, "3,700 > 3,500 × 1.03");

        let empty = Confirmation::default();
        assert!(
            judge(350_000, &empty, &pos(true), &FxTable::default(), &params)
                .is_none()
        );
        let bare = Confirmation {
            cheapest: Some(1_000),
            ..Confirmation::default()
        };
        let v = judge(100_000, &bare, &pos(true), &FxTable::default(), &params)
            .expect("a verdict");
        assert_eq!(v.effective, Money::brl_whole(1_000));
        assert_eq!(v.seller, None);
    }

    #[test]
    fn deals_expire() {
        let mut deal = candidate(
            &Found {
                trip: Trip {
                    departure: date(12, 1),
                    return_date: date(12, 8),
                    days: 7,
                    price_minor: 100_000,
                    legs_sum_minor: 100_000,
                },
                estimated: true,
                reason: DealReason::BelowMedian,
                change: -0.3,
                baseline_minor: 150_000,
            },
            RouteId(1),
            Cabin::Economy,
            &pos(true),
            at(6, 0),
            &Params::default(),
            None,
        );
        deal.status = DealStatus::Confirmed;
        let mut now = BTreeMap::new();
        assert!(!expired(&deal, &now, date(10, 6)));
        // An award deal on the same dates is not judged by cash prices.
        let award = Deal {
            series: Series::Award {
                program: "latam_pass".into(),
            },
            ..deal.clone()
        };
        assert!(expired(&deal, &now, date(12, 2)));
        now.insert((date(12, 1), date(12, 8)), 105_000);
        assert!(!expired(&deal, &now, date(10, 6)));
        now.insert((date(12, 1), date(12, 8)), 115_000);
        assert!(expired(&deal, &now, date(10, 6)));
        assert!(!expired(&award, &now, date(10, 6)));
        assert!(expired(&award, &now, date(12, 2)));
    }

    #[test]
    fn cells_to_maps() {
        let cells = vec![
            obs(1, 1, at(6, 0), date(11, 1), None, 900),
            obs(1, 1, at(6, 0), date(11, 2), Some(date(11, 9)), 1_800),
            Observation {
                price: None,
                ..obs(1, 1, at(6, 0), date(11, 3), None, 0)
            },
        ];
        assert_eq!(
            leg_prices(&cells).into_iter().collect::<Vec<_>>(),
            vec![(date(11, 1), 90_000)]
        );
        let rt = round_trip_prices(&cells);
        assert_eq!(rt.get(&(date(11, 2), date(11, 9))), Some(&180_000));
        let trips = real_trips(&rt);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].days, 7);
    }

    fn award(
        query: i64,
        family: i64,
        observed: DateTime<Utc>,
        dep: NaiveDate,
        ret: Option<NaiveDate>,
        miles: i64,
    ) -> Observation {
        Observation {
            miles: Some(miles),
            price: Some(Money::new(6861, Currency::brl())),
            stops: Some(0),
            flights: Some("LA8148@2026-12-01T22:40".into()),
            award: Some(tm_domain::AwardDetail {
                fare: None,
                brand: Some("LIGHT".into()),
                fare_basis: None,
                seats: Some(tm_domain::Seats {
                    count: 2,
                    low: false,
                }),
            }),
            ..obs(query, family, observed, dep, ret, 0)
        }
    }

    /// Five weeks of LATAM Pass awards around 64,000 miles one way, then
    /// a check at 45,000: a miles deal in its own series, never set
    /// against cash (0023, 0083).
    #[test]
    fn awards_are_deals_against_their_own_series() {
        use tm_scheduler::awards::{award_points, cheapest_by_dates};
        let dep = date(12, 1);
        let mut history = Vec::new();
        for day in 0..36_i64 {
            let observed = at(1, 12) - chrono::TimeDelta::days(36 - day);
            // A dearer brand of the same check never sets the price.
            history.push(award(
                day,
                1,
                observed,
                dep,
                None,
                63_848 + day % 3 * 100,
            ));
            history.push(award(day, 1, observed, dep, None, 81_449));
            // The round-trip series is a series of its own.
            history.push(award(
                day + 500,
                3,
                observed,
                dep,
                Some(date(12, 8)),
                110_000,
            ));
        }
        let now = at(6, 12);
        let current = vec![
            award(999, 1, now, dep, None, 45_000),
            award(999, 1, now, dep, None, 71_786),
        ];
        let exclude: HashSet<QueryId> = [QueryId(999)].into();
        let points = award_points(&history, FamilyId(1), &exclude);
        assert_eq!(points.len(), 36);
        assert!(points.iter().all(|p| p.price_minor < 64_100));
        let cheapest = cheapest_by_dates(&current);
        let params = Params::default();
        let found = find_awards(
            &cheapest,
            &points,
            now,
            &params,
            date(10, 7),
            date(12, 31),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].award.miles, Some(45_000));
        assert_eq!(found[0].reason, DealReason::AllTimeLow);
        assert!(found[0].change < -0.29, "{:?}", found[0]);

        let series = Series::Award {
            program: "latam_pass".into(),
        };
        let group = (RouteId(1), Cabin::Economy, Some(Leg::Outbound));
        let picked = select_awards(found.clone(), &[], &series, group, now, 2);
        let deal = award_candidate(
            &picked[0],
            "latam_pass",
            Some(Leg::Outbound),
            RouteId(1),
            Cabin::Economy,
            &pos(true),
            Some(Money::brl_whole(1_907)),
            now,
        );
        assert_eq!(deal.series, series);
        assert_eq!(deal.return_date, None);
        assert_eq!(deal.miles, Some(45_000));
        assert_eq!(deal.price, Money::new(6861, Currency::brl()));
        assert_eq!(deal.baseline, None);
        assert!(
            deal.baseline_miles
                .is_some_and(|b| (63_800..64_100).contains(&b))
        );
        assert_eq!(deal.carrier.as_deref(), Some("LA8148 nonstop"));
        assert_eq!(deal.brand.as_deref(), Some("LIGHT"));
        assert_eq!(deal.cash, Some(Money::brl_whole(1_907)));
        // Known now: not picked again; a cash deal on the same date is
        // still free to be found, and the other leg too.
        let again = select_awards(
            found.clone(),
            std::slice::from_ref(&deal),
            &series,
            group,
            now,
            2,
        );
        assert!(again.is_empty());
        assert!(!known(
            std::slice::from_ref(&deal),
            (RouteId(1), Cabin::Economy, PosId(1)),
            dep,
            now
        ));
        let other_leg = (RouteId(1), Cabin::Economy, Some(Leg::Return));
        assert_eq!(
            select_awards(
                found,
                std::slice::from_ref(&deal),
                &series,
                other_leg,
                now,
                2
            )
            .len(),
            1
        );

        // It expires when its miles rise 10%, or once departed.
        let mut latest = BTreeMap::new();
        let dearer = award(1000, 1, now, dep, None, 50_000);
        latest.insert((dep, None), &dearer);
        assert!(award_expired(&deal, &latest, date(10, 6)));
        let held = award(1000, 1, now, dep, None, 46_000);
        latest.insert((dep, None), &held);
        assert!(!award_expired(&deal, &latest, date(10, 6)));
        assert!(award_expired(&deal, &latest, date(12, 2)));
    }

    #[test]
    fn award_flights_read_as_a_label() {
        let mut a = award(1, 1, at(6, 0), date(12, 1), None, 1);
        a.flights =
            Some("LA4776@2026-11-10T00:05/LA8150@2026-11-10T05:55".into());
        a.stops = Some(1);
        assert_eq!(award_flights(&a).as_deref(), Some("LA4776/LA8150, 1 stop"));
        a.flights = None;
        assert_eq!(award_flights(&a), None);
    }
}
