//! Everything the app reads and writes, behind one [`Backend`].
//!
//! A [`Snapshot`] holds all a frame needs. Backends build it off the
//! interface's thread: [`store::StoreBackend`] from the daemon's SQLite
//! file through `tm-store`, [`fake::FakeBackend`] from sample data shaped
//! like the designs (`tm --fake`). The views only ever see snapshots, so
//! they are developed and tested without a database.
//!
//! Each backend also says when its data changed ([`Changes`], 0057): the
//! store watches SQLite's `data_version`, the fake is told by tests. The
//! window refreshes on those notices, not on a fixed timer.

pub mod awards;
pub mod detail;
pub mod fake;
pub mod found;
pub mod history;
pub mod store;

use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{
    AwardPrice,
    Cabin,
    CountryCode,
    Currency,
    DealId,
    Leg,
    MilesUnit,
    Money,
    Place,
    PlaceId,
    PlaceKind,
    PointOfSale,
    PosId,
    ProgramSettings,
    Route,
    RouteId,
    Seats,
    Settings,
    SiteLimits,
};
use tokio::sync::watch;

use crate::grid::{Calendar, TripGrid};

/// A row of the list: one route in one cabin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RowKey {
    pub route: RouteId,
    pub cabin: Cabin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowStatus {
    /// A confirmed deal (0031).
    Deal,
    /// A candidate waiting for its confirming query.
    Confirming,
    /// No deal: the route is watched.
    Watching,
}

impl RowStatus {
    pub fn label(self) -> &'static str {
        match self {
            RowStatus::Deal => "Deal",
            RowStatus::Confirming => "Confirming",
            RowStatus::Watching => "Watching",
        }
    }

    /// Deals and candidates are listed under "Deals".
    pub fn is_deal(self) -> bool {
        !matches!(self, RowStatus::Watching)
    }
}

/// A specific date pair and what it costs (effective BRL, 0038).
#[derive(Debug, Clone, PartialEq)]
pub struct Trip {
    pub departure: NaiveDate,
    pub return_date: NaiveDate,
    pub price: Money,
    pub carrier: Option<String>,
    pub stops: Option<u8>,
    pub duration_minutes: Option<u32>,
    /// Estimated from the two legs (0042) rather than read as a round
    /// trip.
    pub estimated: bool,
}

impl Trip {
    pub fn days(&self) -> u16 {
        (self.return_date - self.departure).num_days().max(0) as u16
    }

    /// `TAP nonstop`, `Copa, 1 stop`.
    pub fn carrier_label(&self) -> Option<String> {
        let carrier = self.carrier.as_deref()?;
        Some(match self.stops {
            Some(0) => format!("{carrier} nonstop"),
            Some(n) => format!("{carrier}, {}", crate::format::stops(n)),
            None => carrier.to_owned(),
        })
    }
}

/// How the list and the card pay (0083): cash, miles, or both, each
/// series with its own "% vs usual" (0023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PayWith {
    #[default]
    Both,
    Cash,
    Miles,
}

impl PayWith {
    pub const ALL: [PayWith; 3] =
        [PayWith::Both, PayWith::Cash, PayWith::Miles];

    pub fn label(self) -> &'static str {
        match self {
            PayWith::Both => "Both",
            PayWith::Cash => "Cash",
            PayWith::Miles => "Miles",
        }
    }
}

/// One award as a program priced it (0083): one brand of one
/// itinerary, one way or a round trip.
#[derive(Debug, Clone, PartialEq)]
pub struct AwardQuote {
    pub program: String,
    pub unit: MilesUnit,
    pub price: AwardPrice,
    /// The program's own fare before taxes, BRL.
    pub fare: Option<Money>,
    /// `LIGHT`, `PREMIUM BUSINESS STANDARD`.
    pub brand: Option<String>,
    pub cabin: Cabin,
    /// The itinerary key (`LA8146@2026-11-10T17:40`).
    pub flights: Option<String>,
    pub stops: Option<u8>,
    pub seats: Option<Seats>,
    pub departure: NaiveDate,
    /// Set for a round-trip award.
    pub return_date: Option<NaiveDate>,
    /// The leg of a one-way award.
    pub leg: Option<Leg>,
    pub observed_at: DateTime<Utc>,
}

