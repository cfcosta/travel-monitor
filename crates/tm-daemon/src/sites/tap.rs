//! TAP Air Portugal's booking site, `booking.flytap.com` (DECISIONS.md
//! 0051; research: `docs/research/sites/tap.md`). Pure: the deep link,
//! the parsers for the two responses the page makes, block detection
//! and the page scripts. [`crate::sources::browser`] drives the page.
//!
//! - **Deep link** (`/booking/flights/deeplink`): opens the results of
//!   a one-way or round-trip search directly. No cabin parameter: the
//!   availability response holds every cabin.
//! - **Availability** (`bfm/rest/booking/availability/search`): every
//!   flight × fare family for the searched date, with cabins `M`
//!   (economy), `W` (Economy Prime) and `C` (business), and a 7-day
//!   strip of each day's lowest price.
//! - **Calendar** (`bfm/rest/booking/availability/calendar/`): one
//!   month of per-day one-way economy prices for one adult, fetched by
//!   the page's "Datas" step as each month scrolls into view. Every day
//!   is a cached price stamped with its `insertionDate`; only days
//!   stored within [`FRESH_HOURS`] are recorded.

use std::fmt::Write as _;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;
use tm_domain::{Cabin, Currency, FamilyKind, Money, Passengers};

/// TAP's IATA code, as in `Source::Airline { code }`.
pub const CODE: &str = "TP";
/// The rate-limit site and the seller recorded on its observations.
pub const SITE: &str = "airline:TP";
/// The booking engine.
pub const BASE: &str = "https://booking.flytap.com";
/// The month calendar's URL. It also matches `calendarReturns/` and
/// `calendar/prices/lowest/monthly`; [`is_calendar_url`] tells them
/// apart.
pub const CALENDAR_PATH: &str = "bfm/rest/booking/availability/calendar";
/// The availability search's URL.
pub const SEARCH_PATH: &str = "bfm/rest/booking/availability/search";

/// A calendar day is recorded only if TAP stored its price at most
/// this long before the check. The probe saw near months 0 to 3 days
/// old and far months up to three months old; two days keeps the near
/// months and drops what TAP has not refreshed (0051: stale days are
/// not fresh observations).
pub const FRESH_HOURS: i64 = 48;

/// The most months one check reads: TAP's calendar ends about ten
/// months ahead.
pub const MAX_MONTHS: u32 = 12;

/// Scrolls in a row that fetch no month before the calendar is taken
/// to have ended (each step is a third of a screen; a month is about a
/// screen tall).
pub const QUIET_SCROLLS: u32 = 8;

/// The oldest `insertionDate` still fresh at `now`.
pub fn fresh_since(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::TimeDelta::hours(FRESH_HOURS)
}

/// The months a check needs: from `first`'s month to `horizon_end`'s,
/// at most [`MAX_MONTHS`].
pub fn months_needed(first: NaiveDate, horizon_end: NaiveDate) -> u32 {
    let span = (horizon_end.year() - first.year()) * 12
        + horizon_end.month() as i32
        - first.month() as i32
        + 1;
    u32::try_from(span.max(1)).unwrap_or(1).min(MAX_MONTHS)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TapError {
    #[error("not JSON: {0}")]
    Json(String),
    #[error("unexpected shape: {0}")]
    Shape(&'static str),
    #[error("TAP returned an error: {0}")]
    Api(String),
}

// ---------------------------------------------------------------------
// What runs
// ---------------------------------------------------------------------

/// Why a TAP family cannot be checked, if it cannot.
///
/// - Round trips: TAP's calendar is one way only, and its round-trip
///   calendar (`calendarReturns/`) was not studied. Round trips are
///   priced from the legs (0042).
/// - Parties other than one adult: the calendar request carries
///   `paxType: "ADT"` and no count, so its prices are for one adult,
///   whatever the deep link's `adt`. Storing them for a larger party
///   would break 0036 (prices for the whole party), and multiplying
///   them would be a guess.
pub fn unsupported(
    kind: FamilyKind,
    passengers: &Passengers,
) -> Option<&'static str> {
    match kind {
        FamilyKind::RoundTrip { .. } => {
            Some("TAP's calendar is one way: round trips come from the legs")
        }
        FamilyKind::Explore => Some("TAP has no Explore"),
        FamilyKind::Leg { .. } if *passengers != Passengers::default() => {
            Some("TAP's calendar prices one adult only")
        }
        FamilyKind::Leg { .. } => None,
    }
}

// ---------------------------------------------------------------------
// The deep link
// ---------------------------------------------------------------------

/// A search, as the deep link expresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deeplink {
    /// The point of sale (`BR`, `US`, `PT`): it sets the currency and
    /// the selling office.
    pub market: String,
    /// `pt`, `en`…
    pub language: String,
    pub origin: String,
    pub destination: String,
    pub departure: NaiveDate,
    /// A round trip when set.
    pub return_date: Option<NaiveDate>,
    pub passengers: Passengers,
}

