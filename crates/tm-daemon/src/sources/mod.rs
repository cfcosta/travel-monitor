//! Where prices come from, behind one small interface so the daemon's
//! glue is tested with fakes (DECISIONS.md 0047).
//!
//! [`browser::BrowserSources`] is the real one: Google Flights through
//! the page's own RPCs (0032), TAP through its booking site's own
//! calendar and availability calls (0051), both in the daemon's
//! headless Chromium (0021).

pub mod browser;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{Cabin, Currency, Money, PointOfSale, PosId, QueryOutcome};
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
}

impl<T> Fetch<T> {
    pub fn ok(value: T, page_loads: u32) -> Self {
        Self {
            outcome: QueryOutcome::Ok,
            value,
            page_loads,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.outcome == QueryOutcome::Ok
    }
}

impl<T: Default> Fetch<T> {
    pub fn failed(outcome: QueryOutcome, page_loads: u32) -> Self {
        Self {
            outcome,
            value: T::default(),
            page_loads,
        }
    }

    pub fn error(detail: impl Into<String>) -> Self {
        Self::failed(
            QueryOutcome::Error {
                detail: detail.into(),
            },
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

/// One TAP leg (0051): the deep link for one direction, read through
/// the page's own responses.
///
/// Economy reads the month calendar from the "Datas" step, month by
/// month until `horizon_end`, keeping the days TAP stored since
/// `fresh_since`; the searched date's live economy price is added from
/// the availability search. Economy Prime and business have no
/// calendar: they read the availability search of `departure` only.
#[derive(Debug, Clone, PartialEq)]
pub struct TapJob {
    pub pos: PosId,
    /// The deep link (`sites::tap::Deeplink`).
    pub url: String,
    pub origin: String,
    pub destination: String,
    pub cabin: Cabin,
    /// The deep link's date: the first departure for economy, the date
    /// to price for the other cabins.
    pub departure: NaiveDate,
    /// The first departure worth recording (tomorrow).
    pub first: NaiveDate,
    /// The last departure the calendar should reach.
    pub horizon_end: NaiveDate,
    /// The most calendar months to read.
    pub max_months: u32,
    /// Calendar days stored by TAP before this are stale (0051).
    pub fresh_since: DateTime<Utc>,
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
///
/// A site runs several fetches at once (0072), each on its own page, so
/// every method takes `&self`. The worker never starts or stops the
/// browser while a fetch runs.
#[async_trait]
pub trait Sources: Send + Sync {
    /// Starts the browser with one isolated context per point of sale
    /// (0033).
    async fn start(&self, points_of_sale: &[PointOfSale])
    -> anyhow::Result<()>;
    /// Closes the browser, releasing the profile (0024).
    async fn stop(&self);
    fn is_running(&self) -> bool;
    async fn calendar(&self, job: &CalendarJob) -> Fetch<Vec<Cell>>;
    async fn explore(&self, job: &ExploreJob)
    -> Fetch<Vec<ExploreDestination>>;
    async fn tap(&self, job: &TapJob) -> Fetch<Vec<Cell>>;
    async fn confirm(&self, job: &ConfirmJob) -> Fetch<Option<Confirmation>>;
    async fn resolve(&self, job: &ResolveJob) -> Fetch<Option<String>>;
}
