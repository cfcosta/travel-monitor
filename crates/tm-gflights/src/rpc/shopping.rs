//! `GetShoppingResults` and the results page's embedded `ds:1`.
//!
//! Payload: `[header, places, best, other, …]`, where `best` and
//! `other` are `[[itinerary, …], …]` or null. An itinerary is
//! `[flight, [[null, price], "<token>"], …]` and a flight is
//!
//! ```text
//! [0] "TP" | "multi"     [1] ["Tap Air Portugal", …]   [2] [segment, …]
//! [3] "GRU" [4] [2026, 11, 10] [5] [1, 30]              departure
//! [6] "LIS" [7] [2026, 11, 10] [8] [14, 25]             arrival
//! [9] 595 (minutes)
//! ```
//!
//! A segment: `[2]` operated by, `[3]` from, `[4]`/`[5]` airport names,
//! `[6]` to, `[8]` departure time, `[10]` arrival time, `[11]` minutes,
//! `[17]` aircraft, `[20]`/`[21]` dates, `[22]` `["TP", "84", null, "Tap Air Portugal"]`.
//! Times omit zero minutes (`[10]` is 10:00).
//!
//! The RPC streams several payloads as results come in; the last one
//! with itineraries is the most complete.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::Value;
use tm_domain::{Currency, Money};

use super::{
    RpcError,
    at,
    calendar::truncate,
    exact_of,
    parse_init_data,
    payloads,
    price_pair,
    str_at,
    unrecognized,
};
use crate::tfs::FlightSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub from: String,
    pub to: String,
    pub from_name: Option<String>,
    pub to_name: Option<String>,
    pub departure: NaiveDateTime,
    pub arrival: NaiveDateTime,
    pub duration_minutes: Option<u32>,
    /// Marketing carrier code (`"TP"`).
    pub carrier: String,
    pub carrier_name: Option<String>,
    /// Flight number without the carrier (`"84"`).
    pub number: String,
    pub operated_by: Option<String>,
    pub aircraft: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Itinerary {
    /// `"TP"`, or `"multi"` for several carriers.
    pub carrier: String,
    pub airlines: Vec<String>,
    pub from: String,
    pub to: String,
    pub departure: NaiveDateTime,
    pub arrival: NaiveDateTime,
    pub duration_minutes: u32,
    pub segments: Vec<Segment>,
    /// Whole units of the display currency. On a round trip's results
    /// it is the round-trip total.
    pub price: Option<i64>,
    pub exact: Option<Money>,
    /// The price token; [`crate::url::booking_url`] takes it as `tfu`.
    pub token: Option<String>,
    /// In the "Top"/"Best" list rather than "Other flights".
    pub best: bool,
}

impl Itinerary {
    pub fn stops(&self) -> u8 {
        u8::try_from(self.segments.len().saturating_sub(1)).unwrap_or(u8::MAX)
    }

    pub fn money(&self, display: &Currency) -> Option<Money> {
        self.price.map(|p| Money::from_whole(p, display.clone()))
    }

    /// The flights to put in a [`crate::tfs::LegSpec`] to select this
    /// itinerary (next page, or the booking page).
    pub fn flight_specs(&self) -> Vec<FlightSpec> {
        self.segments
            .iter()
            .map(|s| FlightSpec {
                from: s.from.clone(),
                date: s.departure.date(),
                to: s.to.clone(),
                carrier: s.carrier.clone(),
                number: s.number.clone(),
                extra: Vec::new(),
            })
            .collect()
    }
}

fn date(v: Option<&Value>) -> Option<NaiveDate> {
    let v = v?;
    let n = |i| at(v, &[i]).and_then(Value::as_u64);
    NaiveDate::from_ymd_opt(
        i32::try_from(n(0)?).ok()?,
        u32::try_from(n(1)?).ok()?,
        u32::try_from(n(2)?).ok()?,
    )
}