impl Deeplink {
    /// The deep link under `base` ([`BASE`], or a test server). `None`
    /// for a party with an infant in its own seat: the deep link has
    /// only `adt`, `yth`, `chd` and `inf` (on lap), and which of them
    /// TAP books a seated infant as is not known.
    pub fn url(&self, base: &str) -> Option<String> {
        let p = self.passengers;
        if p.infants_in_seat > 0 {
            return None;
        }
        let date = |d: NaiveDate| d.format("%d.%m.%Y").to_string();
        let mut url = format!(
            "{}/booking/flights/deeplink?market={}&language={}&origin={}\
             &destination={}&flexibleDates=false&flightType={}&adt={}&chd={}\
             &inf={}&yth=0&depDate={}",
            base.trim_end_matches('/'),
            query_value(&self.market.to_ascii_uppercase()),
            query_value(&self.language),
            query_value(&self.origin.to_ascii_uppercase()),
            query_value(&self.destination.to_ascii_uppercase()),
            if self.return_date.is_some() {
                "return"
            } else {
                "single"
            },
            p.adults,
            p.children,
            p.infants_on_lap,
            date(self.departure),
        );
        if let Some(r) = self.return_date {
            let _ = write!(url, "&retDate={}", date(r));
        }
        url.push_str("&x_tap_source=WEB");
        Some(url)
    }
}

/// The code TAP is searched with for a place code. TAP flies São Paulo
/// only from Guarulhos, so the SAO city (0005) is GRU; other codes are
/// passed as they are.
pub fn airport(code: &str) -> &str {
    match code {
        "SAO" => "GRU",
        other => other,
    }
}

/// TAP's `language` from a BCP 47 tag: `pt-BR` → `pt`.
pub fn language(tag: &str) -> String {
    let primary = tag.split(['-', '_']).next().unwrap_or("").trim();
    if primary.is_empty() {
        "en".to_owned()
    } else {
        primary.to_ascii_lowercase()
    }
}

/// Keeps letters, digits and `-`/`.`; anything else is dropped (codes
/// and languages never need more).
fn query_value(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        .collect()
}

// ---------------------------------------------------------------------
// Shared parsing
// ---------------------------------------------------------------------

fn json(text: &str) -> Result<Value, TapError> {
    serde_json::from_str(text).map_err(|e| TapError::Json(e.to_string()))
}

