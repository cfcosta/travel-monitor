//! Sample data shaped like the designs (`tm --fake`): the sixteen
//! routes of "G · Map + list" with their prices, drops, carriers and
//! sellers, legs and baselines that make a believable calendar, and an
//! activity log. Edits change the in-memory state, so the settings
//! screen works too.
//!
//! Tests stand in for the daemon: [`FakeBackend::land_query`],
//! [`FakeBackend::set_checking`] and [`FakeBackend::check_route`] change
//! the data and send a change notice, as a commit would (0057).

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use anyhow::{Context as _, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use tm_domain::{
    Cabin,
    CountryCode,
    Currency,
    DealId,
    Money,
    Place,
    PlaceId,
    PlaceKind,
    PointOfSale,
    PosId,
    Route,
    RouteId,
    Settings,
    effective_brl,
};

use super::{
    ActivityRow,
    Backend,
    BlockRow,
    ChangeSender,
    Changes,
    CurrentCheck,
    DaemonStatus,
    ExploreSummary,
    FeedItem,
    Login,
    MilesRow,
    NewPlace,
    NewPos,
    Request,
    Role,
    RouteRow,
    RowStatus,
    SellerRow,
    SiteRow,
    Snapshot,
    SnapshotQuery,
    Ticket,
    TicketState,
    Tone,
    Trip,
    detail::{Baselines, DetailInputs, assemble},
};
use crate::grid::{LegPrices, TripInputs};

/// One row of the designs' sample.
struct Sample {
    code: &'static str,
    city: &'static str,
    country: &'static str,
    city_code: bool,
    lat: f64,
    lon: f64,
    price: i64,
    drop: i64,
    status: RowStatus,
    cabin: Cabin,
    /// Days from today to the departure.
    offset: i64,
    carrier: &'static str,
    stops: u8,
    seller: &'static str,
    checked_min: i64,
    duration_min: u32,
}

#[allow(clippy::too_many_arguments, reason = "one sample row per call")]
const fn s(
    code: &'static str,
    city: &'static str,
    country: &'static str,
    city_code: bool,
    (lat, lon): (f64, f64),
    (price, drop): (i64, i64),
    status: RowStatus,
    cabin: Cabin,
    offset: i64,
    (carrier, stops, seller): (&'static str, u8, &'static str),
    checked_min: i64,
    duration_min: u32,
) -> Sample {
    Sample {
        code,
        city,
        country,
        city_code,
        lat,
        lon,
        price,
        drop,
        status,
        cabin,
        offset,
        carrier,
        stops,
        seller,
        checked_min,
        duration_min,
    }
}

use Cabin::{Business, Economy};
use RowStatus::{Confirming, Deal, Watching};

#[rustfmt::skip]
const SAMPLES: [Sample; 16] = [
    s("LIS", "Lisbon", "PT", false, (38.72, -9.14), (3932, -19), Deal, Economy, 35, ("TAP", 0, "Kiwi.com"), 11, 595),
    s("MIA", "Miami", "US", false, (25.79, -80.29), (2482, -24), Deal, Economy, 149, ("LATAM", 0, "LATAM"), 60, 500),
    s("CUN", "Cancún", "MX", false, (21.04, -86.88), (2053, -15), Deal, Economy, 160, ("Copa", 1, "Copa"), 180, 720),
    s("SSA", "Salvador", "BR", false, (-12.97, -38.51), (916, -27), Deal, Economy, 20, ("GOL", 0, "GOL"), 40, 140),
    s("BUE", "Buenos Aires", "AR", true, (-34.60, -58.38), (1572, -21), Confirming, Economy, 46, ("Aerolíneas", 0, "Aerolíneas"), 25, 175),
    s("MAD", "Madrid", "ES", false, (40.42, -3.70), (14870, -22), Deal, Business, 135, ("Iberia", 0, "Iberia"), 180, 610),
    s("RIO", "Rio de Janeiro", "BR", true, (-22.91, -43.17), (718, -6), Watching, Economy, 60, ("LATAM", 0, "LATAM"), 120, 60),
    s("LIM", "Lima", "PE", false, (-12.05, -77.04), (1491, -8), Watching, Economy, 9, ("LATAM", 0, "LATAM"), 300, 330),
    s("NYC", "New York", "US", true, (40.71, -74.01), (2557, -9), Watching, Economy, 149, ("United", 1, "United"), 360, 720),
    s("MCO", "Orlando", "US", false, (28.43, -81.31), (2615, -5), Watching, Economy, 143, ("Azul", 0, "Azul"), 240, 530),
    s("SCL", "Santiago", "CL", false, (-33.45, -70.67), (1830, 4), Watching, Economy, 28, ("SKY", 0, "SKY"), 420, 240),
    s("PAR", "Paris", "FR", true, (48.86, 2.35), (3777, 1), Watching, Economy, 16, ("Air France", 0, "Air France"), 120, 685),
    s("LON", "London", "GB", true, (51.51, -0.13), (4310, 6), Watching, Economy, 42, ("BA", 0, "British Airways"), 540, 700),
    s("DXB", "Dubai", "AE", false, (25.20, 55.27), (5980, -3), Watching, Economy, 98, ("Emirates", 0, "Emirates"), 720, 885),
    s("TYO", "Tokyo", "JP", true, (35.68, 139.69), (7640, 2), Watching, Economy, 105, ("Emirates", 1, "Emirates"), 1440, 1500),
    s("REC", "Recife", "BR", false, (-8.05, -34.88), (1180, 3), Watching, Economy, 31, ("Azul", 0, "Azul"), 480, 190),
];

/// A small deterministic generator (the mockups' own LCG).
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed % 233_280)
    }

    fn next(&mut self) -> f64 {
        self.0 = (self.0 * 9301 + 49_297) % 233_280;
        self.0 as f64 / 233_280.0
    }
}

