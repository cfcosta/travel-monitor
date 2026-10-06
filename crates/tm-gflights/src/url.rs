//! Google Flights URLs: generated searches, booking pages, Explore and
//! the `q=` searches used to learn a city's Freebase ID (DECISIONS.md
//! 0041).

use thiserror::Error;
use tm_domain::{Cabin, CountryCode, Currency};

use crate::{
    proto::{self, Field},
    tfs::{self, Search},
};

pub const BASE: &str = "https://www.google.com/travel";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UrlError {
    /// The `q=` text has no phrasing for this cabin: "premium economy"
    /// makes the parser drop the destination and fall back to Explore
    /// (research, 2026-10-06).
    #[error("{0:?} cannot be expressed in q= text")]
    CabinNotExpressible(Cabin),
}

/// Language, point of sale and display currency (`hl`, `gl`, `curr`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Locale {
    pub hl: String,
    pub gl: CountryCode,
    pub curr: Currency,
}

impl Locale {
    pub fn new(hl: &str, gl: CountryCode, curr: Currency) -> Self {
        Self {
            hl: hl.to_owned(),
            gl,
            curr,
        }
    }

    /// English UI, the given point of sale, prices in BRL: what every
    /// research capture used. The parsers' date and price labels
    /// assume `hl=en`.
    pub fn english(gl: &str) -> Self {
        Self::new("en", CountryCode::new(gl), Currency::brl())
    }

    fn query(&self) -> String {
        format!(
            "hl={}&gl={}&curr={}",
            encode_component(&self.hl),
            encode_component(self.gl.as_str()),
            encode_component(self.curr.as_str())
        )
    }
}

/// Percent-encodes everything but RFC 3986 unreserved characters
/// (spaces become `%20`, as in the research URLs).
pub fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Decodes `%XX` escapes and, for form bodies, `+` as a space.
pub fn decode_component(text: &str, plus_is_space: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' if plus_is_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The results page for a search.
pub fn search_url(search: &Search, locale: &Locale) -> String {
    format!(
        "{BASE}/flights/search?tfs={}&{}",
        tfs::encode(search),
        locale.query()
    )
}

/// The booking page for a search whose legs carry their selected
/// flights ([`tfs::LegSpec::flights`]).
///
/// `selection` is the price token of the chosen itinerary, as returned
/// by `GetShoppingResults` under the same point of sale; it becomes the
/// `tfu` parameter. In the research, a booking URL loaded under a
/// foreign `gl` with a `tfu` from a BR session listed only the airline,
/// while the same itinerary reached through that `gl`'s own search page
/// listed every agency. Without `tfu` the page has not been tried.
pub fn booking_url(
    search: &Search,
    selection: Option<&str>,
    locale: &Locale,
) -> String {
    let mut url = format!("{BASE}/flights/booking?tfs={}", tfs::encode(search));
    if let Some(token) = selection {
        url.push_str("&tfu=");
        url.push_str(&tfu(token));
    }
    url.push('&');
    url.push_str(&locale.query());
    url
}

/// The `tfu` value Google put on booking URLs:
/// `{ 1: <price token>, 2: { 1: 0 }, 4: { 1: "1" } }`.
pub fn tfu(token: &str) -> String {
    proto::b64_encode(&proto::encode(&[
        Field::bytes(1, token.as_bytes()),
        Field::message(2, &[Field::varint(1, 0)]),
        Field::message(4, &[Field::bytes(1, "1")]),
    ]))
}

/// The cabin as `q=` text, if it can be said there.
fn cabin_phrase(cabin: Cabin) -> Result<&'static str, UrlError> {
    match cabin {
        Cabin::Economy => Ok(""),
        Cabin::Business => Ok(" business class"),
        Cabin::First => Ok(" first class"),
        Cabin::PremiumEconomy => Err(UrlError::CabinNotExpressible(cabin)),
    }
}