/// The `data` object, or the API's own error.
fn data(root: &Value) -> Result<&Value, TapError> {
    if let Some(errors) = root.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        return Err(TapError::Api(
            errors
                .iter()
                .map(|e| match e {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    root.get("data")
        .filter(|d| d.is_object())
        .ok_or(TapError::Shape("no data object"))
}

/// A decimal amount (`1911.99`) in minor units.
fn minor(v: &Value) -> Option<i64> {
    let amount = v.as_f64()?;
    if !amount.is_finite() || amount <= 0.0 {
        return None;
    }
    Some((amount * 100.0).round() as i64)
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `2026-11-02T00:00:00`, with or without fractions or a zone.
fn date_time(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim_end_matches('Z');
    let s = s.split('+').next().unwrap_or(s);
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok()
}

/// `10112026` (`ddMMyyyy`).
fn compact_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%d%m%Y").ok()
}

/// The cabin of a TAP cabin code.
pub fn cabin(code: &str) -> Option<Cabin> {
    match code {
        "M" => Some(Cabin::Economy),
        "W" => Some(Cabin::PremiumEconomy),
        "C" => Some(Cabin::Business),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// The calendar
// ---------------------------------------------------------------------

/// What the page asked the calendar for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarRequest {
    pub origin: String,
    pub destination: String,
    pub market: String,
    /// `O` one way.
    pub trip_type: String,
    /// `E`.
    pub cabin_class: String,
    pub year: i32,
    pub month: u32,
}

impl CalendarRequest {
    /// The month's first day.
    pub fn first_day(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, 1)
    }
}

/// Parses a calendar request body.
pub fn parse_calendar_request(body: &str) -> Result<CalendarRequest, TapError> {
    let v = json(body)?;
    let month = v
        .get("month")
        .and_then(Value::as_u64)
        .and_then(|m| u32::try_from(m).ok())
        .filter(|m| (1..=12).contains(m))
        .ok_or(TapError::Shape("no month"))?;
    let year = v
        .get("year")
        .and_then(Value::as_i64)
        .and_then(|y| i32::try_from(y).ok())
        .ok_or(TapError::Shape("no year"))?;
    Ok(CalendarRequest {
        origin: text(&v, "origin").unwrap_or_default(),
        destination: text(&v, "destination").unwrap_or_default(),
        market: text(&v, "market").unwrap_or_default(),
        trip_type: text(&v, "tripType").unwrap_or_default(),
        cabin_class: text(&v, "cabinClass").unwrap_or_default(),
        year,
        month,
    })
}

/// One calendar day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarDay {
    pub departure: NaiveDate,
    pub origin: String,
    pub destination: String,
    /// The lowest one-way price for one adult; `None` when the day is
    /// sold out, has no flights, or shows no amount.
    pub price: Option<Money>,
    /// When TAP stored the price (its own clock, taken as UTC).
    pub inserted: Option<NaiveDateTime>,
    pub sold_out: bool,
    pub no_flights: bool,
}

impl CalendarDay {
    /// Stored at or after `since` (see [`fresh_since`]). A day with no
    /// `insertionDate` is not fresh.
    pub fn is_fresh(&self, since: DateTime<Utc>) -> bool {
        self.inserted.is_some_and(|at| at >= since.naive_utc())
    }
}

/// Parses a calendar response: `data.bestPriceForDates[]`. An empty
/// list is a month past the calendar's end.
pub fn parse_calendar(body: &str) -> Result<Vec<CalendarDay>, TapError> {
    let root = json(body)?;
    let days = data(&root)?
        .get("bestPriceForDates")
        .and_then(Value::as_array)
        .ok_or(TapError::Shape("no bestPriceForDates"))?;
    let mut out = Vec::with_capacity(days.len());
    for d in days {
        let Some(departure) = d
            .get("departureDate")
            .and_then(Value::as_str)
            .and_then(date_time)
            .map(|t| t.date())
        else {
            continue;
        };
        let sold_out = d.get("soldOut").and_then(Value::as_bool) == Some(true);
        let no_flights =
            d.get("noFlights").and_then(Value::as_bool) == Some(true);
        let currency = text(d, "currency").filter(|c| !c.is_empty());
        let price = match (sold_out || no_flights, currency) {
            (false, Some(c)) => d
                .get("bestTotalPrice")
                .and_then(minor)
                .map(|m| Money::new(m, Currency::new(&c))),
            _ => None,
        };
        out.push(CalendarDay {
            departure,
            origin: text(d, "departureAirport").unwrap_or_default(),
            destination: text(d, "arrivalAirport").unwrap_or_default(),
            price,
            inserted: d
                .get("insertionDate")
                .and_then(Value::as_str)
                .and_then(date_time),
            sold_out,
            no_flights,
        });
    }
    Ok(out)
}

/// Whether a captured URL is the month calendar itself (not
/// `calendarReturns/` or `calendar/prices/lowest/monthly`).
pub fn is_calendar_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.trim_end_matches('/').ends_with(CALENDAR_PATH)
}

// ---------------------------------------------------------------------
// The availability search
// ---------------------------------------------------------------------

/// What the page searched for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub origin: String,
    pub destination: String,
    pub departure: NaiveDate,
    pub market: String,
    pub one_way: bool,
}

/// Parses an availability search request body.
pub fn parse_search_request(body: &str) -> Result<SearchRequest, TapError> {
    let v = json(body)?;
    let first = |key: &str| -> Option<String> {
        match v.get(key)? {
            Value::Array(a) => a.first()?.as_str().map(str::to_owned),
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    };
    Ok(SearchRequest {
        origin: first("origin").ok_or(TapError::Shape("no origin"))?,
        destination: first("destination")
            .ok_or(TapError::Shape("no destination"))?,
        departure: first("departureDate")
            .as_deref()
            .and_then(compact_date)
            .ok_or(TapError::Shape("no departureDate"))?,
        market: text(&v, "market").unwrap_or_default(),
        one_way: v.get("oneWay").and_then(Value::as_bool).unwrap_or(true),
    })
}

/// One flight of a segment list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub from: String,
    pub to: String,
    /// Local time at `from` (TAP writes it with a fake `Z`).
    pub departs: Option<NaiveDateTime>,
    /// Local time at `to`.
    pub arrives: Option<NaiveDateTime>,
    /// The marketing carrier.
    pub carrier: String,
    pub number: String,
    pub operated_by: Option<String>,
}

/// One itinerary (`listOutbound[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flight {
    pub id: i64,
    pub duration_minutes: Option<u32>,
    pub stops: Option<u8>,
    pub segments: Vec<Segment>,
}