impl AwardQuote {
    /// The implied value of a mile: fare before taxes ÷ miles × 1,000.
    pub fn rate(&self) -> Option<f64> {
        tm_scheduler::awards::implied_rate(
            self.price.miles,
            self.fare.as_ref()?,
        )
    }

    /// `LA8146 17:40`, `LA4776/LA8150 00:05`.
    pub fn flight_label(&self) -> Option<String> {
        self.flights.as_deref().map(tm_domain::itinerary_label)
    }

    /// `LA8146 nonstop`, `LA8070, 1 stop`: flights and stops.
    pub fn route_label(&self) -> Option<String> {
        let label = self.flight_label()?;
        let flights = label.split(' ').next().unwrap_or(&label).to_owned();
        Some(match self.stops {
            Some(0) => format!("{flights} nonstop"),
            Some(n) => format!("{flights}, {}", crate::format::stops(n)),
            None => flights,
        })
    }
}

/// A cash trip's dates in awards: both legs one way, each from its
/// latest check, and the round-trip award when one was searched.
#[derive(Debug, Clone, PartialEq)]
pub struct AwardTrip {
    pub departure: NaiveDate,
    pub return_date: NaiveDate,
    pub outbound: Option<AwardQuote>,
    pub inbound: Option<AwardQuote>,
    pub round_trip: Option<AwardQuote>,
}

impl AwardTrip {
    /// Both ways known: the round trip, or both legs.
    pub fn complete(&self) -> bool {
        self.round_trip.is_some()
            || (self.outbound.is_some() && self.inbound.is_some())
    }

    /// What the whole trip costs in miles: the round-trip award, else
    /// the cheaper of it and the two legs; `None` until both ways are
    /// known.
    pub fn price(&self) -> Option<AwardPrice> {
        let legs = match (&self.outbound, &self.inbound) {
            (Some(o), Some(i)) => o.price.plus(&i.price),
            _ => None,
        };
        let rt = self.round_trip.as_ref().map(|q| q.price.clone());
        match (rt, legs) {
            (Some(a), Some(b)) => Some(if b.miles < a.miles { b } else { a }),
            (a, b) => a.or(b),
        }
    }

    pub fn any(&self) -> bool {
        self.outbound.is_some()
            || self.inbound.is_some()
            || self.round_trip.is_some()
    }
}

/// What a row knows about its leading program's awards (0083).
#[derive(Debug, Clone, PartialEq)]
pub struct AwardSummary {
    pub program: String,
    /// `LATAM Pass`.
    pub name: String,
    /// The cheapest award on sale for the route and cabin: the
    /// outbound leg's, else the return's.
    pub best: AwardQuote,
    /// The row's own cash trip dates, in awards.
    pub trip: Option<AwardTrip>,
    /// The award series' bucket median for `best`, in miles.
    pub baseline_miles: Option<i64>,
    /// `best` against it: the miles' own "% vs usual".
    pub change: Option<f64>,
    /// A miles deal, of the award series alone (0023).
    pub status: RowStatus,
    pub deal: Option<DealId>,
    pub found_at: Option<DateTime<Utc>>,
    /// The cheapest award each day, in miles, oldest first.
    pub history: Vec<f64>,
    /// Days of award checks behind `history`.
    pub history_days: usize,
    /// `best`'s implied value of a mile, BRL per 1,000.
    pub rate: Option<f64>,
    /// The program's usual rate: the median of its recent checks.
    pub usual_rate: Option<f64>,
    /// `rate` is far from the usual one: a discount, or a dearer class.
    pub unusual: bool,
    /// The cheapest cash fare of `best`'s leg and date (Google Flights,
    /// any airline), for scale.
    pub cash: Option<Money>,
    pub value_per_1000: Option<Money>,
    pub buy_per_1000: Option<Money>,
}

