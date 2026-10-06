//! Everything the app reads and writes, behind one [`Backend`].
//!
//! A [`Snapshot`] holds all a frame needs. Backends build it off the
//! interface's thread: [`store::StoreBackend`] from the daemon's SQLite
//! file through `tm-store`, [`fake::FakeBackend`] from sample data shaped
//! like the designs (`tm --fake`). The views only ever see snapshots, so
//! they are developed and tested without a database.

pub mod detail;
pub mod fake;
pub mod history;
pub mod store;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
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
};

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
}

impl RouteRow {
    pub fn key(&self) -> RowKey {
        RowKey {
            route: self.route.id,
            cabin: self.cabin,
        }
    }

    pub fn code(&self) -> &str {
        &self.route.destination.code
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

/// The daemon's heartbeat, budget and blocks (0039), for the status
/// chip.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DaemonStatus {
    pub heartbeat: Option<DateTime<Utc>>,
    pub state: String,
    pub next_check: Option<String>,
    pub loads_today: u32,
    pub budget: u32,
    pub blocks_today: u32,
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

impl DaemonStatus {
    pub fn health(&self, now: DateTime<Utc>) -> Health {
        match self.heartbeat {
            None => Health::Missing,
            Some(at) if (now - at).num_seconds() > STALE_AFTER_SECS => {
                Health::Stale
            }
            Some(_) => Health::Running,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Login {
    LoggedIn,
    LoggedOut,
    Expired,
}

/// A miles program and its session in the daemon's profile (0023, 0024).
#[derive(Debug, Clone, PartialEq)]
pub struct MilesRow {
    pub program: String,
    pub name: String,
    pub login: Login,
    pub last_checked: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExploreSummary {
    pub last_sweep: Option<DateTime<Utc>>,
    pub destinations: u32,
}

/// A newly confirmed deal the app has not shown (0040).
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub deal: DealId,
    pub title: String,
    pub body: String,
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
    pub points_of_sale: Vec<PointOfSale>,
    pub settings: Settings,
    pub miles: Vec<MilesRow>,
    pub rows: Vec<RouteRow>,
    pub daemon: DaemonStatus,
    pub activity: Vec<ActivityRow>,
    pub blocks: Vec<BlockRow>,
    pub feed: Vec<FeedItem>,
    pub explore: ExploreSummary,
    pub notices: Vec<Notice>,
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
    OpenLogin(String),
    CloseLogin,
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

/// Where snapshots come from and edits go. Every method may do I/O; the
/// app runs them on background tasks, never on the interface's thread.
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn snapshot(&self, query: SnapshotQuery) -> anyhow::Result<Snapshot>;
    async fn enqueue(&self, request: Request) -> anyhow::Result<Ticket>;
    async fn ticket(&self, ticket: Ticket) -> anyhow::Result<TicketState>;
    /// The app has shown these deals (0040).
    async fn mark_seen(&self, deals: Vec<DealId>) -> anyhow::Result<()>;
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
}

pub type SharedBackend = Arc<dyn Backend>;