fn seed_of(code: &str) -> u64 {
    code.bytes().fold(7, |acc, b| acc * 31 + u64::from(b))
}

struct FakeState {
    origins: Vec<Place>,
    destinations: Vec<Place>,
    removed: HashSet<PlaceId>,
    pos: Vec<PointOfSale>,
    settings: Settings,
    miles: Vec<MilesRow>,
    tickets: HashMap<i64, (Request, u32)>,
    next_ticket: i64,
    next_place: i64,
    requests: Vec<Request>,
    /// Queries that landed after the sample log, newest last.
    landed: Vec<ActivityRow>,
    /// The checks the daemon is running, one per site (0062).
    checking: Vec<CurrentCheck>,
    /// Minutes taken off a destination's last check (`check_route`).
    rechecked: HashMap<String, i64>,
    snapshots: u32,
}

/// The fake backend.
pub struct FakeBackend {
    state: Mutex<FakeState>,
    /// A fixed clock for tests; the real one otherwise.
    now: Option<DateTime<Utc>>,
    changes: ChangeSender,
}

fn place(
    id: i64,
    code: &str,
    name: &str,
    country: &str,
    city: bool,
    lat: f64,
    lon: f64,
) -> Place {
    Place {
        id: PlaceId(id),
        code: code.to_owned(),
        name: name.to_owned(),
        country: CountryCode::new(country),
        kind: if city {
            PlaceKind::City { freebase_id: None }
        } else {
            PlaceKind::Airport
        },
        lat,
        lon,
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeBackend {
    pub fn new() -> Self {
        let origins = vec![
            place(1, "SAO", "São Paulo", "BR", true, -23.55, -46.63),
            place(2, "VCP", "Campinas", "BR", false, -23.01, -47.13),
        ];
        let destinations = SAMPLES
            .iter()
            .enumerate()
            .map(|(i, s)| {
                place(
                    10 + i as i64,
                    s.code,
                    s.city,
                    s.country,
                    s.city_code,
                    s.lat,
                    s.lon,
                )
            })
            .collect();
        let pos = [("BR", true), ("US", false), ("PT", false), ("IN", false)]
            .into_iter()
            .enumerate()
            .map(|(i, (country, home))| PointOfSale {
                id: PosId(i as i64 + 1),
                country: CountryCode::new(country),
                currency: Currency::brl(),
                is_home: home,
                active: true,
            })
            .collect();
        let miles = vec![
            MilesRow {
                program: "smiles".into(),
                name: "Smiles".into(),
                login: Login::LoggedIn,
                last_checked: None,
            },
            MilesRow {
                program: "latam_pass".into(),
                name: "LATAM Pass".into(),
                login: Login::Expired,
                last_checked: None,
            },
            MilesRow {
                program: "azul_fidelidade".into(),
                name: "Azul Fidelidade".into(),
                login: Login::LoggedOut,
                last_checked: None,
            },
        ];
        Self {
            state: Mutex::new(FakeState {
                origins,
                destinations,
                removed: HashSet::new(),
                pos,
                settings: Settings::default(),
                miles,
                tickets: HashMap::new(),
                next_ticket: 1,
                next_place: 100,
                requests: Vec::new(),
                landed: Vec::new(),
                checking: vec![sample_check()],
                rechecked: HashMap::new(),
                snapshots: 0,
            }),
            now: None,
            changes: Changes::channel().0,
        }
    }

    /// The same data at a fixed time, for tests.
    #[cfg(test)]
    pub fn at(now: DateTime<Utc>) -> Self {
        Self {
            now: Some(now),
            ..Self::new()
        }
    }

    /// The requests enqueued so far.
    #[cfg(test)]
    pub fn requests(&self) -> Vec<Request> {
        self.lock().requests.clone()
    }

    /// How many snapshots were taken.
    #[cfg(test)]
    pub fn snapshots(&self) -> u32 {
        self.lock().snapshots
    }

    /// A query finished, as the daemon would log it; returns its id.
    #[cfg(test)]
    pub fn land_query(&self, route: &str, result: &str) -> i64 {
        let id = {
            let mut state = self.lock();
            let id = 2000 + state.landed.len() as i64;
            state.landed.push(ActivityRow {
                id,
                at: self.now(),
                route: route.to_owned(),
                query: "Outbound leg · Economy · BR".to_owned(),
                reason: "Scheduled · score 0.52".to_owned(),
                result: result.to_owned(),
                tone: Tone::Neutral,
                loads: Some(2),
                running: false,
            });
            id
        };
        self.changes.notify();
        id
    }

    /// The daemon started (or, with `None`, ended) a check.
    #[cfg(test)]
    pub fn set_checking(&self, check: Option<CurrentCheck>) {
        self.set_checks(check.into_iter().collect());
    }

    /// The checks the daemon's sites are running now (0062).
    #[cfg(test)]
    pub fn set_checks(&self, checks: Vec<CurrentCheck>) {
        self.lock().checking = checks;
        self.changes.notify();
    }

    /// New observations for the route to `code`: its last check moves
    /// to now.
    #[cfg(test)]
    pub fn check_route(&self, code: &str) {
        let minutes = SAMPLES
            .iter()
            .find(|s| s.code == code)
            .map_or(0, |s| s.checked_min);
        self.lock().rechecked.insert(code.to_owned(), minutes);
        self.changes.notify();
    }

    /// A change notice without a change (a write the app does not show).
    #[cfg(test)]
    pub fn touch(&self) {
        self.changes.notify();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        // A poisoned lock still holds consistent data: every write is a
        // single assignment or push.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> DateTime<Utc> {
        self.now.unwrap_or_else(Utc::now)
    }
}

fn usual(s: &Sample) -> i64 {
    (s.price as f64 / (1.0 + s.drop as f64 / 100.0)).round() as i64
}

fn brl(whole: i64) -> Money {
    Money::brl_whole(whole)
}

fn route_of(
    origin: &Place,
    dest: &Place,
    origin_index: i64,
    dest_index: i64,
) -> Route {
    Route {
        id: RouteId(origin_index * 1000 + dest_index),
        origin: origin.clone(),
        destination: dest.clone(),
        weight: 1.0,
        active: true,
    }
}

/// VCP's fares run a little above SAO's.
fn origin_factor(origin: &Place) -> f64 {
    if origin.code == "SAO" { 1.0 } else { 1.06 }
}

fn scaled(whole: i64, f: f64) -> i64 {
    (whole as f64 * f).round() as i64
}

fn history(s: &Sample, f: f64) -> Vec<f64> {
    let mut rng = Lcg::new(seed_of(s.code));
    let base = usual(s) as f64 * f;
    let mut v = base * (1.0 + (rng.next() - 0.5) * 0.1);
    let mut out: Vec<f64> = (0..25)
        .map(|_| {
            v = (v + (rng.next() - 0.5) * base * 0.06)
                .clamp(base * 0.9, base * 1.12);
            v
        })
        .collect();
    out.push(s.price as f64 * f);
    out
}

fn is_domestic(s: &Sample) -> bool {
    s.country == "BR"
}

const WEEKDAY: [f64; 7] = [1.0, 0.96, 0.95, 1.0, 1.06, 1.03, 1.08];

/// Peak season: late December to mid January, and July.
fn season(date: NaiveDate) -> f64 {
    match (date.month(), date.day()) {
        (12, d) if d >= 15 => 1.35,
        (1, d) if d <= 10 => 1.3,
        (7, _) => 1.2,
        _ => 1.0,
    }
}

fn trip_inputs(
    s: &Sample,
    today: NaiveDate,
    horizon: i64,
    f: f64,
) -> TripInputs {
    let discount = if is_domestic(s) { 0.0 } else { 0.10 };
    let half = usual(s) as f64 * f * 100.0 / 2.0 / (1.0 - discount);
    let mut rng = Lcg::new(seed_of(s.code) + 11);
    let mut legs = LegPrices::default();
    for i in 0..=horizon + 12 {
        let day = today + Duration::days(i);
        let wd = WEEKDAY[day.weekday().num_days_from_monday() as usize];
        // Fares climb close to departure, as the baselines do.
        let close = bucket_factor(i);
        let out = half * close * season(day) * wd * (0.92 + rng.next() * 0.16);
        let back = half * close * season(day) * (0.92 + rng.next() * 0.16);
        legs.outbound.insert(day, out.round() as i64);
        legs.inbound.insert(day, back.round() as i64);
    }
    let departure = today + Duration::days(s.offset);
    let mut round_trips = HashMap::new();
    let price = scaled(s.price, f) * 100;
    round_trips.insert((departure, departure + Duration::days(7)), price);
    if s.status.is_deal() {
        for (off, factor) in [(-1, 1.04), (1, 1.02)] {
            let d = departure + Duration::days(off);
            round_trips.insert(
                (d, d + Duration::days(7)),
                (price as f64 * factor).round() as i64,
            );
        }
    }
    TripInputs {
        legs,
        round_trips,
        discount,
    }
}

/// Close-in fares run above the usual, far-out ones a little below,
/// by the scheduler's days-to-departure buckets.
const BUCKET_FACTOR: [f64; 8] = [1.35, 1.25, 1.15, 1.08, 1.0, 0.97, 0.95, 0.95];

fn bucket_factor(days: i64) -> f64 {
    tm_scheduler::DtdBucket::of(days).map_or(1.0, |b| BUCKET_FACTOR[b as usize])
}

fn baselines(s: &Sample, f: f64, today: NaiveDate, horizon: i64) -> Baselines {
    let usual = usual(s) as f64 * f * 100.0;
    Baselines {
        by_departure: (0..=horizon)
            .map(|i| {
                (
                    today + Duration::days(i),
                    (usual * bucket_factor(i)).round() as i64,
                )
            })
            .collect(),
    }
}

/// The designs' sellers: the route's own seller at home, three agencies
/// abroad, each a little cheaper or dearer.
fn sellers(s: &Sample, f: f64, settings: &Settings) -> Vec<SellerRow> {
    let price = scaled(s.price, f);
    [
        ("BR", s.seller, 1.0, "BRL", 1.0),
        ("US", "Flightnetwork", 0.991, "USD", 5.0),
        ("PT", "Expedia", 0.985, "EUR", 5.8),
        ("IN", "BudgetAir", 0.994, "INR", 0.0565),
    ]
    .into_iter()
    .map(|(pos, seller, factor, currency, rate)| {
        let effective = brl(scaled(price, factor));
        let quoted = (currency != "BRL").then(|| {
            let foreign = effective.minor as f64
                / rate
                / (1.0 + settings.iof)
                / (1.0 + settings.card_spread);
            let quoted =
                Money::new(foreign.round() as i64, Currency::new(currency));
            // Re-derive the effective price from the quote, as the store
            // does (0038), so both columns agree.
            (
                effective_brl(
                    &quoted,
                    rate,
                    settings.iof,
                    settings.card_spread,
                ),
                quoted,
            )
        });
        match quoted {
            Some((effective, quoted)) => SellerRow {
                pos: CountryCode::new(pos),
                seller: seller.to_owned(),
                quoted: Some(quoted),
                effective,
                is_airline: false,
            },
            None => SellerRow {
                pos: CountryCode::new(pos),
                seller: seller.to_owned(),
                quoted: None,
                effective,
                is_airline: seller == s.carrier,
            },
        }
    })
    .collect()
}

fn trip_of(s: &Sample, today: NaiveDate, f: f64) -> Trip {
    let departure = today + Duration::days(s.offset);
    Trip {
        departure,
        return_date: departure + Duration::days(7),
        price: brl(scaled(s.price, f)),
        carrier: Some(s.carrier.to_owned()),
        stops: Some(s.stops),
        duration_minutes: Some(s.duration_min),
        estimated: false,
    }
}

/// The running MIA business check of the sample log.
fn sample_check() -> CurrentCheck {
    CurrentCheck {
        query: Some(SAMPLE_RUNNING),
        route: "SAO→MIA".into(),
        cabin: Cabin::Business,
        kind: "outbound".into(),
        pos: "BR".into(),
        site: None,
        reason: "Scheduled · score 0.38".into(),
        // Moved to the snapshot's time.
        started_at: DateTime::<Utc>::MIN_UTC,
    }
}

/// The id of the sample log's running query; ids count down from it.
const SAMPLE_RUNNING: i64 = 1000;

fn activity(origin: &str, now: DateTime<Utc>) -> Vec<ActivityRow> {
    let row = |min: i64,
               dest: &str,
               query: &str,
               reason: &str,
               result: &str,
               tone: Tone,
               loads: u32| ActivityRow {
        at: now - Duration::minutes(min),
        route: if dest.is_empty() {
            format!("{origin} → anywhere")
        } else {
            format!("{origin} → {dest}")
        },
        query: query.to_owned(),
        reason: reason.to_owned(),
        result: result.to_owned(),
        tone,
        loads: Some(loads),
        running: false,
        id: 0,
    };
    let mut rows = vec![
        ActivityRow {
            running: true,
            loads: None,
            result: "running".into(),
            ..row(
                0,
                "MIA",
                "Price graph · Business · BR",
                "Scheduled · score 0.38",
                "",
                Tone::System,
                0,
            )
        },
        row(
            11,
            "LIS",
            "Price graph · Economy · BR",
            "Confirm deal",
            "R$3,932 holds",
            Tone::Deal,
            1,
        ),
        row(
            11,
            "LIS",
            "Booking page · 7 days",
            "Confirm deal",
            "TAP direct R$4,122",
            Tone::Deal,
            1,
        ),
        row(
            23,
            "LIS",
            "Price graph · Economy · BR",
            "Explore hint (−11%)",
            "min −19%",
            Tone::Deal,
            7,
        ),
        row(
            25,
            "BUE",
            "Round trip · Economy · BR",
            "Confirm deal",
            "waiting",
            Tone::System,
            1,
        ),
        row(
            40,
            "SSA",
            "Outbound leg · Economy · BR",
            "Fan-out",
            "min −27%",
            Tone::Deal,
            7,
        ),
        row(
            60,
            "MIA",
            "Round trip · Economy · BR",
            "Confirm deal",
            "R$2,482 holds",
            Tone::Deal,
            1,
        ),
        row(
            95,
            "SCL",
            "Return leg · Economy · US",
            "Random sample (10%)",
            "+2%",
            Tone::Neutral,
            7,
        ),
        row(
            120,
            "RIO",
            "Outbound leg · Economy · BR",
            "Scheduled · score 0.29",
            "no change",
            Tone::Neutral,
            7,
        ),
        row(
            150,
            "MAD",
            "Price graph · Business · PT",
            "Scheduled · score 0.41",
            "no change",
            Tone::Neutral,
            7,
        ),
        row(
            180,
            "CUN",
            "Round trip · Economy · BR",
            "Overdue (7 days)",
            "−15%",
            Tone::Deal,
            7,
        ),
        row(
            200,
            "PAR",
            "Price graph · Economy · IN",
            "Scheduled · score 0.22",
            "unexpected page",
            Tone::Bad,
            2,
        ),
        row(
            240,
            "MCO",
            "Outbound leg · Economy · BR",
            "Scheduled · score 0.25",
            "no change",
            Tone::Neutral,
            7,
        ),
        row(
            300,
            "LIM",
            "Return leg · Economy · BR",
            "Random sample (10%)",
            "−3%",
            Tone::Neutral,
            7,
        ),
        row(
            360,
            "",
            "Explore · Economy · BR",
            "Daily sweep",
            "72 destinations",
            Tone::System,
            3,
        ),
        row(
            361,
            "",
            "Explore · Business · BR",
            "Daily sweep",
            "64 destinations",
            Tone::System,
            3,
        ),
        row(
            420,
            "SCL",
            "Outbound leg · Economy · BR",
            "Scheduled · score 0.19",
            "+4%",
            Tone::Neutral,
            7,
        ),
        row(
            540,
            "LON",
            "Price graph · Economy · BR",
            "Scheduled · score 0.17",
            "+6%",
            Tone::Neutral,
            7,
        ),
        row(
            720,
            "DXB",
            "Price graph · Economy · US",
            "Overdue (7 days)",
            "−3%",
            Tone::Neutral,
            7,
        ),
        row(
            1440,
            "TYO",
            "Price graph · Economy · BR",
            "Bootstrap",
            "first check",
            Tone::System,
            7,
        ),
    ];
    rows.sort_by_key(|r| std::cmp::Reverse(r.at));
    for (i, r) in rows.iter_mut().enumerate() {
        r.id = SAMPLE_RUNNING - i as i64;
    }
    rows
}

fn feed(now: DateTime<Utc>) -> Vec<FeedItem> {
    let item = |min: i64, text: &str, tone: Tone| FeedItem {
        at: now - Duration::minutes(min),
        text: text.to_owned(),
        tone,
    };
    vec![
        item(
            11,
            "Deal confirmed: LIS R$3,932, 19% under usual",
            Tone::Deal,
        ),
        item(
            23,
            "Explore hint: LIS −11%, price graph queued",
            Tone::System,
        ),
        item(60, "Deal confirmed: MIA R$2,482", Tone::Deal),
        item(
            150,
            "Scheduled check: MAD business, no change",
            Tone::Neutral,
        ),
        item(270, "BUE −21%, confirming with a second query", Tone::Deal),
        item(
            360,
            "Explore sweep: 72 destinations, economy and business",
            Tone::System,
        ),
    ]
}

#[async_trait]
impl Backend for FakeBackend {
    fn changes(&self) -> Changes {
        self.changes.subscribe()
    }

    async fn snapshot(&self, query: SnapshotQuery) -> anyhow::Result<Snapshot> {
        let now = self.now();
        let today = now.date_naive();
        let mut state = self.lock();
        state.snapshots += 1;
        let state = state;
        let origin = query
            .origin
            .and_then(|id| state.origins.iter().find(|p| p.id == id))
            .or_else(|| state.origins.first())
            .context("no origin configured")?
            .clone();
        let origin_index = state
            .origins
            .iter()
            .position(|p| p.id == origin.id)
            .unwrap_or(0) as i64;
        let f = origin_factor(&origin);
        let live: Vec<(usize, &Place)> = state
            .destinations
            .iter()
            .enumerate()
            .filter(|(_, p)| !state.removed.contains(&p.id))
            .collect();
        let sample_of =
            |p: &Place| SAMPLES.iter().position(|s| s.code == p.code);
        let mut rows = Vec::new();
        for (di, dest) in &live {
            let route = route_of(&origin, dest, origin_index + 1, *di as i64);
            let Some(si) = sample_of(dest) else {
                // A destination added in the settings: watched, no data yet.
                rows.push(RouteRow {
                    route,
                    cabin: Cabin::Economy,
                    status: RowStatus::Watching,
                    best: None,
                    baseline: None,
                    change: None,
                    history: Vec::new(),
                    last_checked: None,
                    deal: None,
                    found_at: None,
                });
                continue;
            };
            let s = &SAMPLES[si];
            rows.push(RouteRow {
                route,
                cabin: s.cabin,
                status: s.status,
                best: Some(trip_of(s, today, f)),
                baseline: Some(brl(scaled(usual(s), f))),
                change: Some(s.drop as f64 / 100.0),
                history: history(s, f),
                last_checked: Some(
                    now - Duration::minutes(s.checked_min)
                        + Duration::minutes(
                            state.rechecked.get(s.code).copied().unwrap_or(0),
                        ),
                ),
                deal: s.status.is_deal().then_some(DealId(100 + si as i64)),
                found_at: s
                    .status
                    .is_deal()
                    .then(|| now - Duration::minutes(s.checked_min)),
            });
        }
        let settings = state.settings.clone();
        let detail = query.selected.and_then(|key| {
            let row = rows.iter().find(|r| r.key() == key)?;
            let si = sample_of(&row.route.destination)?;
            let s = &SAMPLES[si];
            let days = query.calendar_days.unwrap_or(settings.trips.round_trip_days);
            let airline = s.carrier.to_owned();
            let inputs = DetailInputs {
                trip: row.best.clone(),
                deal: row.status == RowStatus::Deal,
                trips: trip_inputs(s, today, i64::from(settings.horizon_days), f),
                baselines: baselines(s, f, today, i64::from(settings.horizon_days)),
                sellers: sellers(s, f, &settings),
                airline_direct: Some((airline, brl(scaled(scaled(s.price, f), 1.048)))),
                url: Some(format!(
                    "https://www.google.com/travel/flights?q=Flights%20from%20{}%20to%20{}%20on%20{}%20returning%20{}",
                    origin.code,
                    s.code,
                    row.best.as_ref().map(|t| t.departure).unwrap_or(today),
                    row.best.as_ref().map(|t| t.return_date).unwrap_or(today),
                )),
                lowest_ever: Some((
                    brl(scaled(s.price, f * 0.92)),
                    today - Duration::days(1),
                    "US".to_owned(),
                )),
                change_rate: Some(0.8),
                recent_checks: activity(&origin.code, now)
                    .into_iter()
                    .filter(|a| a.route.ends_with(s.code))
                    .collect(),
                next_check: Some(now + Duration::hours(5)),
            };
            Some(assemble(key, inputs, &settings, today, days, query.departure))
        });
        Ok(Snapshot {
            taken_at: now,
            today,
            origins: state.origins.clone(),
            origin: Some(origin.id),
            destinations: live.iter().map(|(_, p)| (*p).clone()).collect(),
            points_of_sale: state.pos.clone(),
            settings,
            miles: state.miles.clone(),
            rows,
            daemon: DaemonStatus {
                heartbeat: Some(now - Duration::seconds(4)),
                state: "running".into(),
                next_check: Some("MIA · Business in 4 min".into()),
                next_check_at: state
                    .checking
                    .is_empty()
                    .then(|| now + Duration::minutes(4)),
                checks: state
                    .checking
                    .iter()
                    .cloned()
                    .map(|mut c| {
                        if c.started_at == DateTime::<Utc>::MIN_UTC {
                            c.started_at = now - Duration::seconds(12);
                        }
                        c
                    })
                    .collect(),
                sites: vec![
                    SiteRow {
                        site: "google_flights".into(),
                        label: "Google".into(),
                        state: if state
                            .checking
                            .iter()
                            .any(|c| c.site.is_none())
                        {
                            "checking".into()
                        } else {
                            "idle".into()
                        },
                        loads_today: 212,
                        budget: Some(state.settings.daily_budget),
                        max_budget: Some(state.settings.max_daily_budget),
                    },
                    SiteRow {
                        site: "airline:TP".into(),
                        label: "TAP".into(),
                        state: if state
                            .checking
                            .iter()
                            .any(|c| c.site.is_some())
                        {
                            "checking".into()
                        } else {
                            "idle".into()
                        },
                        loads_today: 40,
                        budget: Some(300),
                        max_budget: Some(1000),
                    },
                ],
                loads_today: 252,
                budget: state.settings.daily_budget + 300,
                blocks_today: 0,
            },
            activity: {
                let mut rows = activity(&origin.code, now);
                // The sample's running query ends with its check.
                if let Some(r) = rows.first_mut()
                    && r.running
                    && state.checking.is_empty()
                {
                    r.running = false;
                    r.loads = Some(2);
                    r.result = "no change".into();
                    r.tone = Tone::Neutral;
                }
                rows.splice(0..0, state.landed.iter().rev().cloned());
                rows
            },
            blocks: vec![BlockRow {
                at: now - Duration::hours(50),
                site: "google_flights".into(),
                detail: "\"unusual traffic\" page".into(),
                loads_last_hour: 48,
            }],
            feed: feed(now),
            explore: ExploreSummary {
                last_sweep: Some(now - Duration::hours(6)),
                destinations: 72,
            },
            detail,
        })
    }

    async fn enqueue(&self, request: Request) -> anyhow::Result<Ticket> {
        let mut state = self.lock();
        let id = state.next_ticket;
        state.next_ticket += 1;
        state.tickets.insert(id, (request.clone(), 0));
        state.requests.push(request);
        Ok(Ticket(id))
    }

    async fn ticket(&self, ticket: Ticket) -> anyhow::Result<TicketState> {
        let mut state = self.lock();
        let (_, polls) = state
            .tickets
            .get_mut(&ticket.0)
            .ok_or_else(|| anyhow!("no request {}", ticket.0))?;
        *polls += 1;
        Ok(match *polls {
            1 => TicketState::Pending,
            2 => TicketState::Running,
            _ => TicketState::Done,
        })
    }

    async fn save_settings(&self, settings: Settings) -> anyhow::Result<()> {
        settings.validate()?;
        self.lock().settings = settings;
        Ok(())
    }

    async fn add_place(&self, role: Role, new: NewPlace) -> anyhow::Result<()> {
        let mut state = self.lock();
        let id = PlaceId(state.next_place);
        state.next_place += 1;
        let place = Place {
            id,
            code: new.code.to_ascii_uppercase(),
            name: new.name,
            country: new.country,
            kind: new.kind,
            lat: new.lat,
            lon: new.lon,
        };
        match role {
            Role::Origin => state.origins.push(place),
            Role::Destination => {
                // Adding a removed destination back continues its series.
                if let Some(old) = state
                    .destinations
                    .iter()
                    .find(|p| p.code == place.code)
                    .map(|p| p.id)
                {
                    state.removed.remove(&old);
                } else {
                    state.destinations.push(place);
                }
            }
        }
        Ok(())
    }

    async fn remove_place(
        &self,
        role: Role,
        place: PlaceId,
    ) -> anyhow::Result<()> {
        let mut state = self.lock();
        match role {
            Role::Origin => {
                if state.origins.len() <= 1 {
                    anyhow::bail!("at least one origin must stay");
                }
                state.origins.retain(|p| p.id != place);
            }
            Role::Destination => {
                state.removed.insert(place);
            }
        }
        Ok(())
    }

    async fn add_pos(&self, pos: NewPos) -> anyhow::Result<()> {
        let mut state = self.lock();
        if state.pos.iter().any(|p| p.country == pos.country) {
            anyhow::bail!("{} is already a point of sale", pos.country);
        }
        let id = PosId(state.pos.len() as i64 + 1);
        state.pos.push(PointOfSale {
            id,
            country: pos.country,
            currency: pos.currency,
            is_home: false,
            active: true,
        });
        Ok(())
    }

    async fn set_pos_active(
        &self,
        pos: PosId,
        active: bool,
    ) -> anyhow::Result<()> {
        let mut state = self.lock();
        let p = state
            .pos
            .iter_mut()
            .find(|p| p.id == pos)
            .ok_or_else(|| anyhow!("no point of sale {pos}"))?;
        p.active = active;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, 15, 0, 0).unwrap()
    }

    async fn snap(b: &FakeBackend, q: SnapshotQuery) -> Snapshot {
        b.snapshot(q).await.expect("snapshot")
    }

    fn run<F: std::future::Future>(f: F) -> F::Output {
        // The fake never awaits anything that is not ready.
        futures_lite_block_on(f)
    }

    /// Polls a future that completes without waking (the fake's).
    fn futures_lite_block_on<F: std::future::Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut f = std::pin::pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn has_the_designs_rows() {
        let b = FakeBackend::at(now());
        let s = run(snap(&b, SnapshotQuery::default()));
        assert_eq!(s.rows.len(), 16);
        assert_eq!(s.rows.iter().filter(|r| r.status.is_deal()).count(), 6);
        let lis = s.rows.iter().find(|r| r.code() == "LIS").expect("LIS");
        let trip = lis.best.as_ref().expect("trip");
        assert_eq!(trip.price, Money::brl_whole(3932));
        assert_eq!(
            trip.departure,
            NaiveDate::from_ymd_opt(2026, 11, 10).expect("date")
        );
        assert_eq!(trip.carrier_label().as_deref(), Some("TAP nonstop"));
        assert!(s.detail.is_none());
    }

    #[test]
    fn price_per_km_ranks_far_cheap_trips_first() {
        use crate::ui::{Sort, sort_rows};
        let b = FakeBackend::at(now());
        let s = run(snap(&b, SnapshotQuery::default()));
        let position = |rows: &[&RouteRow], code: &str| {
            rows.iter().position(|r| r.code() == code).expect("row")
        };
        let mut by_price: Vec<&RouteRow> = s.rows.iter().collect();
        sort_rows(&mut by_price, Sort::LowestPrice);
        let mut by_km: Vec<&RouteRow> = s.rows.iter().collect();
        sort_rows(&mut by_km, Sort::LowestPricePerKm);
        // Salvador is cheaper, but Lisbon goes over five times as far.
        assert!(position(&by_price, "SSA") < position(&by_price, "LIS"));
        assert!(position(&by_km, "LIS") < position(&by_km, "SSA"));
        let values: Vec<f64> =
            by_km.iter().filter_map(|r| r.price_per_km()).collect();
        assert!(values.windows(2).all(|w| w[0] <= w[1]), "{values:?}");
        // R$3,932 over 2 × 7,949 km.
        let lis = by_km[position(&by_km, "LIS")];
        assert!((lis.price_per_km().expect("priced") - 0.2473).abs() < 1e-3);
    }

    #[test]
    fn detail_follows_the_selection() {
        let b = FakeBackend::at(now());
        let first = run(snap(&b, SnapshotQuery::default()));
        let key = first.rows[0].key();
        let s = run(snap(
            &b,
            SnapshotQuery {
                selected: Some(key),
                ..Default::default()
            },
        ));
        let detail = s.detail.expect("detail");
        assert_eq!(detail.key, key);
        assert_eq!(detail.grid.rows.len(), 5);
        assert_eq!(detail.grid.lengths, vec![5, 6, 7, 8, 9, 10]);
        let centre = &detail.grid.rows[2].cells[2];
        assert!(centre.selected);
        assert_eq!(
            centre.price.map(|p| (p.minor, p.confirmed)),
            Some((393_200, true))
        );
        assert_eq!(detail.calendar.weeks.len(), 26);
        assert_eq!(detail.sellers.len(), 4);
        // PT is the cheapest effective price, as in the designs.
        let cheapest = detail
            .sellers
            .iter()
            .min_by_key(|s| s.effective.minor)
            .expect("sellers");
        assert_eq!(cheapest.pos.as_str(), "PT");
    }

    #[test]
    fn edits_change_the_snapshot() {
        let b = FakeBackend::at(now());
        let s = run(snap(&b, SnapshotQuery::default()));
        let lis = s
            .destinations
            .iter()
            .find(|p| p.code == "LIS")
            .expect("LIS")
            .id;
        run(b.remove_place(Role::Destination, lis)).expect("remove");
        let s = run(snap(&b, SnapshotQuery::default()));
        assert_eq!(s.rows.len(), 15);
        run(b.add_place(
            Role::Destination,
            NewPlace {
                code: "lis".into(),
                name: "Lisbon".into(),
                country: CountryCode::new("PT"),
                kind: PlaceKind::Airport,
                lat: 38.7,
                lon: -9.1,
            },
        ))
        .expect("add back");
        let s = run(snap(&b, SnapshotQuery::default()));
        assert_eq!(s.rows.len(), 16);
        let mut settings = s.settings.clone();
        settings.trips.min_days = 20;
        assert!(run(b.save_settings(settings)).is_err());
    }

    #[test]
    fn tickets_finish() {
        let b = FakeBackend::at(now());
        let t =
            run(b.enqueue(Request::CheckRoute(RouteId(1)))).expect("ticket");
        let states: Vec<_> =
            (0..3).map(|_| run(b.ticket(t)).expect("state")).collect();
        assert_eq!(
            states,
            vec![
                TicketState::Pending,
                TicketState::Running,
                TicketState::Done
            ]
        );
        assert_eq!(b.requests(), vec![Request::CheckRoute(RouteId(1))]);
    }
}