impl AwardSummary {
    /// `best`'s rate against `cash`: what its miles save per 1,000.
    pub fn rate_against_cash(&self) -> Option<f64> {
        tm_scheduler::awards::rate_against_cash(
            &self.best.price,
            self.cash.as_ref()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteRow {
    pub route: Route,
    pub cabin: Cabin,
    pub status: RowStatus,
    /// The cheapest trip in range (the deal's own trip for deals).
    pub best: Option<Trip>,
    /// The usual price it is compared with (bucket median, 0030).
    pub baseline: Option<Money>,
    /// `best / baseline − 1`.
    pub change: Option<f64>,
    /// Daily minimum prices, oldest first, for the sparkline.
    pub history: Vec<f64>,
    pub last_checked: Option<DateTime<Utc>>,
    pub deal: Option<DealId>,
    pub found_at: Option<DateTime<Utc>>,
    /// The leading miles program's awards, when it priced this route
    /// and cabin (0083).
    pub award: Option<AwardSummary>,
}

impl RouteRow {
    /// Listed under "Deals" when paying `pay`: a cash deal, a miles
    /// deal, or either.
    pub fn is_deal(&self, pay: PayWith) -> bool {
        let miles = self.award.as_ref().is_some_and(|a| a.status.is_deal());
        match pay {
            PayWith::Both => self.status.is_deal() || miles,
            PayWith::Cash => self.status.is_deal(),
            PayWith::Miles => miles,
        }
    }

    /// The award leads the row: paying in miles, or a miles deal where
    /// cash has none.
    pub fn award_leads(&self, pay: PayWith) -> bool {
        let Some(award) = &self.award else {
            return false;
        };
        match pay {
            PayWith::Miles => true,
            PayWith::Cash => false,
            PayWith::Both => award.status.is_deal() && !self.status.is_deal(),
        }
    }

    /// The award, shown when paying `pay` allows it.
    pub fn award_for(&self, pay: PayWith) -> Option<&AwardSummary> {
        self.award.as_ref().filter(|_| pay != PayWith::Cash)
    }

    /// The leading series' change against its own usual.
    pub fn lead_change(&self, pay: PayWith) -> Option<f64> {
        if self.award_leads(pay) {
            self.award.as_ref().and_then(|a| a.change)
        } else {
            self.change
        }
    }

    /// The great-circle distance from origin to destination, one way.
    pub fn distance_km(&self) -> f64 {
        let (o, d) = (&self.route.origin, &self.route.destination);
        crate::geo::great_circle_km(o.lat, o.lon, d.lat, d.lon)
    }

    /// What the best trip costs per kilometre flown: its price over the
    /// round trip's distance (there and back). `None` without a price,
    /// or for places too close to measure.
    pub fn price_per_km(&self) -> Option<f64> {
        let trip = self.best.as_ref()?;
        let km = 2.0 * self.distance_km();
        (km >= 1.0).then(|| trip.price.as_f64() / km)
    }

    pub fn key(&self) -> RowKey {
        RowKey {
            route: self.route.id,
            cabin: self.cabin,
        }
    }

    pub fn code(&self) -> &str {
        &self.route.destination.code
    }

    /// The destination, which a star makes a favorite (0094).
    pub fn place(&self) -> PlaceId {
        self.route.destination.id
    }

    pub fn city(&self) -> &str {
        &self.route.destination.name
    }
}

/// The cheapest seller under one point of sale (0033, 0034).
#[derive(Debug, Clone, PartialEq)]
pub struct SellerRow {
    pub pos: CountryCode,
    pub seller: String,
    /// In the seller's own currency, when shown (`US$746`).
    pub quoted: Option<Money>,
    /// With IOF and the card spread (0038).
    pub effective: Money,
    pub is_airline: bool,
}

/// One query of the log, as the activity views show it.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityRow {
    /// The query log's row id: what tells a new row from an old one.
    pub id: i64,
    pub at: DateTime<Utc>,
    /// `SAO → LIS`, or `SAO → anywhere` for Explore.
    pub route: String,
    /// `Price graph · Economy · BR`.
    pub query: String,
    /// `Scheduled · score 0.41`, `Confirm deal`, `Random sample`.
    pub reason: String,
    /// `R$3,932 holds`, `no change`, `blocked`.
    pub result: String,
    pub tone: Tone,
    pub loads: Option<u32>,
    pub running: bool,
}

/// How a line of activity reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// A deal or a drop (yellow).
    Deal,
    /// The system's own doing (teal).
    System,
    /// A block or an error (red).
    Bad,
    Neutral,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockRow {
    pub at: DateTime<Utc>,
    pub site: String,
    pub detail: String,
    pub loads_last_hour: u32,
}

/// A line of the overview's activity feed.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedItem {
    pub at: DateTime<Utc>,
    pub text: String,
    pub tone: Tone,
}

/// The daemon's heartbeat, its sites' budgets and checks, and blocks
/// (0039, 0057, 0060, 0062), for the status chip and the activity
/// screen.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DaemonStatus {
    pub heartbeat: Option<DateTime<Utc>>,
    /// The process's state: `"running"`, `"starting"`, `"stopped"`.
    pub state: String,
    pub next_check: Option<String>,
    /// When the first idle site looks for work again (0057).
    pub next_check_at: Option<DateTime<Utc>>,
    /// The checks running now, at most one per site (0062), Google
    /// Flights first.
    pub checks: Vec<CurrentCheck>,
    /// Each site's budget and state, Google Flights first (0060).
    pub sites: Vec<SiteRow>,
    /// Loads today and today's budgets, over every site.
    pub loads_today: u32,
    pub budget: u32,
    pub blocks_today: u32,
}

