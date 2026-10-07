//! Award prices: miles (or points) plus taxes in BRL (DECISIONS.md
//! 0023, 0083).
//!
//! An award is a series of its own: its miles are never compared with a
//! cash price for baselines or deals (0023). What a mile is worth is
//! measured, not set: every award observation keeps the program's own
//! fare before taxes, and the implied rate is that fare over the miles
//! (0083). The arithmetic on rates lives in `tm-scheduler`; this module
//! holds the types and how they read (`63,848 mi + R$69`), shared by the
//! app and the daemon's notifications.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Money, money::group_thousands};

/// What a program counts in: miles (LATAM Pass, Smiles) or points (Azul
/// Fidelidade).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MilesUnit {
    #[default]
    Miles,
    Points,
}

impl MilesUnit {
    /// The unit of a program (`"azul_fidelidade"` counts points).
    pub fn of(program: &str) -> Self {
        match program {
            "azul_fidelidade" => MilesUnit::Points,
            _ => MilesUnit::Miles,
        }
    }

    /// `mi`, `pts`.
    pub fn label(self) -> &'static str {
        match self {
            MilesUnit::Miles => "mi",
            MilesUnit::Points => "pts",
        }
    }
}

/// A known program's name (`latam_pass` is `LATAM Pass`), for text
/// written without the store at hand; the slug otherwise.
pub fn program_name(program: &str) -> &str {
    match program {
        "smiles" => "Smiles",
        "latam_pass" => "LATAM Pass",
        "azul_fidelidade" => "Azul Fidelidade",
        other => other,
    }
}

/// The programs the daemon reads awards from, or is about to (0050,
/// 0083): LATAM Pass, whose responses were read with a headed browser.
/// The others have not been probed logged in; they cannot be switched
/// on in the app yet.
pub fn has_award_reader(program: &str) -> bool {
    program == "latam_pass"
}

/// What an award costs: miles plus taxes, paid in BRL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AwardPrice {
    pub miles: i64,
    pub taxes: Money,
}

impl AwardPrice {
    pub fn new(miles: i64, taxes: Money) -> Self {
        Self { miles, taxes }
    }

    /// `63,848 mi + R$69` (taxes in whole reais, like every price).
    pub fn text(&self, unit: MilesUnit) -> String {
        format!(
            "{} {} + {}",
            format_miles(self.miles),
            unit.label(),
            self.taxes
        )
    }

    /// Both legs of a trip, or `None` when the taxes are in different
    /// currencies.
    pub fn plus(&self, other: &AwardPrice) -> Option<AwardPrice> {
        (self.taxes.currency == other.taxes.currency).then(|| AwardPrice {
            miles: self.miles + other.miles,
            taxes: Money::new(
                self.taxes.minor + other.taxes.minor,
                self.taxes.currency.clone(),
            ),
        })
    }
}

/// `63,848`: miles with thousands separators, as the app writes every
/// count (not the site's pt-BR `63.848`).
pub fn format_miles(miles: i64) -> String {
    let digits = group_thousands(miles.unsigned_abs());
    if miles < 0 {
        format!("-{digits}")
    } else {
        digits
    }
}

/// `264k`, `64k`, `950`: miles for map labels and narrow columns.
pub fn format_miles_short(miles: i64) -> String {
    if miles.abs() < 1000 {
        return miles.to_string();
    }
    format!("{}k", (miles as f64 / 1000.0).round() as i64)
}

/// `R$28.80/k`, `R$20/k`: a value in reais per 1,000 miles, to the
/// cent, whole values without them.
pub fn format_mile_value(brl_per_1000: f64) -> String {
    let cents = (brl_per_1000 * 100.0).round() as i64;
    if cents % 100 == 0 {
        format!("R${}/k", cents / 100)
    } else {
        format!("R${}.{:02}/k", cents / 100, (cents % 100).abs())
    }
}

/// Seats left at a brand's price (`seatsRemaining`). LATAM caps the
/// count at 9, which means "9 or more".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Seats {
    pub count: u8,
    /// The program's own "few left" flag.
    pub low: bool,
}

impl Seats {
    /// The count the app starts naming seats at.
    pub const FEW: u8 = 3;

    /// Worth showing: 3 or fewer, or flagged by the program.
    pub fn few(&self) -> bool {
        self.count <= Self::FEW || self.low
    }

    /// `1 seat left`, `2 seats left`.
    pub fn text(&self) -> String {
        match self.count {
            1 => "1 seat left".to_owned(),
            n => format!("{n} seats left"),
        }
    }

    /// `9+` for the capped count, `2 left` otherwise (tables).
    pub fn short(&self) -> String {
        if self.count >= 9 {
            "9+".to_owned()
        } else {
            format!("{} left", self.count)
        }
    }
}

