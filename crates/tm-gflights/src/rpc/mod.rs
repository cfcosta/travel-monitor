//! The page's own RPCs to
//! `/_/FlightsFrontendUi/data/travel.frontend.flights.FlightsFrontendService/<Method>`
//! and the data the page embeds in its HTML (DECISIONS.md 0032).
//!
//! The daemon never calls these RPCs; it reads what the page fetched
//! (through CDP) and hands the bodies to these parsers. Parsers ignore
//! fields they do not read. What they read must have the expected
//! shape: a missing price is `None`, a structure they cannot recognise
//! is an [`RpcError`], which the daemon treats as a format change
//! (`Unexpected`, 0059).

mod booking;
mod calendar;
mod explore;
mod search;
mod shopping;

pub use booking::{Seller, SellerOffer, parse_booking_results};
pub use calendar::{
    CalendarCell,
    CalendarRequest,
    DateRange,
    parse_calendar,
    parse_calendar_request,
};
use chrono::NaiveDate;
pub use explore::{ExploreDestination, parse_explore};
pub use search::{parse_data_service_requests, search_from_rpc};
use serde_json::Value;
pub use shopping::{
    Itinerary,
    Segment,
    parse_shopping_results,
    shopping_results_from_init_data,
};
use thiserror::Error;
use tm_domain::{Currency, Money};

use crate::proto;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RpcError {
    #[error("malformed response envelope: {0}")]
    Envelope(String),
    #[error("invalid JSON: {0}")]
    Json(String),
    /// The RPC answered with an error status (`[13]` when BotGuard
    /// rejected a replay).
    #[error("the RPC returned error {0}")]
    Server(i64),
    #[error("unrecognised {what}: {detail}")]
    Unrecognized { what: &'static str, detail: String },
    #[error("malformed form body: {0}")]
    Form(String),
}

pub(crate) fn unrecognized(
    what: &'static str,
    detail: impl Into<String>,
) -> RpcError {
    RpcError::Unrecognized {
        what,
        detail: detail.into(),
    }
}

/// The services' methods, named without their `Get` prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    ShoppingResults,
    CalendarGraph,
    CalendarGrid,
    ExploreDestinations,
    BookingResults,
    /// Any other method of the flights service.
    Other(String),
}

impl Method {
    pub fn name(&self) -> &str {
        match self {
            Method::ShoppingResults => "GetShoppingResults",
            Method::CalendarGraph => "GetCalendarGraph",
            Method::CalendarGrid => "GetCalendarGrid",
            Method::ExploreDestinations => "GetExploreDestinations",
            Method::BookingResults => "GetBookingResults",
            Method::Other(name) => name,
        }
    }
}

/// The substring that identifies the flights service's RPC URLs, for
/// the daemon's network capture filter.
pub const SERVICE_PATH: &str = "/data/travel.frontend.flights.";

/// Which RPC a captured request URL is, if it is one of the flights
/// service's.
pub fn method_of(url: &str) -> Option<Method> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if !path.contains(SERVICE_PATH) {
        return None;
    }
    let name = path.rsplit('/').next().filter(|n| !n.is_empty())?;
    Some(match name {
        "GetShoppingResults" => Method::ShoppingResults,
        "GetCalendarGraph" => Method::CalendarGraph,
        "GetCalendarGrid" => Method::CalendarGrid,
        "GetExploreDestinations" => Method::ExploreDestinations,
        "GetBookingResults" => Method::BookingResults,
        other => Method::Other(other.to_owned()),
    })
}

/// One entry of a response chunk, such as
/// `["wrb.fr", null, "<inner JSON>"]` or `["di", 57]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    /// `"wrb.fr"`, `"di"`, `"af.httprm"`, `"e"`, …
    pub kind: String,
    pub entry: Vec<Value>,
}

impl Envelope {
    /// The inner JSON string of a `wrb.fr` entry.
    pub fn payload(&self) -> Option<&str> {
        if self.kind != "wrb.fr" {
            return None;
        }
        self.entry.get(2).and_then(Value::as_str)
    }

    /// The error status of a `wrb.fr` entry without a payload
    /// (`["wrb.fr", null, null, null, null, [13]]`).
    pub fn error_code(&self) -> Option<i64> {
        if self.kind != "wrb.fr" || self.payload().is_some() {
            return None;
        }
        self.entry.iter().skip(3).find_map(|v| {
            v.as_array().and_then(|a| a.first()).and_then(Value::as_i64)
        })
    }
}

