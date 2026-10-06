//! Prices. Amounts are integer minor units (cents), never floats.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An ISO 4217 currency code, upper case (`"BRL"`).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Currency(pub String);

impl Currency {
    pub fn new(code: &str) -> Self {
        Self(code.trim().to_ascii_uppercase())
    }

    pub fn brl() -> Self {
        Self("BRL".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An amount in a currency, in minor units.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

impl Money {
    pub fn new(minor: i64, currency: Currency) -> Self {
        Self { minor, currency }
    }

    /// Google Flights quotes whole units (`3932` is R$3,932).
    pub fn from_whole(units: i64, currency: Currency) -> Self {
        Self {
            minor: units * 100,
            currency,
        }
    }

    pub fn brl_whole(units: i64) -> Self {
        Self::from_whole(units, Currency::brl())
    }

    /// The amount in whole units, rounded half away from zero.
    pub fn whole(&self) -> i64 {
        let rounded = (self.minor.abs() + 50) / 100;
        if self.minor < 0 { -rounded } else { rounded }
    }

    pub fn as_f64(&self) -> f64 {
        self.minor as f64 / 100.0
    }
}

impl fmt::Display for Money {
    /// `R$3,932`, `US$746`, or `1234 EUR` for others.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.whole();
        let digits = group_thousands(whole.unsigned_abs());
        let sign = if whole < 0 { "-" } else { "" };
        match self.currency.as_str() {
            "BRL" => write!(f, "{sign}R${digits}"),
            "USD" => write!(f, "{sign}US${digits}"),
            "EUR" => write!(f, "{sign}€{digits}"),
            "INR" => write!(f, "{sign}₹{digits}"),
            other => write!(f, "{sign}{digits} {other}"),
        }
    }
}

fn group_thousands(n: u64) -> String {
    let raw = n.to_string();
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    for (i, c) in raw.chars().enumerate() {
        if i > 0 && (raw.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a price costs the user in BRL (DECISIONS.md 0038):
/// `price × rate × (1 + IOF) × (1 + card spread)`. A BRL price is
/// returned unchanged; `brl_per_unit` is the exchange rate for the
/// price's currency (ignored for BRL). Rates are fractions (0.035).
pub fn effective_brl(
    price: &Money,
    brl_per_unit: f64,
    iof: f64,
    card_spread: f64,
) -> Money {
    if price.currency.as_str() == "BRL" {
        return price.clone();
    }
    let minor =
        price.minor as f64 * brl_per_unit * (1.0 + iof) * (1.0 + card_spread);
    Money::new(minor.round() as i64, Currency::brl())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_google() {
        assert_eq!(Money::brl_whole(3932).to_string(), "R$3,932");
        assert_eq!(
            Money::from_whole(746, Currency::new("usd")).to_string(),
            "US$746"
        );
        assert_eq!(Money::brl_whole(14870).to_string(), "R$14,870");
        assert_eq!(Money::brl_whole(916).to_string(), "R$916");
    }

    #[test]
    fn effective_cost_adds_iof_and_spread() {
        let usd = Money::from_whole(746, Currency::new("USD"));
        // 746 × 5.0 × 1.035 × 1.01 = 3899.47
        let brl = effective_brl(&usd, 5.0, 0.035, 0.01);
        assert_eq!(brl.whole(), 3899);
        assert_eq!(brl.currency, Currency::brl());
    }

    #[test]
    fn brl_is_unchanged() {
        let p = Money::brl_whole(3932);
        assert_eq!(effective_brl(&p, 9.9, 0.035, 0.05), p);
    }
}
