//! LATAM in the daemon's Chromium (DECISIONS.md 0081, 0082, 0084, 0085,
//! 0090): load the results page and read the offers call the page
//! makes, one page load per date; with the strip switched on, click
//! days of the page's date strip for a one-way award search; for a round
//! trip, choose the cheapest outbound brand to make the page price the
//! return leg. [`crate::sites::latam`] parses; this module drives the
//! page and turns offers into cells.
//!
//! The page runs in the profile's own browser context, not a point of
//! sale's isolated one: the login and the warm-up the user did by hand
//! live in its cookies (0084). The searches are the page's own: the
//! daemon never calls LATAM's API (latam-pass.md §5, 0052). The strip is
//! off by default (`Config::latam_strip`, 0090): in the live run a
//! search a strip click made got Akamai's 403.
//!
//! - **Cash** (`airline:LA`): one cell per date, the cabin's cheapest
//!   brand (LIGHT in economy), with its carrier, stops and itinerary.
//! - **Awards** (`miles:latam_pass`): one cell per itinerary × brand of
//!   the cabin: miles, taxes in BRL as the price, the itinerary's key,
//!   and the brand's fare before taxes, fare basis and seats (0083).
//! - **Round trips**: the outbound call prices each outbound as its half
//!   of the trip; choosing the cheapest outbound brand of the cabin
//!   makes the page call again for the return leg (latam.md §2). The
//!   cells are that outbound with each return, one per return
//!   itinerary × brand (one cell for cash, the cheapest). The choice is
//!   a click, so round trips are not scheduled until a live run shows
//!   it is not denied ([`crate::sites::latam::unsupported`], 0090).
//! - **Blocks and logins**: Akamai's "Access Denied", or a 403 on a BFF
//!   call, is a block (0039, 0060), never retried; a page on LATAM's
//!   login is a session that ended. A block or failure after the page
//!   was read keeps what was read ([`Fetch::partial`]): the page's
//!   cells and the strip days read before it, or for a round trip only
//!   the page's calendar, since its outbound halves price nothing alone.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use rand::Rng;
use serde_json::{Value, json};
use tm_browser::{Captured, Page};
use tm_domain::{AwardDetail, Cabin, Currency, Money, QueryOutcome, Seats};
use tracing::{debug, info, warn};

use super::{
    Cell,
    Fetch,
    LatamFound,
    LatamJob,
    browser::{Timeouts, navigate},
};
use crate::sites::latam::{
    self,
    Fare,
    Itinerary,
    Offers,
    OffersCall,
    Price,
    dates::strip_days,
    page,
};

// ---------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------

fn seats(fare: &Fare) -> Option<Seats> {
    let count = u8::try_from(fare.seats?).unwrap_or(u8::MAX);
    Some(Seats {
        count,
        low: fare.low_seats,
    })
}

fn award_cell(
    itinerary: &Itinerary,
    fare: &Fare,
    departure: NaiveDate,
    seller: &str,
) -> Option<Cell> {
    let Price::Miles(miles) = fare.price else {
        return None;
    };
    let mut cell = Cell::priced(departure, None, fare.taxes.clone());
    cell.miles = Some(miles);
    cell.carrier = itinerary.carrier();
    cell.stops = itinerary.stops;
    cell.duration_minutes = itinerary.duration_minutes;
    cell.seller = Some(seller.to_owned());
    cell.flights = Some(itinerary.key());
    cell.award = Some(AwardDetail {
        fare: fare.fare.clone(),
        brand: Some(fare.brand.clone()).filter(|b| !b.is_empty()),
        fare_basis: Some(fare.fare_basis.clone()).filter(|b| !b.is_empty()),
        seats: seats(fare),
    });
    Some(cell)
}

