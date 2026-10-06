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
}
