//! What a query saw, and the deals made from it.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    AwardDetail,
    AwardPrice,
    Cabin,
    DealId,
    FamilyId,
    Leg,
    Money,
    ObservationId,
    PlaceId,
    PosId,
    QueryId,
    RouteId,
    Seats,
    Series,
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
    /// The itinerary, when the source lists its segments: every
    /// segment's flight and local departure
    /// (`LA4776@2026-11-10T00:05/LA8150@2026-11-10T05:55`, as the LATAM
    /// reader's `Itinerary::key`). Two observations with the same key
    /// are the same itinerary, so an award can be set against the cash
    /// price of its own flights (0083); the first flight alone is not
    /// enough.
    pub flights: Option<String>,
    /// The brand, seats and fare before taxes of an award (0083).
    pub award: Option<AwardDetail>,
}

impl Observation {
    /// Days between the observation and the departure.
    pub fn days_to_departure(&self) -> i64 {
        (self.departure - self.observed_at.date_naive()).num_days()
    }

    /// The award this observation priced: its miles plus its taxes.
    pub fn award_price(&self) -> Option<AwardPrice> {
        Some(AwardPrice::new(self.miles?, self.price.clone()?))
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
    /// Explore's name for the destination (`"Lisbon"`), its country in
    /// words (`"Portugal"`) and its coordinates (latitude, longitude),
    /// when the response carried them (DECISIONS.md 0087).
    pub destination_name: Option<String>,
    pub destination_country: Option<String>,
    pub destination_coordinates: Option<(f64, f64)>,
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

/// A trip that is a deal: a specific date pair (DECISIONS.md 0043), or
/// for an award one leg on one date (0083).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deal {
    pub id: Option<DealId>,
    pub route: RouteId,
    pub cabin: Cabin,
    pub pos: PosId,
    /// Cash or a miles program's awards; each has its own baselines
    /// (0023).
    pub series: Series,
    /// The leg of a one-way award deal; `None` for a round trip.
    pub leg: Option<Leg>,
    /// The leg's date for a one-way deal.
    pub departure: NaiveDate,
    /// `None` for a one-way award deal (0083).
    pub return_date: Option<NaiveDate>,
    /// The round-trip price (as displayed, usually BRL); for an award,
    /// its taxes.
    pub price: Money,
    /// What it costs in BRL with IOF and card spread (0038); for an
    /// award, its taxes.
    pub effective_brl: Money,
    /// The bucket median it was compared with (cash deals).
    pub baseline: Option<Money>,
    /// An award's miles, and the bucket median of the award series it
    /// was compared with (0030, 0083).
    pub miles: Option<i64>,
    pub baseline_miles: Option<i64>,
    /// An award's brand and seats left at its price.
    pub brand: Option<String>,
    pub seats: Option<Seats>,
    /// The cheapest cash fare for the same leg and date when an award
    /// deal was found: shown for scale, never compared (0023, 0083).
    pub cash: Option<Money>,
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
    /// The airline, or an award deal's flights (`LA8148 nonstop`).
    pub carrier: Option<String>,
    pub seller: Option<String>,
    /// Every seller per point of sale, from the booking page (0034).
    pub sellers: Vec<SellerQuote>,
    /// The search URL that shows it.
    pub url: Option<String>,
}

impl Deal {
    /// The award it is, for an award deal.
    pub fn award_price(&self) -> Option<AwardPrice> {
        Some(AwardPrice::new(self.miles?, self.price.clone()))
    }
}
