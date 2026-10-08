//! Where trips start and end, who sells them, and in which cabin.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{PlaceId, PosId, RouteId, money::Currency};

/// An ISO 3166-1 alpha-2 country code, upper case (`"BR"`).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CountryCode(pub String);

impl CountryCode {
    pub fn new(code: &str) -> Self {
        Self(code.trim().to_ascii_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CountryCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How Google Flights names a place in `tfs` (docs/research/google-flights.md).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaceKind {
    /// A single airport, by IATA code (`tfs` place type 1).
    Airport,
    /// A metropolitan area such as SAO, by Freebase ID (`tfs` place
    /// type 2). The ID is learned once, with a `q=` search, when the
    /// place is added (DECISIONS.md 0041); `None` until then.
    City { freebase_id: Option<String> },
}

/// An origin or a destination (DECISIONS.md 0035).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub id: PlaceId,
    /// IATA airport or city code, upper case (`"GRU"`, `"SAO"`).
    pub code: String,
    pub name: String,
    pub country: CountryCode,
    pub kind: PlaceKind,
    /// Coordinates, for the map.
    pub lat: f64,
    pub lon: f64,
}

impl Place {
    /// The identifier `tfs` needs, if known: the airport code, or the
    /// city's Freebase ID.
    pub fn google_id(&self) -> Option<&str> {
        match &self.kind {
            PlaceKind::Airport => Some(&self.code),
            PlaceKind::City { freebase_id } => freebase_id.as_deref(),
        }
    }
}

/// An origin × destination pair. Removing a route keeps its history
/// (`active = false`); adding it back continues the same series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub id: RouteId,
    pub origin: Place,
    pub destination: Place,
    /// The scheduler's weight `w` for this route (default 1.0).
    pub weight: f64,
    pub active: bool,
    /// Its destination is a favorite: checked more often and judged
    /// with the lower favorite deal threshold (DECISIONS.md 0094).
    /// Always false for an Explore sweep.
    #[serde(default)]
    pub favorite: bool,
}

impl Route {
    /// Both ends in the same country (DECISIONS.md 0026, 0035).
    pub fn is_domestic(&self) -> bool {
        self.origin.country == self.destination.country
    }

    /// The cabins searched on this route: economy everywhere, premium
    /// economy and business on international routes (0026).
    pub fn cabins(&self) -> &'static [Cabin] {
        if self.is_domestic() {
            &[Cabin::Economy]
        } else {
            &[Cabin::Economy, Cabin::PremiumEconomy, Cabin::Business]
        }
    }
}

/// A seat class, numbered as in `tfs` field 9.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Cabin {
    Economy = 1,
    PremiumEconomy = 2,
    Business = 3,
    First = 4,
}

impl Cabin {
    pub const ALL: [Cabin; 4] = [
        Cabin::Economy,
        Cabin::PremiumEconomy,
        Cabin::Business,
        Cabin::First,
    ];

    /// The `tfs` and RPC number.
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|cabin| cabin.code() == code)
    }

    pub fn label(self) -> &'static str {
        match self {
            Cabin::Economy => "Economy",
            Cabin::PremiumEconomy => "Premium economy",
            Cabin::Business => "Business",
            Cabin::First => "First",
        }
    }
}

/// A point of sale: the market a price is quoted in, set with Google's
/// `gl` parameter (DECISIONS.md 0033).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PointOfSale {
    pub id: PosId,
    /// The `gl` value (`"BR"`, `"US"`, `"PT"`, `"IN"`).
    pub country: CountryCode,
    /// The currency prices are requested in. Foreign points of sale
    /// still request BRL for display; sellers may quote their own
    /// currency, which is recorded on each seller quote.
    pub currency: Currency,
    /// The user's home market (BR): no IOF or card spread applies.
    pub is_home: bool,
    pub active: bool,
}