/// Explore from an origin ("1-week trip in the next 6 months"):
/// `q=Flights from SAO`, plus " business class" for business. Premium
/// economy has no `q=` phrasing and is an error.
pub fn explore_url(
    origin_query: &str,
    cabin: Cabin,
    locale: &Locale,
) -> Result<String, UrlError> {
    let q = format!("Flights from {origin_query}{}", cabin_phrase(cabin)?);
    Ok(format!(
        "{BASE}/explore?q={}&{}",
        encode_component(&q),
        locale.query()
    ))
}

/// A `q=Flights to <text>` search, used once to learn the Freebase ID
/// of a city added in the app. The page embeds its search in the
/// HTML (no RPC is made): read it with
/// [`crate::page::DATA_SERVICE_REQUESTS_JS`] and
/// [`crate::rpc::parse_data_service_requests`]; the first leg's `to`
/// is the place Google resolved (`"Rome"` → `/m/06c62`).
pub fn resolve_url(text: &str, locale: &Locale) -> String {
    let q = format!("Flights to {text}");
    format!(
        "{BASE}/flights?q={}&{}",
        encode_component(&q),
        locale.query()
    )
}

/// The value of a query parameter in a URL, percent-decoded.
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == name).then(|| decode_component(v, true))
    })
}

/// Decodes the `tfs` parameter of a search or booking URL.
pub fn search_of(url: &str) -> Option<Result<Search, tfs::TfsError>> {
    query_param(url, "tfs").map(|t| tfs::decode(&t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explore_urls() {
        let br = Locale::english("BR");
        assert_eq!(
            explore_url("SAO", Cabin::Economy, &br).ok().as_deref(),
            Some(
                "https://www.google.com/travel/explore?q=Flights%20from%20SAO&hl=en&gl=BR&curr=BRL"
            )
        );
        assert_eq!(
            explore_url("SAO", Cabin::Business, &br).ok().as_deref(),
            Some(
                "https://www.google.com/travel/explore?q=Flights%20from%20SAO%20business%20class&hl=en&gl=BR&curr=BRL"
            )
        );
        assert_eq!(
            explore_url("SAO", Cabin::PremiumEconomy, &br),
            Err(UrlError::CabinNotExpressible(Cabin::PremiumEconomy))
        );
    }

    #[test]
    fn resolve() {
        assert_eq!(
            resolve_url("Rome", &Locale::english("BR")),
            "https://www.google.com/travel/flights?q=Flights%20to%20Rome&hl=en&gl=BR&curr=BRL"
        );
    }

    #[test]
    fn percent_round_trip() {
        for s in ["São Paulo", "a+b c", "100%", "%zz", "%4", ""] {
            assert_eq!(decode_component(&encode_component(s), false), s);
        }
        assert_eq!(decode_component("a+b%20c", true), "a b c");
        assert_eq!(decode_component("%4", false), "%4");
    }

    #[test]
    fn query_params() {
        let url = "https://x/y?tfs=AB_c&tfu=x%3D&hl=en#frag";
        assert_eq!(query_param(url, "tfs").as_deref(), Some("AB_c"));
        assert_eq!(query_param(url, "tfu").as_deref(), Some("x="));
        assert_eq!(query_param(url, "hl").as_deref(), Some("en"));
        assert_eq!(query_param(url, "gl"), None);
    }

    #[test]
    fn tfu_matches_the_captured_shape() {
        // From the gl=BR booking URL in fixtures/tfs_urls.txt.
        let token = "CjRIRUctVUM3OUU3NVVBSEdYVFFCRy0tLS0tLWNlYnUxMy1uMkFBQUFBR3JFZllzR0hnOGlBEgRUUDg5GgsIzNwYEAIaA0JSTDgccIT5BA==";
        assert_eq!(
            tfu(token),
            "CmxDalJJUlVjdFZVTTNPVVUzTlZWQlNFZFlWRkZDUnkwdExTMHRMV05sWW5VeE15MXVNa0ZCUVVGQlIzSkZabGx6UjBobk9HbEJFZ1JVVURnNUdnc0l6TndZRUFJYUEwSlNURGdjY0lUNUJBPT0SAggAIgMKATE"
        );
    }
}