/// The cells of one leg's offers for `departure` in `cabin`: every
/// itinerary × brand for an award, the cheapest brand for cash. A date
/// whose cabin has no fare gets one cell without a price, so it counts
/// as read.
pub fn leg_cells(
    offers: &Offers,
    cabin: Cabin,
    departure: NaiveDate,
    seller: &str,
) -> Vec<Cell> {
    let mut cells: Vec<Cell> = Vec::new();
    for itinerary in &offers.itineraries {
        for fare in itinerary.fares.iter().filter(|f| f.cabin == Some(cabin)) {
            if let Some(cell) = award_cell(itinerary, fare, departure, seller) {
                cells.push(cell);
            }
        }
    }
    if cells.is_empty()
        && let Some((itinerary, fare)) = offers.cheapest(cabin)
        && let Price::Cash(price) = &fare.price
    {
        let mut cell = Cell::priced(departure, None, Some(price.clone()));
        cell.carrier = itinerary.carrier();
        cell.stops = itinerary.stops;
        cell.duration_minutes = itinerary.duration_minutes;
        cell.seller = Some(seller.to_owned());
        cell.flights = Some(itinerary.key());
        cells.push(cell);
    }
    if cells.is_empty() {
        let mut empty = Cell::priced(departure, None, None);
        empty.seller = Some(seller.to_owned());
        cells.push(empty);
    }
    cells
}

/// Money plus money of the same currency.
fn plus(a: Option<&Money>, b: Option<&Money>) -> Option<Money> {
    let (a, b) = (a?, b?);
    (a.currency == b.currency)
        .then(|| Money::new(a.minor + b.minor, a.currency.clone()))
}

/// The round trip's cells: the chosen `outbound` fare with each return
/// itinerary × brand of the cabin in `back` (one cell, the cheapest,
/// for cash). Miles, taxes and fares add up over the two legs; the
/// itinerary key runs over every segment of both.
pub fn round_trip_cells(
    outbound: (&Itinerary, &Fare),
    back: &Offers,
    cabin: Cabin,
    (departure, return_date): (NaiveDate, NaiveDate),
    seller: &str,
) -> Vec<Cell> {
    let (out_itinerary, out_fare) = outbound;
    let key =
        |ret: &Itinerary| format!("{}/{}", out_itinerary.key(), ret.key());
    let stops = |ret: &Itinerary| Some(out_itinerary.stops?.max(ret.stops?));
    let carrier = |ret: &Itinerary| {
        let (a, b) = (out_itinerary.carrier()?, ret.carrier()?);
        Some(if a == b { a } else { "multi".to_owned() })
    };
    let mut cells = Vec::new();
    match out_fare.price {
        Price::Miles(out_miles) => {
            for ret in &back.itineraries {
                for fare in ret.fares.iter().filter(|f| f.cabin == Some(cabin))
                {
                    let Price::Miles(miles) = fare.price else {
                        continue;
                    };
                    let mut cell = Cell::priced(
                        departure,
                        Some(return_date),
                        plus(out_fare.taxes.as_ref(), fare.taxes.as_ref()),
                    );
                    cell.miles = Some(out_miles + miles);
                    cell.carrier = carrier(ret);
                    cell.stops = stops(ret);
                    cell.seller = Some(seller.to_owned());
                    cell.flights = Some(key(ret));
                    let brand = if out_fare.brand == fare.brand {
                        out_fare.brand.clone()
                    } else {
                        format!("{} + {}", out_fare.brand, fare.brand)
                    };
                    cell.award = Some(AwardDetail {
                        fare: plus(out_fare.fare.as_ref(), fare.fare.as_ref()),
                        brand: Some(brand),
                        fare_basis: Some(format!(
                            "{} + {}",
                            out_fare.fare_basis, fare.fare_basis
                        )),
                        seats: match (seats(out_fare), seats(fare)) {
                            (Some(a), Some(b)) => Some(Seats {
                                count: a.count.min(b.count),
                                low: a.low || b.low,
                            }),
                            (a, b) => a.or(b),
                        },
                    });
                    cells.push(cell);
                }
            }
        }
        Price::Cash(ref out_price) => {
            if let Some((ret, fare)) = back.cheapest(cabin)
                && let Price::Cash(price) = &fare.price
            {
                let mut cell = Cell::priced(
                    departure,
                    Some(return_date),
                    plus(Some(out_price), Some(price)),
                );
                cell.carrier = carrier(ret);
                cell.stops = stops(ret);
                cell.seller = Some(seller.to_owned());
                cell.flights = Some(key(ret));
                cells.push(cell);
            }
        }
    }
    if cells.is_empty() {
        let mut empty = Cell::priced(departure, Some(return_date), None);
        empty.seller = Some(seller.to_owned());
        cells.push(empty);
    }
    cells
}