fn time(v: Option<&Value>) -> Option<NaiveTime> {
    let arr = v?.as_array()?;
    let part = |i: usize| match arr.get(i) {
        None | Some(Value::Null) => Some(0),
        Some(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
    };
    NaiveTime::from_hms_opt(part(0)?, part(1)?, 0)
}

fn moment(v: &Value, d: usize, t: usize) -> Option<NaiveDateTime> {
    Some(date(at(v, &[d]))?.and_time(time(at(v, &[t]))?))
}

fn opt_string(v: &Value, i: usize) -> Option<String> {
    str_at(v, &[i]).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn minutes(v: &Value, i: usize) -> Option<u32> {
    at(v, &[i])
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

fn segment(v: &Value) -> Result<Segment, RpcError> {
    let bad = || unrecognized("segment", truncate(v));
    let flight = at(v, &[22]).ok_or_else(bad)?;
    Ok(Segment {
        from: opt_string(v, 3).ok_or_else(bad)?,
        to: opt_string(v, 6).ok_or_else(bad)?,
        from_name: opt_string(v, 4),
        to_name: opt_string(v, 5),
        departure: moment(v, 20, 8).ok_or_else(bad)?,
        arrival: moment(v, 21, 10).ok_or_else(bad)?,
        duration_minutes: minutes(v, 11),
        carrier: opt_string(flight, 0).ok_or_else(bad)?,
        number: opt_string(flight, 1).ok_or_else(bad)?,
        carrier_name: opt_string(flight, 3),
        operated_by: opt_string(v, 2),
        aircraft: opt_string(v, 17),
    })
}

fn itinerary(v: &Value, best: bool) -> Result<Itinerary, RpcError> {
    let f =
        at(v, &[0]).ok_or_else(|| unrecognized("itinerary", truncate(v)))?;
    let bad = || unrecognized("itinerary", truncate(f));
    let segments = at(f, &[2])
        .and_then(Value::as_array)
        .ok_or_else(bad)?
        .iter()
        .map(segment)
        .collect::<Result<Vec<_>, _>>()?;
    if segments.is_empty() {
        return Err(bad());
    }
    let airlines = at(f, &[1])
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let (price, token) = price_pair(at(v, &[1]))?;
    Ok(Itinerary {
        carrier: opt_string(f, 0).ok_or_else(bad)?,
        airlines,
        from: opt_string(f, 3).ok_or_else(bad)?,
        to: opt_string(f, 6).ok_or_else(bad)?,
        departure: moment(f, 4, 5).ok_or_else(bad)?,
        arrival: moment(f, 7, 8).ok_or_else(bad)?,
        duration_minutes: minutes(f, 9).ok_or_else(bad)?,
        segments,
        price,
        exact: exact_of(token.as_deref()),
        token,
        best,
    })
}

/// The itinerary list of a section (`best` or `other`), if any.
fn section(
    payload: &Value,
    index: usize,
) -> Result<Option<&Vec<Value>>, RpcError> {
    match at(payload, &[index]) {
        None | Some(Value::Null) => Ok(None),
        Some(s) => match at(s, &[0]) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(list)) => Ok(Some(list)),
            Some(other) => {
                Err(unrecognized("results section", truncate(other)))
            }
        },
    }
}

/// Itineraries from shopping payloads: the last payload that has any.
pub(crate) fn from_payloads(all: &[Value]) -> Result<Vec<Itinerary>, RpcError> {
    let mut last = None;
    for payload in all {
        if payload.as_array().is_none_or(|a| a.len() < 4) {
            return Err(unrecognized("shopping payload", truncate(payload)));
        }
        let mut items = Vec::new();
        for (index, best) in [(2, true), (3, false)] {
            if let Some(list) = section(payload, index)? {
                for v in list {
                    items.push(itinerary(v, best)?);
                }
            }
        }
        if !items.is_empty() {
            last = Some(items);
        }
    }
    Ok(last.unwrap_or_default())
}

/// Itineraries from a `GetShoppingResults` response body.
pub fn parse_shopping_results(body: &str) -> Result<Vec<Itinerary>, RpcError> {
    from_payloads(&payloads(body)?)
}

/// Itineraries from the `ds:1` script a results page embeds (a page
/// loaded from a URL shows its first results without an RPC).
pub fn shopping_results_from_init_data(
    script: &str,
) -> Result<Vec<Itinerary>, RpcError> {
    let (_, data) = parse_init_data(script)?;
    from_payloads(std::slice::from_ref(&data))
}
