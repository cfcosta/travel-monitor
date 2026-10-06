//! `GetCalendarGraph` (price graph) and `GetCalendarGrid` (date grid).
//!
//! Requests: `[null, search, [first, last], returns, trip_length]`:
//! - round-trip graph: `returns` null, `trip_length` `[7, 7]`;
//! - round-trip grid: `returns` is the return range;
//! - one way (graph or grid): departures only.
//!
//! Responses: `[header, [cell, …]]`, one cell per date (pair):
//! `["2027-01-02", "2027-01-09" | null, [[null, 9326], "<token>"] | null, status]`.

use chrono::NaiveDate;
use serde_json::Value;
use tm_domain::{Currency, Money};

use super::{
    RpcError,
    at,
    date_str,
    exact_of,
    form_request,
    payloads,
    price_pair,
    str_at,
    unrecognized,
};
use crate::tfs::Search;

/// An inclusive range of dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DateRange {
    pub first: NaiveDate,
    pub last: NaiveDate,
}

impl DateRange {
    pub fn days(&self) -> i64 {
        (self.last - self.first).num_days() + 1
    }

    pub fn contains(&self, date: NaiveDate) -> bool {
        self.first <= date && date <= self.last
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarRequest {
    pub search: Search,
    pub departures: DateRange,
    /// The date grid's return range (round trips).
    pub returns: Option<DateRange>,
    /// The price graph's trip length in days, `(min, max)` (round trips).
    pub trip_length: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarCell {
    pub departure: NaiveDate,
    /// Set for round trips.
    pub return_date: Option<NaiveDate>,
    /// Whole units of the display currency; `None` when the page has
    /// no price for the date (sold out, no flights).
    pub price: Option<i64>,
    /// The exact amount and currency from the cell's token.
    pub exact: Option<Money>,
}

impl CalendarCell {
    /// The price as displayed, in the requested display currency.
    pub fn money(&self, display: &Currency) -> Option<Money> {
        self.price.map(|p| Money::from_whole(p, display.clone()))
    }
}

fn range(
    v: Option<&Value>,
    what: &'static str,
) -> Result<Option<DateRange>, RpcError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let d = |i| str_at(v, &[i]).and_then(date_str);
            match (d(0), d(1)) {
                (Some(first), Some(last)) if first <= last => {
                    Ok(Some(DateRange { first, last }))
                }
                _ => Err(unrecognized(what, v.to_string())),
            }
        }
    }
}

/// Parses a captured `GetCalendarGraph`/`GetCalendarGrid` form body.
pub fn parse_calendar_request(form: &str) -> Result<CalendarRequest, RpcError> {
    let req = form_request(form)?;
    let search = at(&req, &[1])
        .ok_or_else(|| unrecognized("calendar request", req.to_string()))?;
    let trip_length = match at(&req, &[4]) {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = |i| {
                at(v, &[i])
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            match (n(0), n(1)) {
                (Some(a), Some(b)) => Some((a, b)),
                _ => return Err(unrecognized("trip length", v.to_string())),
            }
        }
    };
    Ok(CalendarRequest {
        search: super::search_from_rpc(search)?,
        departures: range(at(&req, &[2]), "departure range")?
            .ok_or_else(|| unrecognized("departure range", req.to_string()))?,
        returns: range(at(&req, &[3]), "return range")?,
        trip_length,
    })
}

fn cell(v: &Value) -> Result<CalendarCell, RpcError> {
    let departure = str_at(v, &[0])
        .and_then(date_str)
        .ok_or_else(|| unrecognized("calendar cell", v.to_string()))?;
    let return_date = match at(v, &[1]) {
        None | Some(Value::Null) => None,
        Some(r) => Some(
            r.as_str()
                .and_then(date_str)
                .ok_or_else(|| unrecognized("calendar cell", v.to_string()))?,
        ),
    };
    let (price, token) = price_pair(at(v, &[2]))?;
    Ok(CalendarCell {
        departure,
        return_date,
        price,
        exact: exact_of(token.as_deref()),
    })
}

/// Every cell of a calendar response, graph or grid, one way or round
/// trip. When a response repeats a date (pair), the last one wins.
pub fn parse_calendar(body: &str) -> Result<Vec<CalendarCell>, RpcError> {
    let mut cells: Vec<CalendarCell> = Vec::new();
    for payload in payloads(body)? {
        let list = match at(&payload, &[1]) {
            Some(Value::Array(list)) => list,
            Some(Value::Null)
                if payload.as_array().is_some_and(|a| a.len() == 2) =>
            {
                continue;
            }
            _ => {
                return Err(unrecognized(
                    "calendar payload",
                    truncate(&payload),
                ));
            }
        };
        for v in list {
            let c = cell(v)?;
            match cells.iter_mut().find(|x| {
                x.departure == c.departure && x.return_date == c.return_date
            }) {
                Some(existing) => *existing = c,
                None => cells.push(c),
            }
        }
    }
    Ok(cells)
}

pub(crate) fn truncate(v: &Value) -> String {
    let s = v.to_string();
    s.chars().take(200).collect()
}
