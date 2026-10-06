//! Effective BRL cost of a price (DECISIONS.md 0033, 0038), over
//! [`tm_domain::effective_brl`] with an exchange-rate table.

use std::collections::BTreeMap;

use tm_domain::{Currency, Money, effective_brl};

use crate::Params;

/// BRL per unit of each currency, by ISO code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FxTable {
    pub brl_per_unit: BTreeMap<String, f64>,
}

impl FxTable {
    pub fn new(rates: impl IntoIterator<Item = (String, f64)>) -> Self {
        Self {
            brl_per_unit: rates.into_iter().collect(),
        }
    }

    /// BRL per unit of `currency`: 1 for BRL, `None` when unknown or not
    /// a positive finite number.
    pub fn rate(&self, currency: &Currency) -> Option<f64> {
        if currency.as_str() == "BRL" {
            return Some(1.0);
        }
        self.brl_per_unit
            .get(currency.as_str())
            .copied()
            .filter(|r| r.is_finite() && *r > 0.0)
    }
}

/// What `price` costs in BRL: unchanged for BRL, otherwise
/// `price × rate × (1 + IOF) × (1 + card spread)`. `None` when the rate
/// is unknown.
pub fn effective(
    price: &Money,
    fx: &FxTable,
    params: &Params,
) -> Option<Money> {
    let rate = fx.rate(&price.currency)?;
    Some(effective_brl(price, rate, params.iof, params.card_spread))
}

/// [`effective`] in minor units.
pub fn effective_minor(
    price: &Money,
    fx: &FxTable,
    params: &Params,
) -> Option<i64> {
    effective(price, fx, params).map(|m| m.minor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_with_iof_and_spread() {
        let fx = FxTable::new([
            ("USD".to_owned(), 5.0),
            ("EUR".to_owned(), f64::NAN),
        ]);
        let p = Params::default(); // IOF 3.5%, spread 1%
        let usd = Money::from_whole(746, Currency::new("USD"));
        assert_eq!(
            effective(&usd, &fx, &p).expect("USD is known").whole(),
            3899
        );
        let brl = Money::brl_whole(1000);
        assert_eq!(effective(&brl, &fx, &p), Some(brl.clone()));
        assert_eq!(effective_minor(&brl, &fx, &p), Some(100_000));
        assert_eq!(
            effective(&Money::from_whole(1, Currency::new("EUR")), &fx, &p),
            None
        );
        assert_eq!(
            effective(&Money::from_whole(1, Currency::new("INR")), &fx, &p),
            None
        );
    }
}
