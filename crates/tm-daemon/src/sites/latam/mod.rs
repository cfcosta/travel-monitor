//! LATAM Airlines (`airline:LA`) and LATAM Pass (`miles:latam_pass`) on
//! `www.latamairlines.com` (DECISIONS.md 0081, 0082, 0084, 0085;
//! research: `docs/research/sites/latam.md` and `latam-pass.md`). Pure:
//! the results URL, the parsers for the responses the results page
//! makes, and how a blocked or logged-out page looks.
//! [`crate::sources::latam`] drives the page; [`dates`] chooses which
//! departures a check reads and [`page`] holds the page scripts.
//!
//! - **Results URL** (`/br/pt/oferta-voos`): one search, cash or award
//!   (`redemption=true`), one way or round trip. The page then calls
//!   the offers endpoint itself.
//! - **Offers** (`/bff/air-offers/v2/offers/search` for cash,
//!   `…/offers/search/redemption` for miles): every itinerary × fare
//!   brand, every cabin, of one leg. Cash prices are BRL with taxes,
//!   split into fare and taxes in `newPrices`; award prices are miles,
//!   with the fare before taxes (`priceWithOutTax`) and the taxes in
//!   BRL. A round trip is two calls: the outbound, then (after a brand
//!   is chosen) the return with `outOfferId=<offer>`.
//! - **Calendar** (`/bff/air-offers/v2/calendar…&extended=true`): two
//!   months of the cheapest economy cash fare per day, cached and a few
//!   percent high. It ranks dates and is never recorded (0053, 0085).
//!
//! Miles are the award fare before taxes × [`MILES_PER_BRL`], rounded,
//! except for silent route discounts (× 0.95, × 0.92 seen); the award
//! fare is the cash fare of the same fare basis, when the award search
//! offers that bucket ([`pair_fares`], latam-pass.md "Measuring cash
//! against miles").

pub mod dates;
pub mod page;

use std::fmt::Write as _;

use chrono::{NaiveDate, NaiveDateTime};
use serde_json::Value;
use tm_domain::{Cabin, Currency, FamilyKind, Money, Passengers};

/// LATAM's IATA code, as in `Source::Airline { code }`.
pub const CODE: &str = "LA";
/// The cash site.
pub const AIRLINE_SITE: &str = "airline:LA";
/// LATAM Pass, as in `Source::Miles { program }`.
pub const PROGRAM: &str = "latam_pass";
/// The award site.
pub const MILES_SITE: &str = "miles:latam_pass";
/// The site's origin.
pub const BASE: &str = "https://www.latamairlines.com";
/// The cash offers call; the award one is [`REDEMPTION_PATH`].
pub const SEARCH_PATH: &str = "/bff/air-offers/v2/offers/search";
/// The award offers call.
pub const REDEMPTION_PATH: &str = "/bff/air-offers/v2/offers/search/redemption";
/// The calendar call (the date strip's prices).
pub const CALENDAR_PATH: &str = "/bff/air-offers/v2/calendar";
/// What a check captures: every BFF call of the results page, offers
/// and calendar alike.
pub const CAPTURE: &str = "/bff/air-offers/v2/";
/// The cookie that holds a LATAM Pass login on the site's own host,
/// renewed by every page load (0082;
/// `tm_browser::profile::KNOWN_SESSIONS`).
pub const SESSION_COOKIE: &str = "_xp_session";

/// Miles per BRL of award fare before taxes: R$ 0,0288 a mile, R$ 28,80
/// per 1,000. 1,042 of 1,120 award brands probed on 2026-10-07 had
/// exactly this rate; the rest had a discount ([`Fare::discount`]).
pub const MILES_PER_BRL: f64 = 1.0 / 0.0288;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LatamError {
    #[error("not JSON: {0}")]
    Json(String),
    #[error("unexpected shape: {0}")]
    Shape(&'static str),
}

// ---------------------------------------------------------------------
// What runs
// ---------------------------------------------------------------------

/// Why a LATAM family cannot be checked, if it cannot.
///
/// - Explore: LATAM has none.
/// - Parties other than one adult: the page prices each passenger
///   (latam.md §4), and storing per-person prices for a party would
///   break 0036; multiplying them would be a guess, as for TAP (0053).
pub fn unsupported(
    kind: FamilyKind,
    passengers: &Passengers,
) -> Option<&'static str> {
    match kind {
        FamilyKind::Explore => Some("LATAM has no Explore"),
        _ if *passengers != Passengers::default() => {
            Some("LATAM's results price one passenger")
        }
        _ => None,
    }
}

/// The airport LATAM is searched at for a place code. The results URL
/// was only tested with airports (latam.md, open question 5), so a city
/// is searched at its main airport for LATAM and its partners; other
/// codes are passed as they are.
pub fn airport(code: &str) -> &str {
    match code {
        "SAO" => "GRU",
        "RIO" => "GIG",
        "BUE" => "EZE",
        "NYC" => "JFK",
        "LON" => "LHR",
        "PAR" => "CDG",
        "ROM" => "FCO",
        "MIL" => "MXP",
        "WAS" => "IAD",
        "TYO" => "NRT",
        "SEL" => "ICN",
        other => other,
    }
}

// ---------------------------------------------------------------------
// The results URL
// ---------------------------------------------------------------------

