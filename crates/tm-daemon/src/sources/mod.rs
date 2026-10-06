//! Where prices come from, behind one small interface so the daemon's
//! glue is tested with fakes (DECISIONS.md 0047).
//!
//! [`browser::BrowserSources`] is the real one: Google Flights through
//! the page's own RPCs (0032), Jev-driven airline and miles sites
//! (0041), all in the daemon's headless Chromium (0021).

pub mod browser;

use async_trait::async_trait;
use chrono::NaiveDate;
use tm_browser::agent::Task;
use tm_domain::{Currency, Money, PointOfSale, PosId, QueryOutcome};
use tm_gflights::{
    Locale,
    Search,
    rpc::{ExploreDestination, SellerOffer},
};

/// One price a source showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub departure: NaiveDate,
    pub return_date: Option<NaiveDate>,
    pub price: Option<Money>,
    pub miles: Option<i64>,
    pub carrier: Option<String>,
    pub stops: Option<u8>,
    pub duration_minutes: Option<u32>,
    pub seller: Option<String>,
}

impl Cell {
    pub fn priced(
        departure: NaiveDate,
        return_date: Option<NaiveDate>,
        price: Option<Money>,
    ) -> Self {
        Self {
            departure,
            return_date,
            price,
            miles: None,
            carrier: None,
            stops: None,
            duration_minutes: None,
            seller: None,
        }
    }
}

/// What one fetch did: how it ended, what it found, and what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetch<T> {
    pub outcome: QueryOutcome,
    pub value: T,
    /// Page loads: navigations plus every price-graph page (0039).
    pub page_loads: u32,
    pub jev_calls: u32,
}

impl<T> Fetch<T> {
    pub fn ok(value: T, page_loads: u32, jev_calls: u32) -> Self {
        Self {
            outcome: QueryOutcome::Ok,
            value,
            page_loads,
            jev_calls,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.outcome == QueryOutcome::Ok
    }
}

impl<T: Default> Fetch<T> {
    pub fn failed(
        outcome: QueryOutcome,
        page_loads: u32,
        jev_calls: u32,
    ) -> Self {
        Self {
            outcome,
            value: T::default(),
            page_loads,
            jev_calls,
        }
    }

    pub fn error(detail: impl Into<String>) -> Self {
        Self::failed(
            QueryOutcome::Error {
                detail: detail.into(),
            },
            0,
            0,
        )
    }
}

/// A price graph to read: one leg one way, or round trips of a fixed
/// length (0032, 0042).
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarJob {
    pub pos: PosId,
    pub url: String,
    pub search: Search,
    /// Stop scrolling once the graph's requests reach this departure.
    pub horizon_end: NaiveDate,
    /// The most graph pages to read (the first plus scrolls).
    pub max_pages: u32,
    /// The display currency the URL asks for.
    pub display: Currency,
}

/// An Explore sweep (0025).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExploreJob {
    pub pos: PosId,
    pub url: String,
}

/// An airline or miles search driven by Jev (0041, 0047).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentJob {
    /// The rate-limit site, also the seller tag (`airline:LA`).
    pub site: String,
    pub start_url: String,
    pub task: Task,
    pub departure: NaiveDate,
    pub return_date: Option<NaiveDate>,
    /// A miles program: read miles, and watch for a login wall (0024).
    pub miles: bool,
    /// The carrier to record (the airline's code).
    pub carrier: Option<String>,
}

/// The second query of a deal candidate (0031): a round-trip search
/// for those exact dates under the deal's point of sale, then that
/// point of sale's own booking page (0034).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmJob {
    pub pos: PosId,
    pub search: Search,
    pub locale: Locale,
    /// Replace `https://www.google.com` in URLs (tests).
    pub origin_override: Option<String>,
}

/// What a confirmation saw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Confirmation {
    /// The cheapest round trip on the results page, whole units of the
    /// display currency.
    pub cheapest: Option<i64>,
    /// The cheapest itinerary's carrier (`"TP"`, `"multi"`).
    pub carrier: Option<String>,
    pub search_url: String,
    pub booking_url: Option<String>,
    /// Every seller on the booking page, empty when it did not load.
    pub offers: Vec<SellerOffer>,
}

/// A `q=` search that learns a city's Freebase ID (0041).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveJob {
    pub pos: PosId,
    pub url: String,
}

/// Every source the daemon reads, and the browser's life cycle.
#[async_trait]
pub trait Sources: Send {
    /// Starts the browser with one isolated context per point of sale
    /// (0033).
    async fn start(
        &mut self,
        points_of_sale: &[PointOfSale],
    ) -> anyhow::Result<()>;
    /// Closes the browser, releasing the profile (0024).
    async fn stop(&mut self);
    fn is_running(&self) -> bool;
    fn has_jev(&self) -> bool;
    async fn calendar(&mut self, job: &CalendarJob) -> Fetch<Vec<Cell>>;
    async fn explore(
        &mut self,
        job: &ExploreJob,
    ) -> Fetch<Vec<ExploreDestination>>;
    async fn agent_search(&mut self, job: &AgentJob) -> Fetch<Vec<Cell>>;
    async fn confirm(
        &mut self,
        job: &ConfirmJob,
    ) -> Fetch<Option<Confirmation>>;
    async fn resolve(&mut self, job: &ResolveJob) -> Fetch<Option<String>>;
}
