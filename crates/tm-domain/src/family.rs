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

    /// The source a [`Source::site`] names, if it names one.
    pub fn from_site(site: &str) -> Option<Self> {
        if site == GOOGLE_FLIGHTS {
            return Some(Source::GoogleFlights);
        }
        if let Some(code) = site.strip_prefix("airline:") {
            return (!code.is_empty()).then(|| Source::Airline {
                code: code.to_owned(),
            });
        }
        let program = site.strip_prefix("miles:")?;
        (!program.is_empty()).then(|| Source::Miles {
            program: program.to_owned(),
        })
    }
}

/// Google Flights' [`Source::site`].
pub const GOOGLE_FLIGHTS: &str = "google_flights";

/// A site's daily page-load budget (DECISIONS.md 0060): where it starts
/// and the most the ramp may raise it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteLimits {
    pub start: u32,
    pub max: u32,
}

impl SiteLimits {
    /// Every site but Google Flights, until set otherwise (0060): 300
    /// loads a day, ramping to 1,000. Google Flights' come from
    /// [`crate::Settings`].
    pub const OTHER: SiteLimits = SiteLimits {
        start: 300,
        max: 1000,
    };

    /// A miles program: every award is a page load under the user's own
    /// account, so 40 a day, at most 80 (0083).
    pub const MILES: SiteLimits = SiteLimits { start: 40, max: 80 };

    /// At least one load a day, and a maximum no lower than the start.
    pub fn normalized(self) -> Self {
        let start = self.start.max(1);
        Self {
            start,
            max: self.max.max(start),
        }
    }
}

/// A site's directory name: its [`Source::site`] in lower case, with
/// anything but letters, digits, `_` and `-` turned into `-`
/// (`airline:TP` → `airline-tp`). Each site keeps its own Chromium
/// profile under it (DECISIONS.md 0062).
pub fn site_slug(site: &str) -> String {
    let slug: String = site
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if slug.is_empty() || slug.chars().all(|c| c == '-') {
        "site".to_owned()
    } else {
        slug
    }
}

/// The sites whose cookies hold a miles program's login session (0024):
/// the registrable domains, each matching its subdomains too. Empty for
/// an unknown program.
pub fn miles_session_domains(program: &str) -> &'static [&'static str] {
    match program {
        "smiles" => &["smiles.com.br"],
        "latam_pass" => &["latamairlines.com", "latam.com"],
        "azul_fidelidade" => &["voeazul.com.br", "tudoazul.com.br"],
        _ => &[],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_round_trip_and_slug() {
        for source in [
            Source::GoogleFlights,
            Source::Airline { code: "TP".into() },
            Source::Miles {
                program: "latam_pass".into(),
            },
        ] {
            assert_eq!(Source::from_site(&source.site()), Some(source));
        }
        assert_eq!(Source::from_site("airline:"), None);
        assert_eq!(Source::from_site("bing"), None);
        assert_eq!(site_slug("google_flights"), "google_flights");
        assert_eq!(site_slug("airline:TP"), "airline-tp");
        assert_eq!(site_slug("miles:latam_pass"), "miles-latam_pass");
        assert_eq!(site_slug("../x"), "---x");
        assert_eq!(site_slug(""), "site");
        assert_eq!(site_slug("::"), "site");
    }

    #[test]
    fn limits_are_normalized() {
        let l = SiteLimits { start: 0, max: 0 }.normalized();
        assert_eq!(l, SiteLimits { start: 1, max: 1 });
        let l = SiteLimits {
            start: 500,
            max: 10,
        }
        .normalized();
        assert_eq!(
            l,
            SiteLimits {
                start: 500,
                max: 500
            }
        );
        let s = crate::Settings::default();
        assert_eq!(
            s.default_limits(GOOGLE_FLIGHTS),
            SiteLimits {
                start: 30_000,
                max: 30_000
            }
        );
        assert_eq!(s.default_limits("airline:TP"), SiteLimits::OTHER);
        assert_eq!(s.default_limits("miles:latam_pass"), SiteLimits::MILES);
    }
}