/// A search, as the results URL expresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub origin: String,
    pub destination: String,
    pub outbound: NaiveDate,
    /// A round trip when set.
    pub inbound: Option<NaiveDate>,
    pub passengers: Passengers,
    /// `Business` returns only business brands; anything else searches
    /// `Economy`, which returns every cabin (latam.md §2).
    pub cabin: Cabin,
    /// LATAM Pass miles instead of cash.
    pub redemption: bool,
}

impl Search {
    /// The Brazilian results page under `base` ([`BASE`], or a test
    /// server). `None` for a party with an infant in its own seat: the
    /// URL has only `adt`, `chd` and `inf`, and which of them LATAM
    /// books a seated infant as is not known.
    pub fn url(&self, base: &str) -> Option<String> {
        let p = self.passengers;
        if p.infants_in_seat > 0 {
            return None;
        }
        // Noon UTC: the page reads the date part only, and noon keeps
        // it from shifting with the time zone.
        let date = |d: NaiveDate| {
            format!("{}T12%3A00%3A00.000Z", d.format("%Y-%m-%d"))
        };
        let mut url = format!(
            "{}/br/pt/oferta-voos?origin={}&destination={}&outbound={}",
            base.trim_end_matches('/'),
            code(&self.origin),
            code(&self.destination),
            date(self.outbound),
        );
        match self.inbound {
            Some(d) => {
                let _ = write!(url, "&inbound={}", date(d));
            }
            None => url.push_str("&inbound=null"),
        }
        let _ = write!(
            url,
            "&adt={}&chd={}&inf={}&trip={}&cabin={}&redemption={}&sort=RECOMMENDED",
            p.adults,
            p.children,
            p.infants_on_lap,
            if self.inbound.is_some() { "RT" } else { "OW" },
            if self.cabin == Cabin::Business {
                "Business"
            } else {
                "Economy"
            },
            self.redemption,
        );
        Some(url)
    }
}

/// An airport code for the URL: letters only, upper case.
fn code(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

// ---------------------------------------------------------------------
// Which call a captured URL is
// ---------------------------------------------------------------------

/// A captured offers call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffersCall {
    /// `offers/search`, BRL.
    Cash,
    /// `offers/search/redemption`, miles.
    Award,
}

impl OffersCall {
    /// The offers call `url` is, if it is one (not `…/promo/`,
    /// `brand-bundle-details` or the calendar).
    pub fn of(url: &str) -> Option<Self> {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let path = path.trim_end_matches('/');
        if path.ends_with(REDEMPTION_PATH) {
            Some(OffersCall::Award)
        } else if path.ends_with(SEARCH_PATH) {
            Some(OffersCall::Cash)
        } else {
            None
        }
    }
}

/// Whether an offers call is a round trip's return leg: it names the
/// chosen outbound (`outOfferId=<offer>`, not `null`).
pub fn is_return_leg(url: &str) -> bool {
    query_value(url, "outOfferId").is_some_and(|v| !v.is_empty() && v != "null")
}

/// The first value of `key` in `url`'s query, as written.
fn query_value<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then_some(v)
    })
}

/// The searched outbound date of an offers call (`outFrom`).
pub fn out_from(url: &str) -> Option<NaiveDate> {
    date_param(url, "outFrom")
}

/// The searched return date of an offers call (`inFrom`), for a round
/// trip.
pub fn in_from(url: &str) -> Option<NaiveDate> {
    date_param(url, "inFrom")
}

fn date_param(url: &str, key: &str) -> Option<NaiveDate> {
    let v = query_value(url, key)?;
    NaiveDate::parse_from_str(v.get(..10)?, "%Y-%m-%d").ok()
}

/// Whether the page ended on LATAM's login: an award URL while logged
/// out redirects to `auth.latamairlines.com/u/login/…` (latam-pass.md
/// §1), and any page on the login host means the session is gone.
/// Auth0's path (`/u/login`) on another host counts too, for a test
/// server standing in for both.
pub fn is_login_wall(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    host.eq_ignore_ascii_case("auth.latamairlines.com")
        || path.starts_with("u/login")
}

// ---------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------

/// Akamai's "Access Denied" page in place of LATAM's, from the
/// document's title and visible text (latam.md §5; the same page in
/// Spanish, English and Portuguese).
pub fn detect_block(title: &str, text: &str) -> Option<String> {
    let title_l = title.trim().to_lowercase();
    let text_l = text.to_lowercase();
    if title_l.starts_with("access denied") {
        return Some("Akamai Access Denied page".to_owned());
    }
    let denials = [
        "for security reasons, we cannot grant you access",
        "por razões de segurança, nâo é possível permitir seu acesso",
        "por razões de segurança, não é possível permitir seu acesso",
        "por motivos de seguridad, no pudimos permitir tu acceso",
        "you don't have permission to access",
    ];
    denials
        .iter()
        .find(|d| text_l.contains(*d))
        .map(|d| format!("Akamai denied the page (\"{d}\")"))
}