/// One site as the daemon runs it: its budget today and what its worker
/// is doing (0060, 0062).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SiteRow {
    /// `google_flights`, `airline:TP`, `miles:smiles`.
    pub site: String,
    /// `Google`, `TAP`, `Smiles`.
    pub label: String,
    /// `idle`, `checking`, `cooling down`, `paused for login: smiles`,
    /// `browser unavailable`, `stopped`; empty when the site has no
    /// worker.
    pub state: String,
    pub loads_today: u32,
    /// Today's budget; `None` for a site that never spent a load.
    pub budget: Option<u32>,
    /// The most its ramp may reach.
    pub max_budget: Option<u32>,
}

impl SiteRow {
    /// `Google 1,234/4,800`, for a site with a budget.
    pub fn loads(&self) -> Option<String> {
        self.budget.map(|b| {
            format!(
                "{} {}/{}",
                self.label,
                crate::format::thousands(self.loads_today),
                crate::format::thousands(b)
            )
        })
    }

    pub fn is_paused(&self) -> bool {
        self.state.starts_with("paused")
    }
}

/// The check the daemon is running, as the status chip and the activity
/// screen show it (0057).
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentCheck {
    /// Its row in the query log, which shows as running meanwhile.
    pub query: Option<i64>,
    /// `SAO→LIS`, `SAO→anywhere` for Explore.
    pub route: String,
    pub cabin: Cabin,
    /// `outbound`, `return`, `round trip 7 d`, `explore`, `confirm`.
    pub kind: String,
    /// The point of sale's country: `BR`.
    pub pos: String,
    /// Another site than Google Flights, by its label: `TAP`.
    pub site: Option<String>,
    /// An award check (0083): one load reads every cabin, so the chip
    /// names the program instead of the cabin and point of sale.
    pub award: bool,
    /// `Scheduled · score 0.41`.
    pub reason: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Running,
    /// No heartbeat for a while.
    Stale,
    /// Never seen.
    Missing,
}

/// A heartbeat older than this reads as stale.
pub const STALE_AFTER_SECS: i64 = 120;

/// A check runs without a heartbeat until it ends; one longer than this
/// reads as stale.
pub const CHECK_STALE_AFTER_SECS: i64 = 600;

