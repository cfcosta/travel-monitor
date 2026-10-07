//! LATAM Airlines (`airline:LA`) and LATAM Pass (`miles:latam_pass`) on
//! `www.latamairlines.com` (DECISIONS.md 0081, 0082; research:
//! `docs/research/sites/latam.md` and `latam-pass.md`). Pure: the
//! results URL and the parser for the offers responses the results page
//! makes. No reader runs yet; both sites need the offscreen display
//! (0081), and LATAM Pass a login.
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
//!
//! Miles are the award fare before taxes × [`MILES_PER_BRL`], rounded,
//! except for silent route discounts (× 0.95, × 0.92 seen); the award
//! fare is the cash fare of the same fare basis, when the award search
//! offers that bucket ([`pair_fares`], latam-pass.md "Measuring cash
//! against miles").

use std::fmt::Write as _;

use chrono::{NaiveDate, NaiveDateTime};
use serde_json::Value;
use tm_domain::{Cabin, Currency, Money, Passengers};

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

/// Whether the page ended on LATAM's login (an award URL while logged
/// out redirects to `auth.latamairlines.com/u/login/…`, latam-pass.md
/// §1).
pub fn is_login_wall(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    host.eq_ignore_ascii_case("auth.latamairlines.com")
        && path.starts_with("u/login")
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
            .min_by_key(|(_, f)| match &f.price {
                Price::Cash(m) => m.minor,
                Price::Miles(m) => *m,
            })
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