/// What an award observation has that a cash one does not (0083). The
/// miles are [`crate::Observation::miles`] and the taxes its `price`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AwardDetail {
    /// The program's own fare before taxes for this brand, in BRL
    /// (LATAM's `priceWithOutTax`): what the miles stand for. Over the
    /// miles it is the implied value of a mile.
    pub fare: Option<Money>,
    /// `LIGHT`, `STANDARD`, `FULL`, `PREMIUM BUSINESS STANDARD`.
    pub brand: Option<String>,
    /// The fare basis the award is priced on (`XJEX0N1`). Awards are
    /// offered on some booking classes only: when the cheapest cash
    /// class is closed to awards, the award is priced on a dearer one,
    /// and its rate against the cheapest cash fare is well above the
    /// program's rate (latam-pass.md, "Measuring cash against miles").
    pub fare_basis: Option<String>,
    pub seats: Option<Seats>,
}

/// An itinerary key as the readers write it, every segment's flight and
/// local departure (`LA4776@2026-11-10T00:05/LA8150@2026-11-10T05:55`),
/// as a short label: the flights and the first departure time
/// (`LA4776/LA8150 00:05`). A key without times is returned as its
/// flights.
pub fn itinerary_label(key: &str) -> String {
    let mut flights = Vec::new();
    let mut first_time = None;
    for segment in key.split('/') {
        let (flight, departs) =
            segment.split_once('@').unwrap_or((segment, ""));
        flights.push(flight.trim());
        if first_time.is_none() {
            first_time =
                departs.split_once('T').map(|(_, time)| time.to_owned());
        }
    }
    let flights = flights.join("/");
    match first_time {
        Some(time) if !time.is_empty() => format!("{flights} {time}"),
        _ => flights,
    }
}

/// Which series a price belongs to: cash, or one miles program's
/// awards. Series never share a baseline (0023).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize,
)]
#[serde(tag = "series", rename_all = "snake_case")]
pub enum Series {
    #[default]
    Cash,
    Award {
        program: String,
    },
}

impl Series {
    /// The miles program, for an award series.
    pub fn program(&self) -> Option<&str> {
        match self {
            Series::Cash => None,
            Series::Award { program } => Some(program),
        }
    }

    pub fn is_award(&self) -> bool {
        matches!(self, Series::Award { .. })
    }
}

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Series::Cash => f.write_str("cash"),
            Series::Award { program } => write!(f, "award:{program}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awards_read_in_english() {
        let award =
            AwardPrice::new(63_848, Money::new(6861, crate::Currency::brl()));
        assert_eq!(award.text(MilesUnit::Miles), "63,848 mi + R$69");
        let azul = AwardPrice::new(40_000, Money::brl_whole(120));
        assert_eq!(
            azul.text(MilesUnit::of("azul_fidelidade")),
            "40,000 pts + R$120"
        );
        assert_eq!(format_miles(263_848), "263,848");
        assert_eq!(format_miles(950), "950");
        assert_eq!(format_miles_short(263_848), "264k");
        assert_eq!(format_miles_short(63_848), "64k");
        assert_eq!(format_miles_short(950), "950");
        assert_eq!(format_mile_value(28.7999), "R$28.80/k");
        assert_eq!(format_mile_value(20.0), "R$20/k");
        assert_eq!(format_mile_value(41.04), "R$41.04/k");
    }

    #[test]
    fn legs_add_up() {
        let out =
            AwardPrice::new(63_848, Money::new(6861, crate::Currency::brl()));
        let back =
            AwardPrice::new(63_848, Money::new(12_139, crate::Currency::brl()));
        let trip = out.plus(&back).expect("same currency");
        assert_eq!(trip.miles, 127_696);
        assert_eq!(trip.taxes.minor, 19_000);
        let usd = AwardPrice::new(
            1,
            Money::from_whole(1, crate::Currency::new("USD")),
        );
        assert_eq!(out.plus(&usd), None);
    }

    #[test]
    fn itineraries_read_as_flights_and_a_time() {
        assert_eq!(itinerary_label("LA8146@2026-11-10T17:40"), "LA8146 17:40");
        assert_eq!(
            itinerary_label("LA4776@2026-11-10T00:05/LA8150@2026-11-10T05:55"),
            "LA4776/LA8150 00:05"
        );
        assert_eq!(itinerary_label("LA8146"), "LA8146");
    }

    #[test]
    fn seats_show_when_few() {
        let nine = Seats {
            count: 9,
            low: false,
        };
        assert!(!nine.few());
        assert_eq!(nine.short(), "9+");
        let two = Seats {
            count: 2,
            low: false,
        };
        assert!(two.few());
        assert_eq!(two.text(), "2 seats left");
        assert_eq!(
            Seats {
                count: 1,
                low: false
            }
            .text(),
            "1 seat left"
        );
        assert!(
            Seats {
                count: 7,
                low: true
            }
            .few()
        );
    }

    #[test]
    fn series_name_their_program() {
        assert_eq!(Series::Cash.program(), None);
        let award = Series::Award {
            program: "latam_pass".into(),
        };
        assert_eq!(award.program(), Some("latam_pass"));
        assert!(award.is_award());
        assert_eq!(award.to_string(), "award:latam_pass");
        assert_eq!(MilesUnit::of("latam_pass"), MilesUnit::Miles);
        assert!(has_award_reader("latam_pass"));
        assert!(!has_award_reader("smiles"));
    }
}