/// What the daemon is doing, as of the window's clock (0057).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Live<'a> {
    /// Never seen.
    Missing,
    /// No heartbeat for a while; when the last one was.
    Stale(DateTime<Utc>),
    /// Stopped or starting: the daemon's own words.
    Paused(&'a str),
    /// One check per busy site (0062), never empty.
    Checking(&'a [CurrentCheck]),
    /// No site checking; when the first looks for work again, if known.
    Idle(Option<DateTime<Utc>>),
}

impl DaemonStatus {
    pub fn health(&self, now: DateTime<Utc>) -> Health {
        let limit = if self.checking().is_empty() {
            STALE_AFTER_SECS
        } else {
            CHECK_STALE_AFTER_SECS
        };
        match self.heartbeat {
            None => Health::Missing,
            Some(at) if (now - at).num_seconds() > limit => Health::Stale,
            Some(_) => Health::Running,
        }
    }

    /// The checks in progress, trusted only while the daemon runs (a
    /// stopped daemon's sites say so themselves, but an old row might
    /// not).
    pub fn checking(&self) -> &[CurrentCheck] {
        if matches!(self.state.as_str(), "running" | "checking") {
            &self.checks
        } else {
            &[]
        }
    }

    /// The sites with a budget, Google Flights first.
    pub fn budgets(&self) -> impl Iterator<Item = &SiteRow> {
        self.sites.iter().filter(|s| s.budget.is_some())
    }

    /// The sites paused for a login (0024, 0062).
    pub fn paused_sites(&self) -> impl Iterator<Item = &SiteRow> {
        self.sites.iter().filter(|s| s.is_paused())
    }

    pub fn live(&self, now: DateTime<Utc>) -> Live<'_> {
        match (self.health(now), self.heartbeat) {
            (Health::Missing, _) | (_, None) => Live::Missing,
            (Health::Stale, Some(at)) => Live::Stale(at),
            (Health::Running, Some(_)) => {
                let checks = self.checking();
                if !checks.is_empty() {
                    Live::Checking(checks)
                } else if matches!(self.state.as_str(), "running" | "") {
                    Live::Idle(self.next_check_at)
                } else {
                    Live::Paused(&self.state)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Login {
    LoggedIn,
    LoggedOut,
    Expired,
}

/// A miles program and its session in the daemon's profile (0023,
/// 0024), with what the user set for it (0083).
#[derive(Debug, Clone, PartialEq)]
pub struct MilesRow {
    pub program: String,
    pub name: String,
    pub login: Login,
    pub last_checked: Option<DateTime<Utc>>,
    /// When the session ends, as the profile's cookies say.
    pub session_until: Option<DateTime<Utc>>,
    pub settings: ProgramSettings,
    /// The daemon reads this program's awards (0050).
    pub has_reader: bool,
    pub unit: MilesUnit,
    /// The measured value of a mile: the median of the last 30 days'
    /// checks, BRL per 1,000, and how many checks it rests on.
    pub usual_rate: Option<f64>,
    pub rate_checks: usize,
}

impl MilesRow {
    pub fn active(&self) -> bool {
        self.settings.active
    }

    /// Active, but the session is gone: award checks wait for a login.
    pub fn needs_login(&self) -> bool {
        self.active() && self.has_reader && self.login != Login::LoggedIn
    }

    /// `miles:latam_pass`.
    pub fn site(&self) -> String {
        tm_domain::Source::Miles {
            program: self.program.clone(),
        }
        .site()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExploreSummary {
    pub last_sweep: Option<DateTime<Utc>>,
    pub destinations: u32,
    /// Destinations it priced that nobody watches (0087).
    pub found: Vec<found::FoundRow>,
}

/// The selected row in depth: the detail card and the route detail.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteDetail {
    pub key: RowKey,
    /// The trip the card shows; the grid is centred on it.
    pub trip: Option<Trip>,
    pub grid: TripGrid,
    /// Trips of `calendar.days` days over six months.
    pub calendar: Calendar,
    pub sellers: Vec<SellerRow>,
    /// The airline's own price and name (0034).
    pub airline_direct: Option<(String, Money)>,
    pub url: Option<String>,
    pub lowest_ever: Option<(Money, NaiveDate, String)>,
    /// The bucket median for the trip's days to departure, and the
    /// bucket's label (`21–45 days`).
    pub usual: Option<(Money, String)>,
    /// Learned changes per day.
    pub change_rate: Option<f64>,
    pub recent_checks: Vec<ActivityRow>,
    pub next_check: Option<DateTime<Utc>>,
    /// The leading miles program's awards for this route (0083).
    pub awards: Option<RouteAwards>,
}

/// One program's awards in the route detail (0083): its tiles and the
/// brands of the selected dates, one series per leg and the round trip,
/// never merged with cash.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteAwards {
    pub program: String,
    pub name: String,
    pub unit: MilesUnit,
    pub login: Login,
    /// The cheapest award on sale.
    pub cheapest: Option<AwardQuote>,
    /// The fewest miles ever seen in the outbound series, and the day.
    pub lowest_ever: Option<(i64, NaiveDate)>,
    /// The award series' median for `cheapest`, and its bucket.
    pub usual: Option<(i64, String)>,
    pub checks: usize,
    pub checked_at: Option<DateTime<Utc>>,
    /// Departures with an award on either leg, out of the horizon.
    pub dates_known: usize,
    pub dates_total: usize,
    /// The selected departure's outbound brands, its return's, and the
    /// round trip's, fewest miles first.
    pub outbound: Vec<AwardQuote>,
    pub inbound: Vec<AwardQuote>,
    pub round_trip: Vec<AwardQuote>,
    /// Other cabins' brands from the outbound's load.
    pub same_load: Vec<AwardQuote>,
    /// The program's usual value of a mile, BRL per 1,000.
    pub usual_rate: Option<tm_scheduler::awards::UsualRate>,
    pub unusual_threshold: f64,
    /// The cheapest cash fare of the selected outbound leg and date.
    pub cash_outbound: Option<Money>,
    pub value_per_1000: Option<Money>,
    pub buy_per_1000: Option<Money>,
}

impl RouteAwards {
    /// Whether `quote`'s rate is far from the program's usual one.
    pub fn unusual(&self, quote: &AwardQuote) -> bool {
        match (quote.rate(), &self.usual_rate) {
            (Some(rate), Some(usual)) => tm_scheduler::awards::is_unusual(
                rate,
                usual,
                self.unusual_threshold,
            ),
            _ => false,
        }
    }
}

/// Everything a frame needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub taken_at: DateTime<Utc>,
    pub today: NaiveDate,
    pub origins: Vec<Place>,
    /// The origin the rows are for.
    pub origin: Option<PlaceId>,
    pub destinations: Vec<Place>,
    /// The destinations the user starred (0094): every route to one is
    /// a favorite, in every cabin, cash and miles.
    pub favorites: BTreeSet<PlaceId>,
    pub points_of_sale: Vec<PointOfSale>,
    pub settings: Settings,
    pub miles: Vec<MilesRow>,
    pub rows: Vec<RouteRow>,
    pub daemon: DaemonStatus,
    pub activity: Vec<ActivityRow>,
    pub blocks: Vec<BlockRow>,
    pub feed: Vec<FeedItem>,
    pub explore: ExploreSummary,
    pub detail: Option<RouteDetail>,
}

impl Snapshot {
    pub fn origin_place(&self) -> Option<&Place> {
        self.origin
            .and_then(|id| self.origins.iter().find(|p| p.id == id))
            .or_else(|| self.origins.first())
    }

    pub fn row(&self, key: RowKey) -> Option<&RouteRow> {
        self.rows.iter().find(|r| r.key() == key)
    }

    /// Whether the user starred this destination (0094).
    pub fn is_favorite(&self, place: PlaceId) -> bool {
        self.favorites.contains(&place)
    }
}

/// What the next snapshot is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapshotQuery {
    pub origin: Option<PlaceId>,
    pub selected: Option<RowKey>,
    /// The calendar's trip length; `None` takes the round-trip length
    /// from the settings.
    pub calendar_days: Option<u16>,
    /// The calendar's selected departure.
    pub departure: Option<NaiveDate>,
}

/// Work for the daemon (0046).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    CheckRoute(RouteId),
    ConfirmDeal(DealId),
    /// Price a trip's legs in miles on the logged-in programs (0083).
    CheckAward {
        route: RouteId,
        cabin: Cabin,
        departure: NaiveDate,
        return_date: Option<NaiveDate>,
    },
    /// Pause the program's site for a login window (0024, 0062).
    OpenLogin(String),
    /// The program's login window closed.
    CloseLogin(String),
    ReloadConfig,
}

/// An enqueued request, to follow until the daemon finishes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ticket(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketState {
    Pending,
    Running,
    Done,
    Failed,
}

/// Origin or destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Origin,
    Destination,
}