impl Flight {
    /// The single carrier marketing every segment, or `"multi"`.
    pub fn carrier(&self) -> Option<String> {
        let first = self.segments.first()?.carrier.clone();
        if self.segments.iter().all(|s| s.carrier == first) {
            Some(first)
        } else {
            Some("multi".to_owned())
        }
    }
}

/// One flight × fare family (`offers.listOffers[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub id: i64,
    /// `M`, `W` or `C`.
    pub cabin_code: String,
    pub cabin: Option<Cabin>,
    /// `DISCINT`, `BASINT`, `CLAINT`, `PLUSINT` (economy), `RPEINT`,
    /// `TPEINT` (Economy Prime), `EXEINT`, `TOPINT` (business).
    pub fare_family: String,
    /// The total for the whole party.
    pub price: Money,
    /// The outbound flights (`listOutbound[].idFlight`) it applies to.
    pub flights: Vec<i64>,
}

/// One day of the 7-day strip (`outPanel.listTab[]`): the lowest price
/// of any cabin on that day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripDay {
    pub date: NaiveDate,
    pub price: Option<Money>,
}

/// An availability search response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    pub currency: Currency,
    /// The selling office (`SAOTP08AA` for BR).
    pub office: Option<String>,
    pub flights: Vec<Flight>,
    pub offers: Vec<Offer>,
    pub strip: Vec<StripDay>,
}

impl Availability {
    /// The cheapest offer in `cabin`, with its first flight.
    pub fn cheapest(&self, cabin: Cabin) -> Option<(&Offer, Option<&Flight>)> {
        let offer = self
            .offers
            .iter()
            .filter(|o| o.cabin == Some(cabin))
            .min_by_key(|o| o.price.minor)?;
        let flight = offer
            .flights
            .iter()
            .find_map(|id| self.flights.iter().find(|f| f.id == *id));
        Some((offer, flight))
    }
}

fn parse_segment(s: &Value) -> Segment {
    Segment {
        from: text(s, "departureAirport").unwrap_or_default(),
        to: text(s, "arrivalAirport").unwrap_or_default(),
        departs: s
            .get("departureDate")
            .and_then(Value::as_str)
            .and_then(date_time),
        arrives: s
            .get("arrivalDate")
            .and_then(Value::as_str)
            .and_then(date_time),
        carrier: text(s, "carrier").unwrap_or_default(),
        number: text(s, "flightNumber").unwrap_or_default(),
        operated_by: text(s, "operationCarrier"),
    }
}

fn parse_flight(f: &Value) -> Option<Flight> {
    Some(Flight {
        id: f.get("idFlight")?.as_i64()?,
        duration_minutes: f
            .get("duration")
            .and_then(Value::as_u64)
            .and_then(|d| u32::try_from(d).ok()),
        stops: f
            .get("numberOfStops")
            .and_then(Value::as_u64)
            .and_then(|s| u8::try_from(s).ok()),
        segments: f
            .get("listSegment")
            .and_then(Value::as_array)
            .map(|l| l.iter().map(parse_segment).collect())
            .unwrap_or_default(),
    })
}

