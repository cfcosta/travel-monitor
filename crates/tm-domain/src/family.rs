//! Query families: the unit the scheduler scores (DECISIONS.md 0028).
//!
//! A family is one route × cabin × source × point of sale × kind. Each
//! query of a family is one page load (or one price-graph page) that
//! covers many cells (departure dates).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Cabin, PosId, RouteId};

/// One direction of a trip, priced one way (DECISIONS.md 0042).
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
pub enum Leg {
    /// Origin → destination.
    Outbound,
    /// Destination → origin.
    Return,
}

/// What a family's queries read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FamilyKind {
    /// One leg, one way, one price per departure day (one-way price
    /// graph).
    Leg { leg: Leg },
    /// Round trips of exactly `days` days, one price per departure day
    /// (round-trip price graph). Tracked where the round trip differs
    /// from the sum of its legs (0042).
    RoundTrip { days: u16 },
    /// The daily "anywhere" sweep from an origin (0025). Its route is
    /// the origin's sweep pseudo-route; its cells are hints.
    Explore,
}

/// Where a price is read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Source {
    GoogleFlights,
    /// An airline's own site, by IATA airline code (`"LA"`, `"AD"`,
    /// `"G3"`, `"TP"` …) (0012).
    Airline {
        code: String,
    },
    /// A miles program (`"smiles"`, `"latam_pass"`,
    /// `"azul_fidelidade"`) (0023). Prices are miles plus taxes.
    Miles {
        program: String,
    },
}

impl Source {
    /// The rate-limit bucket this source belongs to (0028: per-site
    /// limits).
    pub fn site(&self) -> String {
        match self {
            Source::GoogleFlights => "google_flights".to_owned(),
            Source::Airline { code } => format!("airline:{code}"),
            Source::Miles { program } => format!("miles:{program}"),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.site())
    }
}

/// Everything that identifies a family.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FamilyKey {
    pub route: RouteId,
    pub cabin: Cabin,
    pub source: Source,
    pub pos: PosId,
    pub kind: FamilyKind,
}
