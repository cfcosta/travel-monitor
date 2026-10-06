//! `GetBookingResults`: the booking page's sellers (DECISIONS.md 0034).
//!
//! The offers are at `[1][0]` of one payload:
//!
//! ```text
//! [0,
//!  [["BOOKING", "Booking.com", null, false], …],   sellers (is_airline)
//!  null,
//!  [["TP", "84"], ["TP", "89"]],                    flights
//!  …,
//!  [[null, 3709], "<token>"],                       [7] display price
//!  [["USD", 742]] | null,                           [8] the seller's own quote
//!  …]
//! ```

use serde_json::Value;
use tm_domain::{Currency, Money, PosId, SellerQuote};

use super::{
    RpcError,
    as_whole,
    at,
    calendar::truncate,
    exact_of,
    payloads,
    price_pair,
    str_at,
    unrecognized,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seller {
    /// `"TP"`, `"BOOKING"`, `"ETRAVELI_FLIGHTNETWORK"`.
    pub code: String,
    pub name: String,
    pub is_airline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellerOffer {
    /// Usually one; several when the trip is sold as separate tickets.
    pub sellers: Vec<Seller>,
    /// Whole units of the display currency.
    pub price: Option<i64>,
    /// The exact amount from the token.
    pub exact: Option<Money>,
    /// In the seller's own currency when the page shows it (`$742`
    /// under `gl=US`); `None` when the seller quotes the display currency.
    pub quoted: Option<Money>,
    /// `(carrier, number)` of every flight in the offer.
    pub flights: Vec<(String, String)>,
}

impl SellerOffer {
    /// The sellers' names, joined with " + ".
    pub fn seller(&self) -> String {
        self.sellers
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// Every seller is an airline (the airline's own price, 0034).
    pub fn is_airline(&self) -> bool {
        !self.sellers.is_empty() && self.sellers.iter().all(|s| s.is_airline)
    }

    /// As a [`SellerQuote`] for a point of sale. `None` without a price.
    pub fn to_quote(
        &self,
        pos: PosId,
        display: &Currency,
    ) -> Option<SellerQuote> {
        Some(SellerQuote {
            pos,
            seller: self.seller(),
            is_airline: self.is_airline(),
            price: Money::from_whole(self.price?, display.clone()),
            quoted: self.quoted.clone(),
        })
    }
}

fn seller(v: &Value) -> Result<Seller, RpcError> {
    let bad = || unrecognized("seller", v.to_string());
    Ok(Seller {
        code: str_at(v, &[0]).ok_or_else(bad)?.to_owned(),
        name: str_at(v, &[1]).ok_or_else(bad)?.to_owned(),
        is_airline: at(v, &[3]).and_then(Value::as_bool).ok_or_else(bad)?,
    })
}

fn quoted(v: Option<&Value>) -> Result<Option<Money>, RpcError> {
    let Some(first) = v.filter(|v| !v.is_null()).and_then(|v| at(v, &[0]))
    else {
        return Ok(None);
    };
    let currency = str_at(first, &[0])
        .ok_or_else(|| unrecognized("quoted price", first.to_string()))?;
    let amount = at(first, &[1])
        .and_then(as_whole)
        .ok_or_else(|| unrecognized("quoted price", first.to_string()))?;
    Ok(Some(Money::from_whole(amount, Currency::new(currency))))
}

fn offer(v: &Value) -> Result<SellerOffer, RpcError> {
    let sellers = at(v, &[1])
        .and_then(Value::as_array)
        .ok_or_else(|| unrecognized("offer", truncate(v)))?
        .iter()
        .map(seller)
        .collect::<Result<Vec<_>, _>>()?;
    let flights = at(v, &[3])
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|f| {
                    Some((
                        str_at(f, &[0])?.to_owned(),
                        str_at(f, &[1])?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let (price, token) = price_pair(at(v, &[7]))?;
    Ok(SellerOffer {
        sellers,
        price,
        exact: exact_of(token.as_deref()),
        quoted: quoted(at(v, &[8]))?,
        flights,
    })
}

/// Does this payload hold the offers? `[1][0]` is a list whose entries
/// start with a number and a seller list.
fn offer_list(payload: &Value) -> Option<&Vec<Value>> {
    let list = at(payload, &[1, 0])?.as_array()?;
    list.iter()
        .all(|o| {
            at(o, &[0]).is_some_and(Value::is_number)
                && at(o, &[1, 0, 1]).is_some_and(Value::is_string)
        })
        .then_some(list)
        .filter(|l| !l.is_empty())
}

/// Every booking option, in the page's order. The response streams
/// payloads; the last one with offers wins.
pub fn parse_booking_results(body: &str) -> Result<Vec<SellerOffer>, RpcError> {
    let mut last = None;
    for payload in payloads(body)? {
        if let Some(list) = offer_list(&payload) {
            last = Some(list.iter().map(offer).collect::<Result<Vec<_>, _>>()?);
        }
    }
    last.ok_or_else(|| unrecognized("booking response", "no offer list"))
}