/// Parses an availability search response (one way: the outbound
/// side).
pub fn parse_availability(body: &str) -> Result<Availability, TapError> {
    let root = json(body)?;
    let data = data(&root)?;
    let offers = data
        .get("offers")
        .filter(|o| o.is_object())
        .ok_or(TapError::Shape("no offers"))?;
    let currency = text(offers, "currency")
        .filter(|c| !c.is_empty())
        .map(|c| Currency::new(&c))
        .ok_or(TapError::Shape("no offers.currency"))?;
    let flights = data
        .get("listOutbound")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(parse_flight).collect())
        .unwrap_or_default();
    let mut list = Vec::new();
    for o in offers
        .get("listOffers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(price) = o.get("totalPrice").and_then(|p| p.get("price"))
        else {
            continue;
        };
        let Some(price) = minor(price) else { continue };
        let cabin_code = text(o, "outCabin").unwrap_or_default();
        list.push(Offer {
            id: o.get("idOffer").and_then(Value::as_i64).unwrap_or_default(),
            cabin: cabin(&cabin_code),
            cabin_code,
            fare_family: text(o, "outFareFamily").unwrap_or_default(),
            price: Money::new(price, currency.clone()),
            flights: o
                .get("groupFlights")
                .and_then(Value::as_array)
                .map(|g| {
                    g.iter()
                        .filter_map(|g| g.get("idOutBound")?.as_i64())
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    let panel_currency = data
        .get("outPanel")
        .and_then(|p| text(p, "currency"))
        .filter(|c| !c.is_empty())
        .map_or_else(|| currency.clone(), |c| Currency::new(&c));
    let strip = data
        .get("outPanel")
        .and_then(|p| p.get("listTab"))
        .and_then(Value::as_array)
        .map(|tabs| {
            tabs.iter()
                .filter_map(|t| {
                    let date = compact_date(t.get("date")?.as_str()?)?;
                    let price = t
                        .get("totalPrice")
                        .and_then(|p| p.get("price"))
                        .and_then(minor)
                        .map(|m| Money::new(m, panel_currency.clone()));
                    Some(StripDay { date, price })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Availability {
        currency,
        office: text(data, "officeId"),
        flights,
        offers: list,
        strip,
    })
}

// ---------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------

/// A Cloudflare challenge or block page, from the document's URL,
/// title and visible text. Cloudflare's `precursor` script runs on
/// every TAP page, so its presence alone is not a challenge.
pub fn detect_block(url: &str, title: &str, text: &str) -> Option<String> {
    let url_l = url.to_ascii_lowercase();
    if url_l.contains("__cf_chl") || url_l.contains("/cdn-cgi/challenge") {
        return Some("Cloudflare challenge (URL)".to_owned());
    }
    let title_l = title.trim().to_lowercase();
    let text_l = text.to_lowercase();
    let challenge_titles = ["just a moment", "um momento", "un momento"];
    if challenge_titles.iter().any(|t| title_l.starts_with(t)) {
        return Some(format!("Cloudflare challenge ({})", title.trim()));
    }
    if title_l.contains("attention required") && title_l.contains("cloudflare")
    {
        return Some("Cloudflare block page".to_owned());
    }
    let challenge_texts = [
        "verify you are human",
        "verifique se você é humano",
        "confirme que você é humano",
        "needs to review the security of your connection",
        "precisa revisar a segurança da sua conexão",
        "checking if the site connection is secure",
        "checking your browser before accessing",
    ];
    if let Some(t) = challenge_texts.iter().find(|t| text_l.contains(*t)) {
        return Some(format!("Cloudflare challenge (\"{t}\")"));
    }
    if text_l.contains("cloudflare")
        && (text_l.contains("you have been blocked")
            || text_l.contains("access denied")
            || text_l.contains("error 1020"))
    {
        return Some("Cloudflare block page".to_owned());
    }
    None
}

/// A captured API response that Cloudflare answered instead of TAP.
pub fn blocked_response(
    status: Option<u16>,
    headers: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    if headers
        .get("cf-mitigated")
        .is_some_and(|v| v.contains("challenge"))
    {
        return Some("Cloudflare challenged an API call".to_owned());
    }
    match status {
        Some(403 | 429) if headers.contains_key("cf-ray") => {
            Some(format!("Cloudflare answered an API call with {status:?}"))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Page scripts
// ---------------------------------------------------------------------

/// The document's title and visible text, as `[title, text]`.
pub const TITLE_AND_TEXT_JS: &str = "[document.title || '', \
     (document.body && document.body.innerText || '').slice(0, 20000)]";

/// Clicks the "Datas"/"Dates" step of the booking progress bar, which
/// opens the month calendar in the app (no page load). True once it
/// clicked, or when the calendar is already showing.
pub const OPEN_DATES_JS: &str = r#"(() => {
  if (location.pathname.includes('/booking/dates')) return 'already';
  const label = /^(\d+\s*[.)]?\s*)?(datas|dates|fechas)$/i;
  const all = [...document.querySelectorAll('a, button, li, span, div, p, [role="tab"], [role="link"], [role="button"]')]
    .filter(e => label.test((e.innerText || '').trim()))
    .filter(e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; });
  if (!all.length) return false;
  // The innermost match, clicked through its nearest clickable parent.
  const el = all.find(e => !all.some(o => o !== e && e.contains(o))) || all[0];
  const target = el.closest('a, button, [role="tab"], [role="link"], [role="button"], li') || el;
  target.scrollIntoView({block: 'center'});
  target.click();
  return 'clicked';
})()"#;

/// Scrolls the calendar down by about a third of a screen, in the
/// window and in every scrollable container. The page fetches a month
/// only when it comes into view: a live check (2026-10-06) with
/// screen-sized jumps skipped November and February, so the steps are
/// small and most of them fetch nothing. True when anything moved.
pub const SCROLL_CALENDAR_JS: &str = r#"(() => {
  const moved = [];
  const step = (e, h) => {
    const before = e.scrollTop;
    e.scrollTop = before + Math.max(h * 0.35, 200);
    if (e.scrollTop !== before) moved.push(e);
  };
  const root = document.scrollingElement || document.documentElement;
  step(root, window.innerHeight);
  for (const e of document.querySelectorAll('div, section, main, mat-dialog-content, cdk-virtual-scroll-viewport')) {
    if (e.scrollHeight > e.clientHeight + 40) {
      const o = getComputedStyle(e).overflowY;
      if (o === 'auto' || o === 'scroll') step(e, e.clientHeight);
    }
  }
  window.dispatchEvent(new Event('scroll'));
  return moved.length > 0;
})()"#;

// ---------------------------------------------------------------------
// Months
// ---------------------------------------------------------------------

/// The first day of `date`'s month.
pub fn month_of(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap_or(date)
}

/// Whether the months read reach `horizon_end`'s month.
pub fn covers(months: &[NaiveDate], horizon_end: NaiveDate) -> bool {
    let last = month_of(horizon_end);
    months.iter().any(|m| *m >= last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap_or_default()
    }

    #[test]
    fn deep_links_follow_the_form() {
        let link = Deeplink {
            market: "br".into(),
            language: language("pt-BR"),
            origin: "GRU".into(),
            destination: "LIS".into(),
            departure: day(2026, 11, 10),
            return_date: None,
            passengers: Passengers::default(),
        };
        assert_eq!(
            link.url(BASE).as_deref(),
            Some(
                "https://booking.flytap.com/booking/flights/deeplink?market=BR\
                 &language=pt&origin=GRU&destination=LIS&flexibleDates=false\
                 &flightType=single&adt=1&chd=0&inf=0&yth=0&depDate=10.11.2026\
                 &x_tap_source=WEB"
            )
        );
        let round = Deeplink {
            return_date: Some(day(2026, 11, 17)),
            passengers: Passengers {
                adults: 2,
                children: 1,
                infants_in_seat: 0,
                infants_on_lap: 1,
            },
            language: language("en"),
            ..link.clone()
        };
        let url = round.url("http://127.0.0.1:9/").unwrap_or_default();
        assert!(
            url.starts_with("http://127.0.0.1:9/booking/flights/deeplink?")
        );
        assert!(url.contains("&language=en&"));
        assert!(url.contains("&flightType=return&adt=2&chd=1&inf=1&yth=0&"));
        assert!(url.ends_with(
            "&depDate=10.11.2026&retDate=17.11.2026&x_tap_source=WEB"
        ));
        let seated = Deeplink {
            passengers: Passengers {
                infants_in_seat: 1,
                ..Passengers::default()
            },
            ..link
        };
        assert_eq!(seated.url(BASE), None);
    }

    #[test]
    fn only_one_adult_legs_run() {
        let one = Passengers::default();
        let leg = FamilyKind::Leg {
            leg: tm_domain::Leg::Return,
        };
        assert_eq!(unsupported(leg, &one), None);
        assert!(unsupported(FamilyKind::RoundTrip { days: 7 }, &one).is_some());
        let two = Passengers { adults: 2, ..one };
        assert!(unsupported(leg, &two).is_some());
    }

    #[test]
    fn calendar_urls_are_told_apart() {
        assert!(is_calendar_url(
            "https://booking.flytap.com/bfm/rest/booking/availability/calendar/"
        ));
        assert!(is_calendar_url(
            "http://127.0.0.1:1/bfm/rest/booking/availability/calendar/?x=1"
        ));
        assert!(!is_calendar_url(
            "https://booking.flytap.com/bfm/rest/booking/availability/calendarReturns/"
        ));
        assert!(!is_calendar_url(
            "https://booking.flytap.com/bfm/rest/booking/availability/calendar/prices/lowest/monthly?marketCode=BR"
        ));
    }

    #[test]
    fn errors_and_shapes() {
        assert!(matches!(parse_calendar("<html>"), Err(TapError::Json(_))));
        assert_eq!(
            parse_calendar(r#"{"status":"500","errors":["boom"],"data":null}"#),
            Err(TapError::Api("boom".into()))
        );
        assert_eq!(
            parse_calendar(r#"{"data":{"bestPriceForDates":[]}}"#),
            Ok(Vec::new())
        );
        let sold = parse_calendar(
            r#"{"data":{"bestPriceForDates":[{"departureDate":"2026-12-24T00:00:00",
            "bestTotalPrice":0,"currency":"BRL","soldOut":true,"noFlights":false,
            "insertionDate":"2026-10-06T01:00:00"}]}}"#,
        )
        .unwrap_or_default();
        assert_eq!(sold.len(), 1);
        assert_eq!(sold[0].price, None);
        assert!(sold[0].sold_out);
        assert!(matches!(
            parse_availability(r#"{"data":{}}"#),
            Err(TapError::Shape(_))
        ));
    }

    #[test]
    fn challenges_are_blocks() {
        assert!(
            detect_block(
                "https://booking.flytap.com/booking/flights",
                "Just a moment...",
                ""
            )
            .is_some()
        );
        assert!(
            detect_block(
                "https://booking.flytap.com/",
                "booking.flytap.com",
                "Verify you are human by completing the action below."
            )
            .is_some()
        );
        assert!(
            detect_block(
                "https://booking.flytap.com/?__cf_chl_rt_tk=abc",
                "",
                ""
            )
            .is_some()
        );
        assert_eq!(
            detect_block(
                "https://booking.flytap.com/booking/flights",
                "TAP Air Portugal",
                "Selecione o seu voo de ida GRU LIS R$ 1.911,99"
            ),
            None
        );
        let mut headers = std::collections::BTreeMap::new();
        assert_eq!(blocked_response(Some(200), &headers), None);
        headers.insert("cf-ray".into(), "x".into());
        assert!(blocked_response(Some(403), &headers).is_some());
        headers.insert("cf-mitigated".into(), "challenge".into());
        assert!(blocked_response(Some(200), &headers).is_some());
    }

    #[test]
    fn months_cover_a_horizon() {
        let months = [day(2026, 10, 1), day(2026, 11, 1)];
        assert!(covers(&months, day(2026, 11, 30)));
        assert!(!covers(&months, day(2026, 12, 1)));
        assert_eq!(month_of(day(2027, 4, 4)), day(2027, 4, 1));
        // 180 days from 2026-10-07: October to April.
        assert_eq!(months_needed(day(2026, 10, 7), day(2027, 4, 4)), 7);
        assert_eq!(months_needed(day(2026, 10, 7), day(2026, 10, 9)), 1);
        assert_eq!(months_needed(day(2026, 10, 7), day(2028, 4, 4)), 12);
    }
}