/// A captured BFF call that Akamai answered instead of LATAM: the
/// headed Xvfb probe got 403 with the "Access Denied" page on
/// `offers/search` (latam.md, "Headed Chromium in a virtual display").
pub fn blocked_response(url: &str, status: Option<u16>) -> Option<String> {
    let call = if OffersCall::of(url).is_some() {
        "the offers search"
    } else {
        "a LATAM API call"
    };
    match status {
        Some(403) => Some(format!("Akamai denied {call} (403)")),
        Some(429) => Some(format!("LATAM rate-limited {call} (429)")),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// The offers response
// ---------------------------------------------------------------------

/// One flight of an itinerary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The marketing carrier (`LA`, `DL`, `IB`).
    pub carrier: String,
    pub number: u32,
    /// The operating carrier, when the response names one.
    pub operated_by: Option<String>,
    pub from: String,
    pub to: String,
    /// Local time at `from`.
    pub departs: Option<NaiveDateTime>,
    /// Local time at `to`.
    pub arrives: Option<NaiveDateTime>,
}

/// What a fare costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Price {
    /// The cash price with taxes, for one adult.
    Cash(Money),
    /// LATAM Pass miles, for one adult; the taxes are in BRL
    /// ([`Fare::taxes`]).
    Miles(i64),
}

impl Price {
    /// Minor units of cash, or miles: what "cheapest" orders by.
    pub fn amount(&self) -> i64 {
        match self {
            Price::Cash(m) => m.minor,
            Price::Miles(m) => *m,
        }
    }
}

/// One itinerary × fare brand (`summary.brands[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fare {
    /// `SL` LIGHT, `KM` STANDARD, `SE` PLUS, `KD` FULL, `SF` DELTA
    /// COMFORT, `RY` PREMIUM ECONOMY, `EV`/`EJ` PREMIUM BUSINESS.
    pub brand_id: String,
    pub brand: String,
    /// `Y`, `W` or `J`.
    pub cabin_code: String,
    pub cabin: Option<Cabin>,
    /// Award fare bases on round trips end in `/XP`.
    pub fare_basis: String,
    /// What a round trip's second call names as `outOfferId`; award
    /// offers end in `loyalty`.
    pub offer_id: String,
    pub price: Price,
    /// The fare before taxes, BRL (award: `priceWithOutTax`; cash:
    /// `newPrices[].fare`).
    pub fare: Option<Money>,
    /// The taxes, BRL.
    pub taxes: Option<Money>,
    pub seats: Option<u32>,
    /// The site's own "few seats left" flag (`lowAvailability`).
    pub low_seats: bool,
}

impl Fare {
    /// Miles per BRL of fare before taxes, for an award fare.
    pub fn miles_per_brl(&self) -> Option<f64> {
        let (Price::Miles(miles), Some(fare)) = (&self.price, &self.fare)
        else {
            return None;
        };
        let fare = fare.as_f64();
        (fare > 0.0).then(|| *miles as f64 / fare)
    }

    /// An award fare's rate against [`MILES_PER_BRL`], to two places:
    /// `1.0` standard, `0.95` and `0.92` the discounts seen. `None` for
    /// cash or without a fare.
    pub fn discount(&self) -> Option<f64> {
        let rate = self.miles_per_brl()? / MILES_PER_BRL;
        Some((rate * 100.0).round() / 100.0)
    }
}

/// One itinerary (`content[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Itinerary {
    pub segments: Vec<Segment>,
    pub stops: Option<u8>,
    pub duration_minutes: Option<u32>,
    pub fares: Vec<Fare>,
}

