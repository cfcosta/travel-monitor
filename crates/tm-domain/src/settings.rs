//! Global settings, edited in the app and stored in the database
//! (DECISIONS.md 0035, 0036, 0038, 0039, 0043).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Who travels: one global setting (0036). Changing it starts new
/// series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Passengers {
    pub adults: u8,
    pub children: u8,
    pub infants_in_seat: u8,
    pub infants_on_lap: u8,
}

impl Default for Passengers {
    fn default() -> Self {
        Self {
            adults: 1,
            children: 0,
            infants_in_seat: 0,
            infants_on_lap: 0,
        }
    }
}

impl Passengers {
    pub fn total(&self) -> u8 {
        self.adults + self.children + self.infants_in_seat + self.infants_on_lap
    }

    /// A key that changes whenever the party changes, so series of
    /// different parties are kept apart.
    pub fn key(&self) -> String {
        format!(
            "a{}c{}s{}l{}",
            self.adults,
            self.children,
            self.infants_in_seat,
            self.infants_on_lap
        )
    }
}

/// Trips are any pair with `min_days..=max_days` between departure and
/// return (0043).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TripRange {
    pub min_days: u16,
    pub max_days: u16,
    /// The round-trip length tracked on international routes (0042).
    pub round_trip_days: u16,
}

impl Default for TripRange {
    fn default() -> Self {
        Self {
            min_days: 5,
            max_days: 10,
            round_trip_days: 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub passengers: Passengers,
    pub trips: TripRange,
    /// How far ahead departures go, in days (0009: six months).
    pub horizon_days: u16,
    /// IOF on foreign-currency card purchases (0038).
    pub iof: f64,
    /// The card's exchange spread (0038).
    pub card_spread: f64,
    /// Page loads per day; starts at 1200 and ramps (0039, 0056).
    pub daily_budget: u32,
    pub max_daily_budget: u32,
    /// A family is checked at least this often (0029: one week).
    pub max_interval_hours: u32,
    /// And at most this often (0028: do not chase volatile entries).
    pub min_interval_hours: u32,
    /// Share of checks spent uniformly at random (0028).
    pub random_share: f64,
    /// A deal is at least this far under the bucket median, before the
    /// robust-spread test (0030).
    pub deal_threshold: f64,
    /// Weeks of observations a bucket needs before it flags deals.
    pub min_history_weeks: u16,
    /// Checks in a row where the round trip matched its legs before
    /// round-trip tracking stops for a route (0042).
    pub additive_checks_to_stop: u16,
    /// Display language for Google (`hl`).
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            passengers: Passengers::default(),
            trips: TripRange::default(),
            horizon_days: 180,
            iof: 0.035,
            card_spread: 0.01,
            // 0056: enough for every destination's core legs daily.
            daily_budget: 1200,
            max_daily_budget: 3000,
            max_interval_hours: 24 * 7,
            min_interval_hours: 6,
            random_share: 0.10,
            deal_threshold: 0.20,
            min_history_weeks: 4,
            additive_checks_to_stop: 5,
            language: "en".to_owned(),
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum SettingsError {
    #[error("at least one adult must travel")]
    NoAdults,
    #[error("more lap infants than adults")]
    TooManyLapInfants,
    #[error("trip range {0}..={1} days is empty or out of 1..=60")]
    TripRange(u16, u16),
    #[error("{0} must be between 0 and 1")]
    Fraction(&'static str),
    #[error("the minimum interval must be shorter than the maximum")]
    Intervals,
}

impl Settings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.passengers.adults == 0 {
            return Err(SettingsError::NoAdults);
        }
        if self.passengers.infants_on_lap > self.passengers.adults {
            return Err(SettingsError::TooManyLapInfants);
        }
        let TripRange {
            min_days, max_days, ..
        } = self.trips;
        if min_days == 0 || min_days > max_days || max_days > 60 {
            return Err(SettingsError::TripRange(min_days, max_days));
        }
        for (name, value) in [
            ("iof", self.iof),
            ("card_spread", self.card_spread),
            ("random_share", self.random_share),
            ("deal_threshold", self.deal_threshold),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(SettingsError::Fraction(name));
            }
        }
        if self.min_interval_hours >= self.max_interval_hours {
            return Err(SettingsError::Intervals);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(Settings::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_empty_trip_range() {
        let mut s = Settings::default();
        s.trips.min_days = 11;
        assert_eq!(s.validate(), Err(SettingsError::TripRange(11, 10)));
    }
}
