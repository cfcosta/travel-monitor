//! TAP's parsers against the responses captured on 2026-10-06
//! (`docs/research/sites/fixtures/tap/`, DECISIONS.md 0051).

use std::path::PathBuf;

use chrono::{NaiveDate, TimeDelta};
use tm_daemon::sites::tap::{
    self,
    CalendarDay,
    parse_availability,
    parse_calendar,
    parse_calendar_request,
    parse_search_request,
};
use tm_domain::{Cabin, Currency, Money};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tap")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The `body` of a captured request, as the page sent it.
fn request_body(name: &str) -> String {
    let v: serde_json::Value =
        serde_json::from_str(&fixture(name)).expect("JSON");
    v["body"].to_string()
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("a date")
}

fn brl(minor: i64) -> Money {
    Money::new(minor, Currency::brl())
}

fn calendar() -> Vec<CalendarDay> {
    parse_calendar(&fixture("calendar.GRU-LIS.2026-11.oneway.json"))
        .expect("parses")
}

#[test]
fn the_calendar_holds_a_month_of_cached_days() {
    let days = calendar();
    assert_eq!(days.len(), 30);
    assert_eq!(days[0].departure, day(2026, 11, 1));
    assert_eq!(days[29].departure, day(2026, 11, 30));
    assert!(
        days.iter()
            .all(|d| d.origin == "GRU" && d.destination == "LIS")
    );
    assert!(days.iter().all(|d| !d.sold_out && !d.no_flights));
    let nov10 = days
        .iter()
        .find(|d| d.departure == day(2026, 11, 10))
        .expect("2026-11-10");
    assert_eq!(nov10.price, Some(brl(191_199)));
    assert_eq!(nov10.inserted, day(2026, 10, 6).and_hms_opt(5, 56, 36));
    let nov1 = &days[0];
    assert_eq!(nov1.price, Some(brl(199_994)));
    assert_eq!(nov1.inserted, day(2026, 10, 3).and_hms_opt(5, 48, 28));
}

#[test]
fn stale_days_are_not_fresh() {
    let days = calendar();
    // As if checked at noon UTC on the capture day: everything stored
    // on 10-06 is fresh, the days stored on 10-03 are not.
    let now = day(2026, 10, 6)
        .and_hms_opt(12, 0, 0)
        .expect("a time")
        .and_utc();
    let since = tap::fresh_since(now);
    assert_eq!(since, now - TimeDelta::hours(48));
    let fresh: Vec<&CalendarDay> =
        days.iter().filter(|d| d.is_fresh(since)).collect();
    assert_eq!(fresh.len(), 22);
    assert_eq!(fresh[0].departure, day(2026, 11, 2));
    assert_eq!(fresh[21].departure, day(2026, 11, 23));
    assert!(fresh.iter().all(|d| d.price == Some(brl(191_199))));
    // Three days later, nothing in it is fresh any more.
    let later = tap::fresh_since(now + TimeDelta::days(3));
    assert_eq!(days.iter().filter(|d| d.is_fresh(later)).count(), 0);
}

#[test]
fn the_calendar_request_names_its_month() {
    let q = parse_calendar_request(&request_body("calendar.request.json"))
        .expect("parses");
    assert_eq!(q.origin, "GRU");
    assert_eq!(q.destination, "LIS");
    assert_eq!(q.market, "BR");
    assert_eq!(q.trip_type, "O");
    assert_eq!(q.cabin_class, "E");
    assert_eq!(q.first_day(), Some(day(2026, 11, 1)));
}

#[test]
fn the_availability_search_has_every_cabin() {
    let a = parse_availability(&fixture(
        "availability-search.GRU-LIS.2026-11-10.oneway.json",
    ))
    .expect("parses");
    assert_eq!(a.currency, Currency::brl());
    assert_eq!(a.office.as_deref(), Some("SAOTP08AA"));
    assert_eq!(a.flights.len(), 35);
    assert_eq!(a.offers.len(), 92);
    assert_eq!(a.flights.iter().filter(|f| f.stops == Some(0)).count(), 3);
    assert!(a.offers.iter().all(|o| o.cabin.is_some()));

    let (eco, flight) = a.cheapest(Cabin::Economy).expect("economy");
    assert_eq!(eco.price, brl(191_199));
    assert_eq!(eco.fare_family, "DISCINT");
    assert_eq!(eco.cabin_code, "M");
    let flight = flight.expect("its flight");
    assert_eq!(flight.stops, Some(0));
    assert_eq!(flight.duration_minutes, Some(590));
    assert_eq!(flight.carrier().as_deref(), Some("TP"));
    let tp88 = &flight.segments[0];
    assert_eq!((tp88.from.as_str(), tp88.to.as_str()), ("GRU", "LIS"));
    assert_eq!(tp88.number, "88");
    assert_eq!(tp88.operated_by.as_deref(), Some("TP"));
    // Local time, despite the `Z`.
    assert_eq!(tp88.departs, day(2026, 11, 10).and_hms_opt(21, 45, 0));

    let (prime, _) = a.cheapest(Cabin::PremiumEconomy).expect("prime");
    assert_eq!(prime.price, brl(563_972));
    assert_eq!(prime.cabin_code, "W");
    assert_eq!(prime.fare_family, "RPEINT");
    let (business, _) = a.cheapest(Cabin::Business).expect("business");
    assert_eq!(business.price, brl(1_214_279));
    assert_eq!(business.cabin_code, "C");
    assert_eq!(business.fare_family, "EXEINT");
    assert!(a.cheapest(Cabin::First).is_none());

    let families: std::collections::BTreeSet<&str> =
        a.offers.iter().map(|o| o.fare_family.as_str()).collect();
    assert_eq!(
        families.into_iter().collect::<Vec<_>>(),
        [
            "BASINT", "CLAINT", "DISCINT", "EXEINT", "PLUSINT", "RPEINT",
            "TOPINT", "TPEINT"
        ]
    );

    // The 7-day strip: 3 days each side of the searched date.
    let dates: Vec<NaiveDate> = a.strip.iter().map(|s| s.date).collect();
    assert_eq!(
        dates,
        (7..=13).map(|d| day(2026, 11, d)).collect::<Vec<_>>()
    );
    assert!(a.strip.iter().all(|s| s.price == Some(brl(191_199))));
}

#[test]
fn the_search_request_names_its_date() {
    let q =
        parse_search_request(&request_body("availability-search.request.json"))
            .expect("parses");
    assert_eq!(q.origin, "GRU");
    assert_eq!(q.destination, "LIS");
    assert_eq!(q.departure, day(2026, 11, 10));
    assert_eq!(q.market, "BR");
    assert!(q.one_way);
}