/// The outbound fare a round trip is priced with: the cabin's cheapest,
/// with its card's position on the page and its brand's in the card.
pub fn chosen_outbound(
    offers: &Offers,
    cabin: Cabin,
) -> Option<(usize, usize, &Itinerary, &Fare)> {
    offers
        .itineraries
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let fare = it.cheapest(cabin)?;
            let b = it.fares.iter().position(|f| std::ptr::eq(f, fare))?;
            Some((i, b, it, fare))
        })
        .min_by_key(|(i, _, _, f)| (f.price.amount(), *i))
}

/// The calendar's one-way days of `origin` → `destination`, minor
/// units of their cached fares.
pub fn calendar_prices(
    days: &[latam::CalendarDay],
    origin: &str,
    destination: &str,
) -> BTreeMap<NaiveDate, i64> {
    days.iter()
        .filter(|d| d.one_way && d.outbound)
        .filter(|d| {
            (d.origin.is_empty() || d.origin == origin)
                && (d.destination.is_empty() || d.destination == destination)
        })
        .filter_map(|d| {
            let p = d.price.as_ref()?;
            (p.currency == Currency::brl()).then_some((d.date, p.minor))
        })
        .collect()
}

// ---------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------

/// A login wall or a block in place of the results.
async fn page_state(page: &Page) -> Option<QueryOutcome> {
    let url = page.url().await.unwrap_or_default();
    if latam::is_login_wall(&url) {
        info!(%url, "LATAM shows its login: the session ended");
        return Some(QueryOutcome::LoggedOut);
    }
    let (title, text): (String, String) = page
        .eval_as(page::TITLE_AND_TEXT_JS)
        .await
        .unwrap_or_default();
    latam::detect_block(&title, &text)
        .map(|detail| QueryOutcome::Blocked { detail })
}

/// Why waiting for an offers call ended without one.
enum Missed {
    Blocked(String),
    Timeout,
}

/// Waits for an offers call of `call` that `wanted` accepts, keeping
/// every captured response in `seen`. Offers calls for other searches
/// are skipped: a search is recorded only for the date it asked for.
async fn wait_offers(
    capture: &tm_browser::Capture,
    seen: &mut Vec<Captured>,
    call: OffersCall,
    wanted: impl Fn(&str) -> bool,
    timeout: std::time::Duration,
) -> Result<Captured, Missed> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        for r in capture.drain() {
            debug!(url = %r.url, status = ?r.status, bytes = r.body.len(), "LATAM call");
            seen.push(r.clone());
            if let Some(detail) = latam::blocked_response(&r.url, r.status) {
                return Err(Missed::Blocked(detail));
            }
            if OffersCall::of(&r.url) != Some(call) {
                continue;
            }
            if wanted(&r.url) {
                return Ok(r);
            }
            warn!(url = %r.url, "an offers call for another search: not recorded");
        }
        let left =
            deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() || capture.wait_for(1, left).await.is_err() {
            return Err(Missed::Timeout);
        }
    }
}