/// Decodes a response body: the `)]}'` guard, then either `rt=c`
/// chunks (a length line followed by a JSON array) or one bare JSON
/// array. The length prefixes are not trusted; each chunk is read as
/// one JSON value.
pub fn frames(body: &str) -> Result<Vec<Envelope>, RpcError> {
    let mut rest = body.trim_start();
    rest = rest.strip_prefix(")]}'").unwrap_or(rest);
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        rest = &rest[digits..];
        if !rest.trim_start().starts_with('[') {
            return Err(RpcError::Envelope(format!(
                "expected a JSON array, found {:?}",
                rest.chars().take(20).collect::<String>()
            )));
        }
        let mut stream =
            serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        let chunk = match stream.next() {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(RpcError::Json(e.to_string())),
            None => break,
        };
        rest = &rest[stream.byte_offset()..];
        let Value::Array(entries) = chunk else {
            return Err(RpcError::Envelope("chunk is not an array".into()));
        };
        for entry in entries {
            let Value::Array(entry) = entry else {
                return Err(RpcError::Envelope("entry is not an array".into()));
            };
            let kind = entry
                .first()
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    RpcError::Envelope("entry without a kind".into())
                })?
                .to_owned();
            out.push(Envelope { kind, entry });
        }
    }
    Ok(out)
}

/// Every `wrb.fr` payload of a response, parsed, in order. An error
/// entry fails the whole response.
pub fn payloads(body: &str) -> Result<Vec<Value>, RpcError> {
    let envelopes = frames(body)?;
    let mut out = Vec::new();
    for env in envelopes.iter().filter(|e| e.kind == "wrb.fr") {
        if let Some(code) = env.error_code() {
            return Err(RpcError::Server(code));
        }
        if let Some(text) = env.payload() {
            out.push(
                serde_json::from_str(text)
                    .map_err(|e| RpcError::Json(e.to_string()))?,
            );
        }
    }
    if out.is_empty() {
        return Err(unrecognized("response", "no wrb.fr payload"));
    }
    Ok(out)
}

/// The inner request of a form body `f.req=[null,"<inner>"]&at=…`.
pub fn form_request(form: &str) -> Result<Value, RpcError> {
    let raw = form
        .split('&')
        .find_map(|pair| pair.strip_prefix("f.req="))
        .ok_or_else(|| RpcError::Form("no f.req".into()))?;
    let outer: Value =
        serde_json::from_str(&crate::url::decode_component(raw, true))
            .map_err(|e| RpcError::Form(e.to_string()))?;
    let inner = outer
        .get(1)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::Form("f.req is not [null, \"…\"]".into()))?;
    serde_json::from_str(inner).map_err(|e| RpcError::Form(e.to_string()))
}

/// Reads one JSON value starting at `text`, ignoring what follows.
fn first_json(text: &str) -> Result<Value, RpcError> {
    let mut stream =
        serde_json::Deserializer::from_str(text).into_iter::<Value>();
    match stream.next() {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => Err(RpcError::Json(e.to_string())),
        None => Err(RpcError::Json("no JSON value".into())),
    }
}

/// Parses a `AF_initDataCallback({key: 'ds:1', hash: '…', data:[…], …})`
/// script, as read with [`crate::page::init_data_js`]. Returns the key
/// and the data, which has the shape of the matching RPC's payload
/// (`ds:1` on a results page is the first `GetShoppingResults`).
pub fn parse_init_data(script: &str) -> Result<(String, Value), RpcError> {
    let key = script
        .split_once("key: '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(key, _)| key.to_owned())
        .ok_or_else(|| unrecognized("init data", "no key"))?;
    let data = script
        .split_once("data:")
        .map(|(_, rest)| rest)
        .ok_or_else(|| unrecognized("init data", "no data"))?;
    Ok((key, first_json(data)?))
}

/// `v[i][j]…`, if every step exists.
pub(crate) fn at<'a>(v: &'a Value, path: &[usize]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, &i| v.as_array()?.get(i))
}

pub(crate) fn str_at<'a>(v: &'a Value, path: &[usize]) -> Option<&'a str> {
    at(v, path).and_then(Value::as_str)
}

pub(crate) fn date_str(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// `[[null, 3951], "<token>"]` → the whole-unit price and the token.
pub(crate) fn price_pair(
    v: Option<&Value>,
) -> Result<(Option<i64>, Option<String>), RpcError> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok((None, None));
    };
    let arr = v
        .as_array()
        .ok_or_else(|| unrecognized("price", v.to_string()))?;
    let price = match arr.first() {
        None | Some(Value::Null) => None,
        Some(p) => match p.as_array().and_then(|p| p.get(1)) {
            None | Some(Value::Null) => None,
            Some(n) => Some(
                as_whole(n)
                    .ok_or_else(|| unrecognized("price", n.to_string()))?,
            ),
        },
    };
    let token = arr.get(1).and_then(Value::as_str).map(str::to_owned);
    Ok((price, token))
}

/// A JSON number as whole units (rounded if fractional).
pub(crate) fn as_whole(n: &Value) -> Option<i64> {
    n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64))
}

