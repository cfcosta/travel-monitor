//! What a query saw, and the deals made from it.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    Cabin,
    DealId,
    FamilyId,
    Money,
    ObservationId,
    PlaceId,
    PosId,
    QueryId,
    RouteId,
};

/// One cell: a price for one departure date (and return date, for round
/// trips). Observations are append-only (DECISIONS.md 0020).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub id: Option<ObservationId>,
    pub query: QueryId,
    pub family: FamilyId,
    pub observed_at: DateTime<Utc>,
    pub departure: NaiveDate,
    /// Set for round trips.
    pub return_date: Option<NaiveDate>,
    /// `None` when the source showed the date with no price (sold out,
    /// no flights).
    pub price: Option<Money>,
    /// Miles programs: the miles part; `price` is then the taxes.
    pub miles: Option<i64>,
    pub carrier: Option<String>,
    pub stops: Option<u8>,
    pub duration_minutes: Option<u32>,
    pub seller: Option<String>,
}

impl Observation {
    /// Days between the observation and the departure.
    pub fn days_to_departure(&self) -> i64 {
        (self.departure - self.observed_at.date_naive()).num_days()
    }
}

/// One seller's price on a booking page, under one point of sale
/// (DECISIONS.md 0034).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SellerQuote {
    pub pos: PosId,
    pub seller: String,
    /// The airline itself (as opposed to an online agency).
    pub is_airline: bool,
    /// As displayed (in the requested display currency).
    pub price: Money,
    /// In the seller's own currency, when the page shows it (`$746`).
    pub quoted: Option<Money>,
}

/// One destination from an Explore sweep (DECISIONS.md 0025, 0041).
/// Trips are "about a week" with one date pair: a hint, not a series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExploreHint {
    pub query: QueryId,
    pub origin: PlaceId,
    pub cabin: Cabin,
    pub pos: PosId,
    pub observed_at: DateTime<Utc>,
    /// The destination's Freebase ID.
    pub destination_freebase_id: String,
    /// The airport Explore priced (`"LIS"`).
    pub destination_airport: String,
    pub price: Money,
    pub departure: Option<NaiveDate>,
    pub return_date: Option<NaiveDate>,
    pub stops: Option<u8>,
    pub duration_minutes: Option<u32>,
    pub carrier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DealStatus {
    /// Found, waiting for the confirming query (DECISIONS.md 0031).
    Candidate,
    /// The second query held; the daemon notifies (0080).
    Confirmed,
    /// The second query did not hold.
    Rejected,
    /// Confirmed earlier, no longer available.
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DealReason {
    /// Well below the bucket's median (DECISIONS.md 0030).
    BelowMedian,
    /// Lower than every earlier observation in the bucket.
    AllTimeLow,
}

/// A trip that is a deal: a specific date pair (DECISIONS.md 0043).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deal {
    pub id: Option<DealId>,
    pub route: RouteId,
    pub cabin: Cabin,
    pub pos: PosId,
    pub departure: NaiveDate,
    pub return_date: NaiveDate,
    /// The round-trip price (as displayed, usually BRL).
    pub price: Money,
    /// What it costs in BRL with IOF and card spread (0038).
    pub effective_brl: Money,
    /// The bucket median it was compared with.
    pub baseline: Option<Money>,
    pub reason: DealReason,
    /// `price / baseline − 1`, negative for drops (−0.19).
    pub change: Option<f64>,
    /// Estimated from the two legs, or read as a real round trip.
    pub estimated: bool,
    pub status: DealStatus,
    pub found_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
    /// Its notification was shown (0080).
    pub seen: bool,
    pub carrier: Option<String>,
    pub seller: Option<String>,
    /// Every seller per point of sale, from the booking page (0034).
    pub sellers: Vec<SellerQuote>,
    /// The search URL that shows it.
    pub url: Option<String>,
}
