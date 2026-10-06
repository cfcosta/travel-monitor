//! `GetExploreDestinations`: one call per Explore page, two payloads.
//!
//! - Destinations with prices, at `[4][0]`:
//!   `["/m/04llb", [[null, 3825], "<token>"], null × 4,
//!     ["multi", "Air France, Transavia, and KLM", 1 (stops), 1131 (minutes),
//!      null, "LIS", "/m/022pfm" (origin), …], …]`.
//!   The token's label carries the itinerary and dates:
//!   `GRU-LIS:2026-11-12_2026-11-19`.
//! - Destination metadata, at `[3][0]`:
//!   `["/m/04llb", [38.72, -9.13], "Lisbon", "<image>", "Portugal", …]`.

use chrono::NaiveDate;
use serde_json::Value;
use tm_domain::{Currency, Money};

use super::{
    RpcError,
    at,
    calendar::truncate,
    decode_price_token,
    payloads,
    price_pair,
    str_at,
    unrecognized,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ExploreDestination {
    pub freebase_id: String,
    /// The airport Explore priced (`"LIS"`).
    pub airport: Option<String>,
    /// The origin airport, from the token (`"GRU"`).
    pub origin_airport: Option<String>,
    /// Whole units of the display currency.
    pub price: Option<i64>,
    /// The exact amount from the token.
    pub exact: Option<Money>,
    pub departure: Option<NaiveDate>,
    pub return_date: Option<NaiveDate>,
    pub stops: Option<u8>,
    pub duration_minutes: Option<u32>,
    /// `"TP"`, or `"multi"` for several carriers.
    pub carrier_code: Option<String>,
    /// `"Tap Air Portugal"`, `"Air France, Transavia, and KLM"`.
    pub carrier: Option<String>,
    /// From the metadata payload, when present.
    pub name: Option<String>,
    pub country: Option<String>,
    pub coordinates: Option<(f64, f64)>,
}

impl ExploreDestination {
    pub fn money(&self, display: &Currency) -> Option<Money> {
        self.price.map(|p| Money::from_whole(p, display.clone()))
    }
}

/// `GRU-LIS:2026-11-12_2026-11-19` → (GRU, LIS, departure, return).
fn route_label(
    label: &str,
) -> Option<(String, String, NaiveDate, Option<NaiveDate>)> {
    let (route, dates) = label.split_once(':')?;
    let (from, to) = route.split_once('-')?;
    let (dep, ret) = match dates.split_once('_') {
        Some((d, r)) => (d, Some(r)),
        None => (dates, None),
    };
    let dep = super::date_str(dep)?;
    let ret = match ret {
        Some(r) => Some(super::date_str(r)?),
        None => None,
    };
    Some((from.to_owned(), to.to_owned(), dep, ret))
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.is_empty()).map(str::to_owned)
}

fn destination(v: &Value) -> Result<ExploreDestination, RpcError> {
    let freebase_id = str_at(v, &[0])
        .filter(|id| id.starts_with('/'))
        .ok_or_else(|| unrecognized("Explore destination", truncate(v)))?
        .to_owned();
    let (price, token) = price_pair(at(v, &[1]))?;
    let decoded = token.as_deref().and_then(|t| decode_price_token(t).ok());
    let route = decoded
        .as_ref()
        .and_then(|t| t.label.as_deref())
        .and_then(route_label);
    let info = at(v, &[6]).filter(|i| i.is_array());
    let num = |i: usize| info.and_then(|f| at(f, &[i])).and_then(Value::as_u64);
    let airport = non_empty(info.and_then(|f| str_at(f, &[5])))
        .or_else(|| route.as_ref().map(|r| r.1.clone()));
    Ok(ExploreDestination {
        freebase_id,
        airport,
        origin_airport: route.as_ref().map(|r| r.0.clone()),
        price,
        exact: decoded.and_then(|t| t.amount),
        departure: route.as_ref().map(|r| r.2),
        return_date: route.as_ref().and_then(|r| r.3),
        // An entry without a price carries zeros here.
        stops: price.and(num(2)).and_then(|n| u8::try_from(n).ok()),
        duration_minutes: price.and(num(3)).and_then(|n| u32::try_from(n).ok()),
        carrier_code: price.and(non_empty(info.and_then(|f| str_at(f, &[0])))),
        carrier: price.and(non_empty(info.and_then(|f| str_at(f, &[1])))),
        name: None,
        country: None,
        coordinates: None,
    })
}

/// Is this payload's `[4][0]` the destination price list?
fn price_list(payload: &Value) -> Option<&Vec<Value>> {
    let list = at(payload, &[4, 0])?.as_array()?;
    list.iter()
        .all(|e| str_at(e, &[0]).is_some_and(|id| id.starts_with('/')))
        .then_some(list)
}

/// Destinations from one Explore response, in Google's order. Entries
/// with no price are kept with `price: None`. The response streams
/// several payloads: the first lists most destinations, later ones add
/// one each.
pub fn parse_explore(body: &str) -> Result<Vec<ExploreDestination>, RpcError> {
    let all = payloads(body)?;
    let mut out = Vec::new();
    let mut found = false;
    for payload in &all {
        if let Some(list) = price_list(payload) {
            found = true;
            // Later payloads add destinations as they are priced; a
            // repeated destination replaces the earlier entry.
            for v in list {
                let d = destination(v)?;
                match out.iter_mut().find(|x: &&mut ExploreDestination| {
                    x.freebase_id == d.freebase_id
                }) {
                    Some(existing) => *existing = d,
                    None => out.push(d),
                }
            }
        }
    }
    if !found {
        return Err(unrecognized("Explore response", "no destination list"));
    }
    // Metadata: name, country and coordinates by Freebase ID.
    for payload in &all {
        let Some(list) = at(payload, &[3, 0]).and_then(Value::as_array) else {
            continue;
        };
        for meta in list {
            let Some(id) = str_at(meta, &[0]) else {
                continue;
            };
            let coords = at(meta, &[1]).and_then(|c| {
                Some((at(c, &[0])?.as_f64()?, at(c, &[1])?.as_f64()?))
            });
            for d in out.iter_mut().filter(|d| d.freebase_id == id) {
                d.name = non_empty(str_at(meta, &[2]));
                d.country = non_empty(str_at(meta, &[4]));
                d.coordinates = coords;
            }
        }
    }
    Ok(out)
}