impl Itinerary {
    /// What identifies the itinerary across searches: every segment's
    /// flight and departure (`LA8084@2026-11-10T23:35/BA512@…`). The
    /// first flight alone is not enough: one flight starts several
    /// itineraries with different connections.
    pub fn key(&self) -> String {
        self.segments
            .iter()
            .map(|s| {
                let departs = s
                    .departs
                    .map(|t| t.format("%Y-%m-%dT%H:%M").to_string())
                    .unwrap_or_default();
                format!("{}{}@{departs}", s.carrier, s.number)
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// The flight numbers, `LA8084/BA512`.
    pub fn flights(&self) -> String {
        self.segments
            .iter()
            .map(|s| format!("{}{}", s.carrier, s.number))
            .collect::<Vec<_>>()
            .join("/")
    }

    pub fn departs(&self) -> Option<NaiveDateTime> {
        self.segments.first()?.departs
    }

    /// The single carrier marketing every segment, or `"multi"`.
    pub fn carrier(&self) -> Option<String> {
        let first = &self.segments.first()?.carrier;
        if self.segments.iter().all(|s| &s.carrier == first) {
            Some(first.clone())
        } else {
            Some("multi".to_owned())
        }
    }

    /// The cheapest fare of `cabin` on this itinerary, by miles or cash
    /// price.
    pub fn cheapest(&self, cabin: Cabin) -> Option<&Fare> {
        self.fares
            .iter()
            .filter(|f| f.cabin == Some(cabin))
            .min_by_key(|f| f.price.amount())
    }
}

/// An offers response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offers {
    /// `totalElements`: the page holds at most 50 (`pageSize`).
    pub total: u32,
    pub itineraries: Vec<Itinerary>,
}

impl Offers {
    /// The cheapest fare in `cabin`, by miles or by cash price, with its
    /// itinerary.
    pub fn cheapest(&self, cabin: Cabin) -> Option<(&Itinerary, &Fare)> {
        self.itineraries
            .iter()
            .flat_map(|i| i.fares.iter().map(move |f| (i, f)))
            .filter(|(_, f)| f.cabin == Some(cabin))
            .min_by_key(|(_, f)| f.price.amount())
    }
}

/// The cabin of a LATAM cabin code.
pub fn cabin(code: &str) -> Option<Cabin> {
    match code {
        "Y" => Some(Cabin::Economy),
        "W" => Some(Cabin::PremiumEconomy),
        "J" => Some(Cabin::Business),
        _ => None,
    }
}

fn json(text: &str) -> Result<Value, LatamError> {
    serde_json::from_str(text).map_err(|e| LatamError::Json(e.to_string()))
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `2026-11-10T17:40:00`.
fn date_time(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok()
}

/// A decimal BRL amount (`1838.82`) as money; `None` when not positive.
fn brl(v: Option<&Value>) -> Option<Money> {
    let amount = v?.as_f64()?;
    (amount.is_finite() && amount > 0.0)
        .then(|| Money::new((amount * 100.0).round() as i64, Currency::brl()))
}

/// `{ currency, amount }` as money, in its own currency.
fn money(v: &Value) -> Option<Money> {
    let currency = text(v, "currency")?;
    let amount = v.get("amount")?.as_f64()?;
    (amount.is_finite() && amount > 0.0).then(|| {
        Money::new((amount * 100.0).round() as i64, Currency::new(&currency))
    })
}

fn parse_segment(s: &Value) -> Option<Segment> {
    let flight = s.get("flight")?;
    Some(Segment {
        carrier: text(flight, "airlineCode")?,
        number: flight
            .get("flightNumber")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())?,
        operated_by: text(flight, "operatingAirlineCode")
            .filter(|c| !c.is_empty()),
        from: text(s, "origin").unwrap_or_default(),
        to: text(s, "destination").unwrap_or_default(),
        departs: s
            .get("departure")
            .and_then(Value::as_str)
            .and_then(date_time),
        arrives: s.get("arrival").and_then(Value::as_str).and_then(date_time),
    })
}

/// A brand; `split` is its `newPrices[]` entry (same order as the
/// brands), which gives a cash fare's split into fare and taxes.
fn parse_fare(b: &Value, split: Option<&Value>) -> Option<Fare> {
    let price = b.get("price")?;
    let amount = price.get("amount")?.as_f64()?;
    if !amount.is_finite() || amount <= 0.0 {
        return None;
    }
    let award = text(price, "currency").as_deref() == Some("LOYALTY_POINTS");
    let (price, fare, taxes) = if award {
        (
            Price::Miles(amount.round() as i64),
            b.get("priceWithOutTax").and_then(money),
            b.get("taxes").and_then(money),
        )
    } else {
        (
            Price::Cash(money(price)?),
            brl(split.and_then(|s| s.get("fare"))),
            brl(split.and_then(|s| s.get("taxes"))),
        )
    };
    let cabin_code = b
        .get("cabin")
        .and_then(|c| text(c, "id"))
        .unwrap_or_default();
    Some(Fare {
        brand_id: text(b, "id").unwrap_or_default(),
        brand: text(b, "brandText").unwrap_or_default(),
        cabin: cabin(&cabin_code),
        cabin_code,
        fare_basis: text(b, "farebasis").unwrap_or_default(),
        offer_id: text(b, "offerId").unwrap_or_default(),
        price,
        fare,
        taxes,
        seats: b
            .get("seatsRemaining")
            .and_then(|s| s.get("count"))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        low_seats: b
            .get("seatsRemaining")
            .and_then(|s| s.get("lowAvailability"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Parses an `offers/search` or `offers/search/redemption` response. An
/// empty `content` is a valid answer: no flights, or (from a call the
/// page did not make itself) a refusal that looks like one.
pub fn parse_offers(body: &str) -> Result<Offers, LatamError> {
    let root = json(body)?;
    let content = root
        .get("content")
        .and_then(Value::as_array)
        .ok_or(LatamError::Shape("no content"))?;
    let mut itineraries = Vec::with_capacity(content.len());
    for item in content {
        let Some(summary) = item.get("summary") else {
            continue;
        };
        let segments: Vec<Segment> = item
            .get("itinerary")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(parse_segment).collect())
            .unwrap_or_default();
        if segments.is_empty() {
            continue;
        }
        let splits = item.get("newPrices").and_then(Value::as_array);
        let fares = summary
            .get("brands")
            .and_then(Value::as_array)
            .map(|brands| {
                brands
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| {
                        parse_fare(b, splits.and_then(|s| s.get(i)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        itineraries.push(Itinerary {
            segments,
            stops: summary
                .get("stopOvers")
                .and_then(Value::as_u64)
                .and_then(|n| u8::try_from(n).ok()),
            duration_minutes: summary
                .get("duration")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            fares,
        });
    }
    let total = root
        .get("totalElements")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(u32::try_from(itineraries.len()).unwrap_or(u32::MAX));
    Ok(Offers { total, itineraries })
}

// ---------------------------------------------------------------------
// The calendar
// ---------------------------------------------------------------------

/// One day of the cash calendar: the cheapest economy fare LATAM cached
/// for it, 2–5% above the live price (latam.md §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarDay {
    pub date: NaiveDate,
    pub origin: String,
    pub destination: String,
    /// `travelType: ONEWAY`; a round trip's days are each leg's share.
    pub one_way: bool,
    /// `direction: OUTBOUND`.
    pub outbound: bool,
    /// `None` for a disabled day or one without a fare.
    pub price: Option<Money>,
}

/// Whether a captured URL is the calendar with prices (`extended=true`;
/// the call without it is the date picker's, always empty).
pub fn is_calendar_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.trim_end_matches('/').ends_with(CALENDAR_PATH)
        && query_value(url, "extended") == Some("true")
}

/// Parses a calendar response: `days[].calendar[]` (the date picker's
/// call without `extended` answers the bare list), each a month of one
/// direction with its `detailsCalendar[]`.
pub fn parse_calendar(body: &str) -> Result<Vec<CalendarDay>, LatamError> {
    let root = json(body)?;
    let days = root
        .get("days")
        .and_then(Value::as_array)
        .or_else(|| root.as_array())
        .ok_or(LatamError::Shape("no days"))?;
    let mut out = Vec::new();
    for month in days
        .iter()
        .filter_map(|d| d.get("calendar").and_then(Value::as_array))
        .flatten()
    {
        let origin = text(month, "origin").unwrap_or_default();
        let destination = text(month, "destination").unwrap_or_default();
        let one_way = text(month, "travelType").as_deref() == Some("ONEWAY");
        let outbound = text(month, "direction").as_deref() != Some("INBOUND");
        for d in month
            .get("detailsCalendar")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(date) = d
                .get("date")
                .and_then(Value::as_str)
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            else {
                continue;
            };
            let enabled =
                d.get("enabled").and_then(Value::as_bool) != Some(false);
            out.push(CalendarDay {
                date,
                origin: origin.clone(),
                destination: destination.clone(),
                one_way,
                outbound,
                price: d.get("fare").and_then(money).filter(|_| enabled),
            });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// Award against cash
// ---------------------------------------------------------------------

/// One brand of one itinerary found in both an award and a cash search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FarePair {
    /// [`Itinerary::key`].
    pub itinerary: String,
    pub brand_id: String,
    pub miles: i64,
    /// The award fare before taxes, BRL.
    pub award_fare: Money,
    /// The cash fare before taxes, BRL.
    pub cash_fare: Money,
    pub award_taxes: Option<Money>,
    pub cash_taxes: Option<Money>,
    pub award_basis: String,
    pub cash_basis: String,
}

impl FarePair {
    /// The award prices the cash fare's bucket: the same fare before
    /// taxes. When it does not, the award search offered another bucket
    /// (award inventory).
    pub fn same_fare(&self) -> bool {
        self.award_fare.minor == self.cash_fare.minor
    }

    /// Miles per BRL of *cash* fare: [`MILES_PER_BRL`] when the award
    /// follows cash, lower for a discount or a cheaper award bucket,
    /// higher for a closed cash bucket.
    pub fn miles_per_cash_brl(&self) -> f64 {
        self.miles as f64 / self.cash_fare.as_f64()
    }
}

/// The brands that `award` and `cash` (responses for the same leg and
/// date) both offer, matched on the whole itinerary and the brand.
pub fn pair_fares(award: &Offers, cash: &Offers) -> Vec<FarePair> {
    let mut out = Vec::new();
    for a in &award.itineraries {
        let key = a.key();
        let Some(c) = cash.itineraries.iter().find(|c| c.key() == key) else {
            continue;
        };
        for af in &a.fares {
            let Price::Miles(miles) = af.price else {
                continue;
            };
            let Some(cf) = c.fares.iter().find(|f| f.brand_id == af.brand_id)
            else {
                continue;
            };
            let (Some(award_fare), Some(cash_fare)) =
                (af.fare.clone(), cf.fare.clone())
            else {
                continue;
            };
            out.push(FarePair {
                itinerary: key.clone(),
                brand_id: af.brand_id.clone(),
                miles,
                award_fare,
                cash_fare,
                award_taxes: af.taxes.clone(),
                cash_taxes: cf.taxes.clone(),
                award_basis: af.fare_basis.clone(),
                cash_basis: cf.fare_basis.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Datelike as _;

    use super::*;

    /// A response from `docs/research/sites/fixtures/` (`latam/…` or
    /// `latam-pass/…`), read at run time: the fixtures stay with the
    /// research and out of the Nix build's sources.
    fn fixture(name: &str) -> Offers {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/research/sites/fixtures")
            .join(name);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        parse_offers(&body)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a date")
    }

    fn brl_minor(minor: i64) -> Money {
        Money::new(minor, Currency::brl())
    }

    fn fare<'a>(offers: &'a Offers, flights: &str, brand: &str) -> &'a Fare {
        offers
            .itineraries
            .iter()
            .find(|i| i.flights() == flights)
            .and_then(|i| i.fares.iter().find(|f| f.brand_id == brand))
            .unwrap_or_else(|| panic!("{flights} {brand}"))
    }

    #[test]
    fn results_urls_follow_the_page() {
        let search = Search {
            origin: "gru".into(),
            destination: "LIS".into(),
            outbound: day(2026, 11, 10),
            inbound: None,
            passengers: Passengers::default(),
            cabin: Cabin::Economy,
            redemption: true,
        };
        assert_eq!(
            search.url(BASE).as_deref(),
            Some(
                "https://www.latamairlines.com/br/pt/oferta-voos?origin=GRU\
                 &destination=LIS&outbound=2026-11-10T12%3A00%3A00.000Z\
                 &inbound=null&adt=1&chd=0&inf=0&trip=OW&cabin=Economy\
                 &redemption=true&sort=RECOMMENDED"
            )
        );
        let round = Search {
            inbound: Some(day(2027, 3, 11)),
            cabin: Cabin::Business,
            redemption: false,
            passengers: Passengers {
                adults: 2,
                children: 1,
                infants_in_seat: 0,
                infants_on_lap: 1,
            },
            ..search.clone()
        };
        let url = round.url("http://127.0.0.1:9/").unwrap_or_default();
        assert!(
            url.starts_with("http://127.0.0.1:9/br/pt/oferta-voos?"),
            "{url}"
        );
        assert!(
            url.contains("&inbound=2027-03-11T12%3A00%3A00.000Z"),
            "{url}"
        );
        assert!(
            url.contains(
                "&adt=2&chd=1&inf=1&trip=RT&cabin=Business&redemption=false"
            ),
            "{url}"
        );
        let seated = Search {
            passengers: Passengers {
                infants_in_seat: 1,
                ..Passengers::default()
            },
            ..search
        };
        assert_eq!(seated.url(BASE), None);
    }

    #[test]
    fn captured_urls_name_their_call() {
        let base = "https://www.latamairlines.com/bff/air-offers/v2";
        assert_eq!(
            OffersCall::of(&format!(
                "{base}/offers/search?origin=GRU&outOfferId=null"
            )),
            Some(OffersCall::Cash)
        );
        assert_eq!(
            OffersCall::of(&format!(
                "{base}/offers/search/redemption?redemption=true"
            )),
            Some(OffersCall::Award)
        );
        for other in [
            "/offers/search/redemption/promo/ABC?x=1",
            "/offers/brand-bundle-details",
            "/calendar?origin=GRU&extended=true",
        ] {
            assert_eq!(
                OffersCall::of(&format!("{base}{other}")),
                None,
                "{other}"
            );
        }
        assert!(!is_return_leg(&format!(
            "{base}/offers/search/redemption?outOfferId=null&inFrom=2027-03-11"
        )));
        assert!(is_return_leg(&format!(
            "{base}/offers/search/redemption?a=1&outOfferId=62f0aab1loyalty&b=2"
        )));
        assert!(is_login_wall(
            "https://auth.latamairlines.com/u/login/identifier?state=x&ui_locales=pt"
        ));
        assert!(!is_login_wall(
            "https://www.latamairlines.com/br/pt/oferta-voos?redemption=true"
        ));
    }

    #[test]
    fn parses_a_cash_response() {
        let cash = fixture(
            "latam/offers-search-ow-gru-lis-2026-11-10-br-headed-warm.json",
        );
        assert_eq!(cash.total, 29);
        assert_eq!(cash.itineraries.len(), 29);
        let (itinerary, light) =
            cash.cheapest(Cabin::Economy).expect("economy");
        assert_eq!(itinerary.flights(), "LA8146");
        assert_eq!(itinerary.stops, Some(0));
        assert_eq!(light.brand, "LIGHT");
        assert_eq!(light.price, Price::Cash(brl_minor(190_743)));
        assert_eq!(light.fare, Some(brl_minor(183_882)));
        assert_eq!(light.taxes, Some(brl_minor(6_861)));
        assert_eq!(light.miles_per_brl(), None);
        let segment = &itinerary.segments[0];
        assert_eq!(
            (segment.from.as_str(), segment.to.as_str()),
            ("GRU", "LIS")
        );
        assert_eq!(segment.departs, day(2026, 11, 10).and_hms_opt(17, 40, 0));
        // Every cabin comes in an economy search.
        assert!(cash.cheapest(Cabin::Business).is_some());
    }

    #[test]
    fn parses_an_award_response() {
        let award = fixture(
            "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json",
        );
        assert_eq!(award.itineraries.len(), 17);
        let (itinerary, light) =
            award.cheapest(Cabin::Economy).expect("economy");
        assert_eq!(itinerary.flights(), "LA8146");
        assert_eq!(light.price, Price::Miles(63_848));
        assert_eq!(light.fare, Some(brl_minor(183_882)));
        assert_eq!(light.taxes, Some(brl_minor(6_861)));
        assert!(light.offer_id.ends_with("loyalty"));
        assert_eq!(light.discount(), Some(1.0));
        let (_, business) = award.cheapest(Cabin::Business).expect("business");
        assert_eq!(business.price, Price::Miles(263_848));
        // Partner connections are priced at the same rate.
        let iberia = fare(&award, "LA8066/IB533", "SL");
        assert_eq!(iberia.discount(), Some(1.0));
    }

    #[test]
    fn miles_are_the_fare_times_the_rate() {
        for name in [
            "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json",
            "latam-pass/offers-search-redemption-ow-gru-lis-2027-06-15-br.json",
            "latam-pass/offers-search-redemption-ow-gru-mia-2027-03-04-br.json",
            "latam-pass/offers-search-redemption-ow-mia-gru-2027-03-11-br.json",
            "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-11-inbound-br.json",
        ] {
            for f in fixture(name).itineraries.iter().flat_map(|i| &i.fares) {
                let (Price::Miles(miles), Some(fare)) = (&f.price, &f.fare)
                else {
                    panic!("{name}: {f:?}");
                };
                let expected = (fare.as_f64() * MILES_PER_BRL).round() as i64;
                assert!((miles - expected).abs() <= 1, "{name}: {f:?}");
            }
        }
    }

    #[test]
    fn some_routes_have_silent_discounts() {
        // CGH→SDU: every economy brand at 0.95, premium economy at 1.
        let domestic = fixture(
            "latam-pass/offers-search-redemption-ow-cgh-sdu-2026-10-21-br.json",
        );
        for f in domestic.itineraries.iter().flat_map(|i| &i.fares) {
            let expected = if f.cabin == Some(Cabin::Economy) {
                0.95
            } else {
                1.0
            };
            assert_eq!(f.discount(), Some(expected), "{f:?}");
        }
        // GRU–MIA round trip: the LATAM nonstops' economy at 0.92,
        // their business and the connections at 1.
        let rt = fixture(
            "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-04-outbound-br.json",
        );
        let light = fare(&rt, "LA8198", "SL");
        assert_eq!(light.price, Price::Miles(69_069));
        assert_eq!(light.discount(), Some(0.92));
        assert_eq!(fare(&rt, "LA8198", "EV").discount(), Some(1.0));
        assert_eq!(fare(&rt, "LA8212/LA4400", "SL").discount(), Some(1.0));
        assert!(light.fare_basis.ends_with("/XP"));
    }

    #[test]
    fn award_fares_are_the_cash_fares_of_the_same_bucket() {
        // GRU→LIS, both read in the same session: every pair the same.
        let pairs = pair_fares(
            &fixture(
                "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json",
            ),
            &fixture(
                "latam/offers-search-ow-gru-lis-2026-11-10-br-headed-warm.json",
            ),
        );
        assert_eq!(pairs.len(), 80);
        assert!(pairs.iter().all(FarePair::same_fare));
        // The itinerary the first reading got wrong (latam-pass.md).
        let la8084 = pairs
            .iter()
            .find(|p| {
                p.itinerary.starts_with("LA8084@")
                    && p.itinerary.contains("/BA512@")
                    && p.brand_id == "SL"
            })
            .expect("LA8084/BA512 LIGHT");
        assert_eq!(la8084.miles, 145_298);
        assert_eq!(la8084.cash_fare, brl_minor(418_457));

        // CGH→SDU: LA3902's cheapest cash bucket is not open to awards.
        let pairs = pair_fares(
            &fixture(
                "latam-pass/offers-search-redemption-ow-cgh-sdu-2026-10-21-br.json",
            ),
            &fixture("latam/offers-search-ow-cgh-sdu-2026-10-21-br.json"),
        );
        let la3902 = pairs
            .iter()
            .find(|p| p.itinerary.starts_with("LA3902@") && p.brand_id == "SL")
            .expect("LA3902 LIGHT");
        assert!(!la3902.same_fare());
        assert_eq!(la3902.cash_fare, brl_minor(90_490));
        assert_eq!(la3902.award_fare, brl_minor(123_090));
        assert_eq!(
            (la3902.cash_basis.as_str(), la3902.award_basis.as_str()),
            ("NLEX0H1", "XJEX0N1")
        );
        assert!(la3902.miles_per_cash_brl() > MILES_PER_BRL);

        // GRU–MIA round trip outbound: the same fares, different taxes,
        // and a Delta connection on a cheaper award bucket.
        let pairs = pair_fares(
            &fixture(
                "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-04-outbound-br.json",
            ),
            &fixture(
                "latam/offers-search-rt-gru-mia-2027-03-04-outbound-br-2026-10-07.json",
            ),
        );
        let nonstop = pairs
            .iter()
            .find(|p| p.itinerary.starts_with("LA8198@") && p.brand_id == "SL")
            .expect("LA8198 LIGHT");
        assert!(nonstop.same_fare());
        assert_eq!(nonstop.award_taxes, Some(brl_minor(16_098)));
        assert_eq!(nonstop.cash_taxes, Some(brl_minor(27_727)));
        let delta = pairs
            .iter()
            .find(|p| p.itinerary.starts_with("LA6346@") && p.brand_id == "KM")
            .expect("LA6346/LA6183 STANDARD");
        assert!(!delta.same_fare());
        assert!(delta.miles_per_cash_brl() < MILES_PER_BRL);
    }

    #[test]
    fn round_trips_cost_less_than_two_one_ways() {
        let out = fixture(
            "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-04-outbound-br.json",
        );
        let back = fixture(
            "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-11-inbound-br.json",
        );
        let ow_out = fixture(
            "latam-pass/offers-search-redemption-ow-gru-mia-2027-03-04-br.json",
        );
        let ow_back = fixture(
            "latam-pass/offers-search-redemption-ow-mia-gru-2027-03-11-br.json",
        );
        let miles =
            |o: &Offers, flights: &str| match fare(o, flights, "SL").price {
                Price::Miles(m) => m,
                Price::Cash(_) => panic!("cash"),
            };
        let round = miles(&out, "LA8198") + miles(&back, "LA8199");
        let one_ways = miles(&ow_out, "LA8198") + miles(&ow_back, "LA8199");
        assert_eq!(round, 144_134);
        assert_eq!(one_ways, 224_676);
    }

    fn raw(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/research/sites/fixtures")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn the_calendar_is_two_months_of_cached_days() {
        let days = parse_calendar(&raw(
            "latam/calendar-ow-gru-lis-2026-11-extended.json",
        ))
        .expect("parses");
        assert!(days.iter().all(|d| d.one_way && d.outbound));
        assert!(
            days.iter()
                .all(|d| d.origin == "GRU" && d.destination == "LIS")
        );
        let nov10 = days
            .iter()
            .find(|d| d.date == day(2026, 11, 10))
            .expect("Nov 10");
        // The cached figure, 4.6% above the live R$1,913.39.
        assert_eq!(nov10.price, Some(brl_minor(200_141)));
        assert!(days.iter().any(|d| d.date.month() == 12));
        let rt = parse_calendar(&raw(
            "latam/calendar-rt-gru-mia-2027-03-extended.json",
        ))
        .expect("parses");
        assert!(rt.iter().all(|d| !d.one_way));
        assert!(rt.iter().any(|d| !d.outbound));
        assert!(
            parse_calendar(&raw(
                "latam/calendar-ow-gru-lis-2026-11-plain.json"
            ))
            .is_ok_and(|d| d.iter().all(|d| d.price.is_none()))
        );
        let base = "https://www.latamairlines.com/bff/air-offers/v2/calendar";
        assert!(is_calendar_url(&format!(
            "{base}?origin=GRU&month=11&isRoundTrip=false&extended=true"
        )));
        assert!(!is_calendar_url(&format!("{base}?origin=GRU&month=11")));
    }

    #[test]
    fn offers_calls_name_their_dates() {
        let url = "https://www.latamairlines.com/bff/air-offers/v2/offers/search/redemption\
                   ?sort=RECOMMENDED&outOfferId=null&inFrom=null&origin=GRU\
                   &destination=LIS&outFrom=2027-06-16&redemption=true";
        assert_eq!(out_from(url), Some(day(2027, 6, 16)));
        assert_eq!(in_from(url), None);
        assert_eq!(
            in_from("x/offers/search?outFrom=2027-03-04&inFrom=2027-03-11"),
            Some(day(2027, 3, 11))
        );
    }

    #[test]
    fn akamai_denials_and_logins_are_told_apart() {
        let html = raw("latam/headless-access-denied.html");
        assert!(detect_block("Access Denied", "").is_some());
        assert!(detect_block("", &html).is_some());
        assert_eq!(
            detect_block(
                "Seleção de voos | LATAM Airlines",
                "GRU LIS R$ 1.913"
            ),
            None
        );
        let search =
            "https://www.latamairlines.com/bff/air-offers/v2/offers/search?x=1";
        assert!(blocked_response(search, Some(403)).is_some());
        assert!(blocked_response(search, Some(429)).is_some());
        assert_eq!(blocked_response(search, Some(200)), None);
        assert!(is_login_wall(
            "https://auth.latamairlines.com/authorize/resume"
        ));
        assert!(is_login_wall(
            "http://127.0.0.1:9/u/login/identifier?state=x"
        ));
    }

    #[test]
    fn one_adult_legs_and_round_trips_run() {
        let one = Passengers::default();
        let leg = FamilyKind::Leg {
            leg: tm_domain::Leg::Return,
        };
        assert_eq!(unsupported(leg, &one), None);
        assert_eq!(unsupported(FamilyKind::RoundTrip { days: 7 }, &one), None);
        assert!(unsupported(FamilyKind::Explore, &one).is_some());
        let two = Passengers { adults: 2, ..one };
        assert!(unsupported(leg, &two).is_some());
        assert_eq!(airport("SAO"), "GRU");
        assert_eq!(airport("LIS"), "LIS");
    }

    #[test]
    fn itineraries_name_their_carrier_and_seats() {
        let award = fixture(
            "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json",
        );
        let nonstop = award
            .itineraries
            .iter()
            .find(|i| i.flights() == "LA8146")
            .expect("LA8146");
        assert_eq!(nonstop.carrier().as_deref(), Some("LA"));
        let iberia = award
            .itineraries
            .iter()
            .find(|i| i.flights() == "LA8066/IB533")
            .expect("LA8066/IB533");
        assert_eq!(iberia.carrier().as_deref(), Some("multi"));
        let light = nonstop.cheapest(Cabin::Economy).expect("economy");
        assert_eq!(light.brand_id, "SL");
        assert_eq!((light.seats, light.low_seats), (Some(9), false));
    }

    #[test]
    fn an_empty_answer_parses_as_no_flights() {
        // What a direct `fetch` of the award call got (latam-pass.md).
        let empty = r#"{"content":[],"empty":true,"first":true,"last":true,"number":0,
            "numberOfElements":0,"pageable":"INSTANCE","size":0,"totalElements":0,"totalPages":1}"#;
        let offers = parse_offers(empty).expect("parses");
        assert_eq!(offers.total, 0);
        assert!(offers.itineraries.is_empty());
        assert!(matches!(parse_offers("<html>"), Err(LatamError::Json(_))));
        assert_eq!(parse_offers("{}"), Err(LatamError::Shape("no content")));
    }
}
