//! Why a query ran and how it ended. Every query records both, so the
//! scheduler's estimates stay unbiased and policies can be replayed
//! (DECISIONS.md 0028).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum QueryReason {
    /// The highest score this tick.
    Scheduled { score: f64, runner_up: Option<f64> },
    /// The ~10% uniform sample that keeps change rates unbiased.
    Random,
    /// The family went past its maximum interval.
    Overdue,
    /// An Explore hint raised it.
    Hint,
    /// A nearby family showed a drop.
    FanOut,
    /// Re-checking a deal candidate before alerting (0031).
    Confirmation,
    /// Asked for from the app (0046).
    Manual,
    /// A new family's first check.
    Bootstrap,
}

impl QueryReason {
    pub fn label(&self) -> &'static str {
        match self {
            QueryReason::Scheduled { .. } => "scheduled",
            QueryReason::Random => "random",
            QueryReason::Overdue => "overdue",
            QueryReason::Hint => "hint",
            QueryReason::FanOut => "fan_out",
            QueryReason::Confirmation => "confirmation",
            QueryReason::Manual => "manual",
            QueryReason::Bootstrap => "bootstrap",
        }
    }

    /// Only these count towards unbiased change-rate estimates.
    pub fn is_unbiased(&self) -> bool {
        matches!(self, QueryReason::Random | QueryReason::Bootstrap)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum QueryOutcome {
    Ok,
    /// "Unusual traffic", a CAPTCHA or a block page (0039).
    Blocked {
        detail: String,
    },
    /// The page loaded but did not produce what was expected (a format
    /// change). The check is retried later (0059).
    Unexpected {
        detail: String,
    },
    /// The page loaded and says there is nothing to sell ("No results
    /// returned", "No options matching your search", "No Business Class
    /// flights found"). Not a failure: the family waits a week (0067).
    NoFlights,
    /// A miles program needs the user to log in again (0024).
    LoggedOut,
    Error {
        detail: String,
    },
}

impl QueryOutcome {
    pub fn is_block(&self) -> bool {
        matches!(self, QueryOutcome::Blocked { .. })
    }

    /// The check went wrong: blocked, an unexpected page, logged out or
    /// an error. A check that loaded, with prices or with none
    /// (`NoFlights`, 0067), is not a failure.
    pub fn is_failure(&self) -> bool {
        !matches!(self, QueryOutcome::Ok | QueryOutcome::NoFlights)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flights_is_its_own_outcome() {
        let json = serde_json::to_string(&QueryOutcome::NoFlights)
            .expect("serializes");
        assert_eq!(json, r#"{"outcome":"no_flights"}"#);
        let back: QueryOutcome =
            serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, QueryOutcome::NoFlights);
        assert!(!QueryOutcome::NoFlights.is_block());
        assert!(!QueryOutcome::NoFlights.is_failure());
        assert!(!QueryOutcome::Ok.is_failure());
        assert!(QueryOutcome::LoggedOut.is_failure());
        assert!(QueryOutcome::Unexpected { detail: "x".into() }.is_failure());
    }
}