/// What a price token (the base64 protobuf next to every price) says:
/// `{ 1: session, 2: label, 3: { 1: amount, 2: decimals, 3: currency } }`.
/// The amount is exact (`382485` with 2 decimals is R$3,824.85, shown
/// as R$3,825).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceToken {
    /// `"GRU-LIS:2026-11-12_2026-11-19"` (Explore), `"TP1924|TP93"`
    /// (results), `"options:1"` (booking), `"15:22"` (calendar).
    pub label: Option<String>,
    pub amount: Option<Money>,
}

pub fn decode_price_token(token: &str) -> Result<PriceToken, RpcError> {
    let bad = |e: String| unrecognized("price token", e);
    let fields = proto::decode(
        &proto::b64_decode(token).map_err(|e| bad(e.to_string()))?,
    )
    .map_err(|e| bad(e.to_string()))?;
    let label = fields
        .iter()
        .find(|f| f.number == 2)
        .and_then(|f| f.as_str())
        .map(str::to_owned);
    let amount = match fields
        .iter()
        .find(|f| f.number == 3)
        .and_then(|f| f.as_bytes())
    {
        None => None,
        Some(bytes) => {
            let sub = proto::decode(bytes).map_err(|e| bad(e.to_string()))?;
            let get = |n| sub.iter().find(|f| f.number == n);
            let units = get(1).and_then(|f| f.as_varint());
            let decimals = get(2).and_then(|f| f.as_varint()).unwrap_or(2);
            let currency = get(3).and_then(|f| f.as_str());
            match (units, currency) {
                (Some(units), Some(currency)) => Some(Money::new(
                    to_minor(units, decimals)?,
                    Currency::new(currency),
                )),
                _ => None,
            }
        }
    };
    Ok(PriceToken { label, amount })
}

/// Scales an amount with `decimals` decimal places to cents.
fn to_minor(units: u64, decimals: u64) -> Result<i64, RpcError> {
    let units = i64::try_from(units)
        .map_err(|_| unrecognized("price token", "amount overflow"))?;
    let scaled = match decimals {
        0 => units.checked_mul(100),
        1 => units.checked_mul(10),
        2 => Some(units),
        d if d <= 18 => Some(units / 10_i64.pow((d - 2) as u32)),
        _ => None,
    };
    scaled.ok_or_else(|| unrecognized("price token", "amount overflow"))
}

/// The exact amount behind a token, if it decodes.
pub(crate) fn exact_of(token: Option<&str>) -> Option<Money> {
    token
        .and_then(|t| decode_price_token(t).ok())
        .and_then(|t| t.amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods() {
        let url = "/_/FlightsFrontendUi/data/travel.frontend.flights.FlightsFrontendService/GetCalendarGraph?f.sid=1&bl=x";
        assert_eq!(method_of(url), Some(Method::CalendarGraph));
        assert_eq!(
            method_of(
                "https://www.google.com/_/FlightsFrontendUi/data/travel.frontend.flights.FlightsFrontendService/GetBookingResults"
            ),
            Some(Method::BookingResults)
        );
        assert_eq!(
            method_of(
                "/x/data/travel.frontend.flights.FlightsFrontendService/GetFoo?a"
            ),
            Some(Method::Other("GetFoo".into()))
        );
        assert_eq!(
            method_of("https://www.google.com/travel/flights?q=x"),
            None
        );
        assert_eq!(Method::CalendarGrid.name(), "GetCalendarGrid");
    }

    #[test]
    fn bare_and_error_bodies() {
        let bare = ")]}'\n[[\"wrb.fr\",null,\"[1,2]\"]]";
        assert_eq!(payloads(bare), Ok(vec![serde_json::json!([1, 2])]));
        let err =
            ")]}'\n\n100\n[[\"wrb.fr\",null,null,null,null,[13]],[\"e\",4]]\n";
        assert_eq!(payloads(err), Err(RpcError::Server(13)));
        assert!(payloads(")]}'\n12\n[[\"di\",57]]").is_err());
        assert!(frames(")]}'\n12\nnot json").is_err());
    }

    #[test]
    fn price_tokens() {
        let t = decode_price_token(
            "CjRIekR3TFpwQXg1cjRBRHBLeEFCRy0tLS0tLWNlZ3gxNS1uMkFBQUFBR3JFYUhzQjFYdUFBEh1HUlUtTElTOjIwMjYtMTEtMTJfMjAyNi0xMS0xORoLCJWsFxACGgNCUkw4KXDX1QQ=",
        );
        assert_eq!(
            t,
            Ok(PriceToken {
                label: Some("GRU-LIS:2026-11-12_2026-11-19".into()),
                amount: Some(Money::new(382_485, Currency::brl())),
            })
        );
        assert!(decode_price_token("@@").is_err());
        assert_eq!(to_minor(5, 0), Ok(500));
        assert_eq!(to_minor(12_345, 3), Ok(1_234));
    }
}