/// A place typed in the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPlace {
    pub code: String,
    pub name: String,
    pub country: CountryCode,
    pub kind: PlaceKind,
    pub lat: f64,
    pub lon: f64,
}

/// A point of sale typed in the settings.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPos {
    pub country: CountryCode,
    pub currency: Currency,
}

/// Notices that a backend's data changed (0057). Every clone sees every
/// notice; a burst of notices between two looks reads as one.
#[derive(Debug, Clone)]
pub struct Changes(watch::Receiver<u64>);

/// The sending end of [`Changes`], kept by a backend.
#[derive(Debug, Clone)]
pub struct ChangeSender(watch::Sender<u64>);

impl Changes {
    pub fn channel() -> (ChangeSender, Changes) {
        let (tx, rx) = watch::channel(0);
        (ChangeSender(tx), Changes(rx))
    }

    /// Waits for a change not seen yet, and marks it seen. `false` once
    /// the backend stopped watching.
    pub async fn next(&mut self) -> bool {
        let changed = self.0.changed().await.is_ok();
        self.0.mark_unchanged();
        changed
    }

    /// Whether a change arrived since the last look; marks it seen.
    pub fn take(&mut self) -> bool {
        let changed = self.0.has_changed().unwrap_or(false);
        self.0.mark_unchanged();
        changed
    }
}