/// A random pause between `t.strip_pause`'s bounds: a strip click (or
/// the round trip's outbound choice) is spaced like a page load (0084).
async fn strip_pause(t: Timeouts) {
    let (min, max) = t.strip_pause;
    let (min, max) = (min.as_millis() as u64, max.as_millis() as u64);
    let ms = rand::rng().random_range(min..=max.max(min));
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// Clicks where `script` (a [`page::locate_js`] or
/// [`page::strip_day_js`]) says, with the mouse. False when it found
/// nothing to click.
async fn click_where(page: &Page, script: &str) -> bool {
    let point = page.eval(script).await.unwrap_or(Value::Null);
    let (Some(x), Some(y)) = (point["x"].as_f64(), point["y"].as_f64()) else {
        return false;
    };
    page.click_at(x, y).await.is_ok()
}

/// Waits for `selector` to show up, then clicks it.
async fn click_when_shown(
    page: &Page,
    selector: &str,
    timeout: std::time::Duration,
) -> bool {
    let script = page::locate_js(selector);
    if page
        .wait_until(&script, timeout, std::time::Duration::from_millis(300))
        .await
        .is_err()
    {
        return false;
    }
    click_where(page, &script).await
}

/// The session cookie's end, from the profile's cookies for the site
/// (`Network.getCookies` sees httpOnly ones). `Some(None)` when it is
/// missing, expired or lasts only for the browser's run.
async fn session_until(
    page: &Page,
    base: &str,
) -> Option<Option<DateTime<Utc>>> {
    let result = page
        .call("Network.getCookies", json!({"urls": [base]}))
        .await
        .ok()?;
    let now = Utc::now();
    let until = result["cookies"]
        .as_array()?
        .iter()
        .filter(|c| c["name"] == latam::SESSION_COOKIE)
        .filter(|c| c["session"] != true)
        .filter_map(|c| c["expires"].as_f64())
        .filter_map(|s| DateTime::from_timestamp(s as i64, 0))
        .filter(|t| *t > now)
        .max();
    Some(until)
}

fn parse(r: &Captured) -> Result<Offers, QueryOutcome> {
    if !r.is_ok() {
        return Err(QueryOutcome::Unexpected {
            detail: format!(
                "the offers call failed: status {:?} {}",
                r.status,
                r.error.as_deref().unwrap_or("")
            ),
        });
    }
    latam::parse_offers(&r.text()).map_err(|e| QueryOutcome::Unexpected {
        detail: format!("unreadable offers response: {e}"),
    })
}

/// The searched date: the call's own `outFrom`, else what was asked.
fn searched(url: &str, asked: NaiveDate) -> bool {
    latam::out_from(url).is_none_or(|d| d == asked)
}

/// One LATAM search on `page`. See the module documentation.
pub(crate) async fn latam_on(
    page: &Page,
    job: &LatamJob,
    t: Timeouts,
) -> Fetch<LatamFound> {
    let capture = match page.capture(latam::CAPTURE).await {
        Ok(c) => c,
        Err(e) => return Fetch::error(format!("capture failed: {e}")),
    };
    let call = if job.award {
        OffersCall::Award
    } else {
        OffersCall::Cash
    };
    let mut loads = 1;
    if let Err(outcome) = navigate(page, &job.url, t.navigation).await {
        return Fetch::failed(outcome, loads);
    }
    if let Some(outcome) = page_state(page).await {
        return Fetch::failed(outcome, loads);
    }
    let mut seen: Vec<Captured> = Vec::new();
    let first = wait_offers(
        &capture,
        &mut seen,
        call,
        |url| !latam::is_return_leg(url) && searched(url, job.departure),
        t.response * 2,
    )
    .await;
    let first = match first {
        Ok(r) => r,
        Err(Missed::Blocked(detail)) => {
            return Fetch::failed(QueryOutcome::Blocked { detail }, loads);
        }
        Err(Missed::Timeout) => {
            let outcome = page_state(page).await.unwrap_or_else(|| {
                QueryOutcome::Unexpected {
                    detail: "no LATAM offers response".into(),
                }
            });
            return Fetch::failed(outcome, loads);
        }
    };
    let offers = match parse(&first) {
        Ok(o) => o,
        Err(outcome) => return Fetch::failed(outcome, loads),
    };
    info!(
        date = %job.departure,
        award = job.award,
        itineraries = offers.itineraries.len(),
        total = offers.total,
        "LATAM offers read"
    );
    if offers.itineraries.is_empty() {
        return Fetch::failed(QueryOutcome::NoFlights, loads);
    }
    let Some(return_date) = job.return_date else {
        let cells = leg_cells(&offers, job.cabin, job.departure, &job.seller);
        return read_strip(page, job, &capture, seen, cells, t).await;
    };
    let Some((card, brand, itinerary, fare)) =
        chosen_outbound(&offers, job.cabin)
    else {
        info!(cabin = ?job.cabin, "LATAM sells no round trip in this cabin");
        let mut empty = Cell::priced(job.departure, Some(return_date), None);
        empty.seller = Some(job.seller.clone());
        return finish(page, job, &capture, seen, vec![empty], loads, 0, t)
            .await;
    };
    // Choosing the outbound is a search of its own: spaced and counted
    // like a page load. From here a failure keeps what the page read
    // (0090); the outbound halves price nothing alone, so no cells.
    strip_pause(t).await;
    loads += 1;
    let chosen = click_when_shown(page, &page::card_header(card), t.open_graph)
        .await
        && click_when_shown(page, &page::brand_select(brand), t.open_graph)
            .await;
    if !chosen {
        let outcome = QueryOutcome::Unexpected {
            detail: format!(
                "the outbound fare (card {card}, brand {brand}) could not be chosen"
            ),
        };
        return failed_after(
            job,
            &capture,
            seen,
            Vec::new(),
            outcome,
            loads,
            0,
        );
    }
    // The LATAM+ upsell, when it shows.
    let _ = click_when_shown(page, page::UPSELL_CONTINUE, t.settle * 2).await;
    let offer = fare.offer_id.clone();
    let back = wait_offers(
        &capture,
        &mut seen,
        call,
        |url| {
            latam::is_return_leg(url)
                && (offer.is_empty() || url.contains(&offer))
        },
        t.response * 2,
    )
    .await;
    let back = match back {
        Ok(r) => parse(&r),
        Err(Missed::Blocked(detail)) => Err(QueryOutcome::Blocked { detail }),
        Err(Missed::Timeout) => {
            Err(page_state(page).await.unwrap_or_else(|| {
                QueryOutcome::Unexpected {
                    detail: "no return-leg offers after choosing the outbound"
                        .into(),
                }
            }))
        }
    };
    let back = match back {
        Ok(o) => o,
        Err(outcome) => {
            return failed_after(
                job,
                &capture,
                seen,
                Vec::new(),
                outcome,
                loads,
                0,
            );
        }
    };
    let cells = round_trip_cells(
        (itinerary, fare),
        &back,
        job.cabin,
        (job.departure, return_date),
        &job.seller,
    );
    finish(page, job, &capture, seen, cells, loads, 0, t).await
}

/// After a one-way page was read (`cells`, one load): clicks the strip
/// days `job.strip` picks, when the strip is switched on (0084, 0090),
/// each one more search and load, and wraps up. A click whose search is
/// blocked or that shows the login keeps the cells read so far
/// ([`failed_after`]).
async fn read_strip(
    page: &Page,
    job: &LatamJob,
    capture: &tm_browser::Capture,
    mut seen: Vec<Captured>,
    mut cells: Vec<Cell>,
    t: Timeouts,
) -> Fetch<LatamFound> {
    let call = if job.award {
        OffersCall::Award
    } else {
        OffersCall::Cash
    };
    let mut loads = 1;
    let mut clicks = 0;
    if job.strip.max_clicks > 0 {
        tokio::time::sleep(t.settle).await;
        seen.extend(capture.drain());
        let mut shown = Vec::new();
        for day in strip_days(job.departure) {
            let found = page
                .eval(&page::strip_day_js(day, false))
                .await
                .unwrap_or(Value::Null);
            if !found.is_null() {
                shown.push(day);
            }
        }
        let calendar = calendar_of(&seen, job);
        let picks = job.strip.clicks(job.departure, &shown, &calendar);
        info!(shown = shown.len(), picks = ?picks, "LATAM date strip");
        for pick in picks {
            strip_pause(t).await;
            if !click_where(page, &page::strip_day_js(pick, true)).await {
                warn!(%pick, "the date strip has no button for this day: stopping");
                break;
            }
            loads += 1;
            clicks += 1;
            let got = wait_offers(
                capture,
                &mut seen,
                call,
                |url| {
                    !latam::is_return_leg(url)
                        && latam::out_from(url) == Some(pick)
                },
                t.response * 2,
            )
            .await;
            match got {
                Ok(r) => match parse(&r) {
                    Ok(o) => {
                        info!(%pick, itineraries = o.itineraries.len(), "LATAM strip day read");
                        // A day with no flights still counts as read.
                        cells.extend(leg_cells(
                            &o,
                            job.cabin,
                            pick,
                            &job.seller,
                        ));
                    }
                    Err(outcome) => {
                        warn!(%pick, ?outcome, "a strip day was not read")
                    }
                },
                Err(Missed::Blocked(detail)) => {
                    return failed_after(
                        job,
                        capture,
                        seen,
                        cells,
                        QueryOutcome::Blocked { detail },
                        loads,
                        clicks,
                    );
                }
                Err(Missed::Timeout) => {
                    if let Some(outcome) = page_state(page).await {
                        return failed_after(
                            job, capture, seen, cells, outcome, loads, clicks,
                        );
                    }
                    warn!(%pick, "no offers call after a strip click: stopping");
                    break;
                }
            }
        }
    }
    finish(page, job, capture, seen, cells, loads, clicks, t).await
}

/// A search that failed (`outcome`) after its page was read: the
/// `cells` it read and the page's calendar are kept, under the failure,
/// which the worker still treats as one (0090). The session is not read
/// on a failure.
fn failed_after(
    job: &LatamJob,
    capture: &tm_browser::Capture,
    mut seen: Vec<Captured>,
    cells: Vec<Cell>,
    outcome: QueryOutcome,
    loads: u32,
    clicks: u32,
) -> Fetch<LatamFound> {
    seen.extend(capture.drain());
    let calendar = calendar_of(&seen, job);
    warn!(
        ?outcome,
        cells = cells.len(),
        calendar = calendar.len(),
        "a later step of a LATAM search failed: keeping what was read"
    );
    Fetch::partial(
        outcome,
        LatamFound {
            cells,
            calendar,
            session: None,
            clicks,
        },
        loads,
    )
}

/// The page's calendar among `seen`.
fn calendar_of(seen: &[Captured], job: &LatamJob) -> BTreeMap<NaiveDate, i64> {
    let mut out = BTreeMap::new();
    for r in seen
        .iter()
        .filter(|r| latam::is_calendar_url(&r.url) && r.is_ok())
    {
        match latam::parse_calendar(&r.text()) {
            Ok(days) => out.extend(calendar_prices(
                &days,
                &job.origin,
                &job.destination,
            )),
            Err(e) => debug!("an unreadable LATAM calendar: {e}"),
        }
    }
    out
}

/// Reads what is left of the page's calls and the session, and wraps up.
#[allow(clippy::too_many_arguments, reason = "one search's results")]
async fn finish(
    page: &Page,
    job: &LatamJob,
    capture: &tm_browser::Capture,
    mut seen: Vec<Captured>,
    cells: Vec<Cell>,
    loads: u32,
    clicks: u32,
    t: Timeouts,
) -> Fetch<LatamFound> {
    tokio::time::sleep(t.settle / 2).await;
    seen.extend(capture.drain());
    let calendar = calendar_of(&seen, job);
    let session = session_until(page, &job.base).await;
    info!(
        cells = cells.len(),
        calendar = calendar.len(),
        loads,
        clicks,
        session = ?session,
        "LATAM search read"
    );
    Fetch::ok(
        LatamFound {
            cells,
            calendar,
            session,
            clicks,
        },
        loads,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> Offers {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/research/sites/fixtures")
            .join(name);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        latam::parse_offers(&body).unwrap_or_else(|e| panic!("{e}"))
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a date")
    }

    fn brl(minor: i64) -> Money {
        Money::new(minor, Currency::brl())
    }

    #[test]
    fn award_cells_are_every_itinerary_and_brand_of_the_cabin() {
        let award = fixture(
            "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json",
        );
        let date = day(2026, 11, 10);
        let cells = leg_cells(&award, Cabin::Economy, date, "miles:latam_pass");
        let economy: usize = award
            .itineraries
            .iter()
            .map(|i| {
                i.fares
                    .iter()
                    .filter(|f| f.cabin == Some(Cabin::Economy))
                    .count()
            })
            .sum();
        assert_eq!(cells.len(), economy);
        let light = cells
            .iter()
            .find(|c| {
                c.flights.as_deref() == Some("LA8146@2026-11-10T17:40")
                    && c.award.as_ref().and_then(|a| a.brand.as_deref())
                        == Some("LIGHT")
            })
            .expect("LA8146 LIGHT");
        assert_eq!(light.miles, Some(63_848));
        assert_eq!(light.price, Some(brl(6861)));
        assert_eq!(light.departure, date);
        assert_eq!(light.return_date, None);
        assert_eq!(light.stops, Some(0));
        assert_eq!(light.carrier.as_deref(), Some("LA"));
        assert_eq!(light.seller.as_deref(), Some("miles:latam_pass"));
        let detail = light.award.as_ref().expect("an award");
        assert_eq!(detail.fare, Some(brl(183_882)));
        assert_eq!(detail.fare_basis.as_deref(), Some("AKTSLZ6E"));
        assert_eq!(
            detail.seats,
            Some(Seats {
                count: 9,
                low: false
            })
        );
        // Business keeps its own brands.
        let business =
            leg_cells(&award, Cabin::Business, date, "miles:latam_pass");
        assert!(business.iter().all(|c| c.miles.is_some()));
        assert!(business.iter().any(|c| c.miles == Some(263_848)));
    }

    #[test]
    fn cash_cells_are_the_cabins_cheapest_brand() {
        let cash = fixture(
            "latam/offers-search-ow-gru-lis-2026-11-10-br-headed-warm.json",
        );
        let cells =
            leg_cells(&cash, Cabin::Economy, day(2026, 11, 10), "airline:LA");
        assert_eq!(cells.len(), 1);
        let c = &cells[0];
        assert_eq!(c.price, Some(brl(190_743)));
        assert_eq!(c.miles, None);
        assert_eq!(c.award, None);
        assert_eq!(c.flights.as_deref(), Some("LA8146@2026-11-10T17:40"));
        assert_eq!((c.stops, c.carrier.as_deref()), (Some(0), Some("LA")));
        // No premium economy on this route: one cell with no price.
        let none = leg_cells(
            &cash,
            Cabin::PremiumEconomy,
            day(2026, 11, 10),
            "airline:LA",
        );
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].price, None);
    }

    #[test]
    fn award_round_trips_add_up_both_legs() {
        let out = fixture(
            "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-04-outbound-br.json",
        );
        let back = fixture(
            "latam-pass/offers-search-redemption-rt-gru-mia-2027-03-11-inbound-br.json",
        );
        let (card, brand, itinerary, fare) =
            chosen_outbound(&out, Cabin::Economy).expect("an outbound");
        assert_eq!(fare.brand, "LIGHT");
        assert_eq!(&out.itineraries[card], itinerary);
        assert_eq!(&itinerary.fares[brand], fare);
        let dates = (day(2027, 3, 4), day(2027, 3, 11));
        let cells = round_trip_cells(
            (itinerary, fare),
            &back,
            Cabin::Economy,
            dates,
            "miles:latam_pass",
        );
        assert!(cells.iter().all(|c| c.return_date == Some(dates.1)));
        let Price::Miles(out_miles) = fare.price else {
            panic!("miles");
        };
        // The cheapest return plus the chosen outbound.
        let cheapest_back = back
            .cheapest(Cabin::Economy)
            .map(|(_, f)| f.price.amount())
            .expect("a return");
        let best = cells.iter().filter_map(|c| c.miles).min().expect("miles");
        assert_eq!(best, out_miles + cheapest_back);
        let c = cells
            .iter()
            .find(|c| c.miles == Some(best))
            .expect("the best");
        let key = c.flights.as_deref().expect("a key");
        assert!(key.starts_with(&itinerary.key()), "{key}");
        assert!(key.matches('@').count() >= 2);
        assert!(c.award.as_ref().and_then(|a| a.fare.clone()).is_some());
        // The nonstops' round trip: 69,069 + 75,065 (latam-pass.md §3).
        let la8198 = out
            .itineraries
            .iter()
            .find(|i| i.flights() == "LA8198")
            .expect("LA8198");
        let light = la8198.cheapest(Cabin::Economy).expect("LIGHT");
        let cells = round_trip_cells(
            (la8198, light),
            &back,
            Cabin::Economy,
            dates,
            "miles:latam_pass",
        );
        assert!(cells.iter().any(|c| c.miles == Some(144_134)
            && c.flights.as_deref().is_some_and(|k| k.contains("LA8199@"))
            && c.price == Some(brl(18_881))));
    }

    #[test]
    fn cash_round_trips_are_one_cell() {
        let out = fixture(
            "latam/offers-search-rt-gru-mia-2027-03-04-outbound-br.json",
        );
        let back = fixture(
            "latam/offers-search-rt-gru-mia-2027-03-11-inbound-br.json",
        );
        let (_, _, itinerary, fare) =
            chosen_outbound(&out, Cabin::Economy).expect("an outbound");
        let cells = round_trip_cells(
            (itinerary, fare),
            &back,
            Cabin::Economy,
            (day(2027, 3, 4), day(2027, 3, 11)),
            "airline:LA",
        );
        assert_eq!(cells.len(), 1);
        let total = cells[0].price.as_ref().expect("a price").minor;
        let (_, ret) = back.cheapest(Cabin::Economy).expect("a return");
        assert_eq!(total, fare.price.amount() + ret.price.amount());
    }

    #[test]
    fn the_calendar_ranks_one_direction() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/research/sites/fixtures/latam/calendar-ow-gru-lis-2026-11-extended.json");
        let days = latam::parse_calendar(
            &std::fs::read_to_string(path).expect("reads"),
        )
        .expect("parses");
        let prices = calendar_prices(&days, "GRU", "LIS");
        assert_eq!(prices.get(&day(2026, 11, 10)), Some(&200_141));
        assert!(calendar_prices(&days, "LIS", "GRU").is_empty());
    }
}