impl ChangeSender {
    pub fn notify(&self) {
        self.0.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// How many notices were sent: it moves with every write noticed.
    pub fn count(&self) -> u64 {
        *self.0.borrow()
    }

    pub fn subscribe(&self) -> Changes {
        Changes(self.0.subscribe())
    }
}

/// Where snapshots come from and edits go. Every method may do I/O; the
/// app runs them on background tasks, never on the interface's thread.
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn snapshot(&self, query: SnapshotQuery) -> anyhow::Result<Snapshot>;
    /// Notices that the data changed, from whoever wrote it (0057).
    /// Watching starts on the first call, off the interface's thread.
    fn changes(&self) -> Changes;
    async fn enqueue(&self, request: Request) -> anyhow::Result<Ticket>;
    async fn ticket(&self, ticket: Ticket) -> anyhow::Result<TicketState>;
    async fn save_settings(&self, settings: Settings) -> anyhow::Result<()>;
    async fn add_place(
        &self,
        role: Role,
        place: NewPlace,
    ) -> anyhow::Result<()>;
    /// Stops new checks; history is kept (0035).
    async fn remove_place(
        &self,
        role: Role,
        place: PlaceId,
    ) -> anyhow::Result<()>;
    async fn add_pos(&self, pos: NewPos) -> anyhow::Result<()>;
    async fn set_pos_active(
        &self,
        pos: PosId,
        active: bool,
    ) -> anyhow::Result<()>;
    /// What the user set for a miles program (0083).
    async fn save_program(
        &self,
        program: String,
        settings: ProgramSettings,
    ) -> anyhow::Result<()>;
    /// A site's daily budget and its most (0060), such as a program's
    /// award loads.
    async fn set_site_limits(
        &self,
        site: String,
        limits: SiteLimits,
    ) -> anyhow::Result<()>;
    /// Stars or unstars a destination (0094).
    async fn set_favorite(
        &self,
        place: PlaceId,
        favorite: bool,
    ) -> anyhow::Result<()>;
}

pub type SharedBackend = Arc<dyn Backend>;
