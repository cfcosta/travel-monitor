//! Parsers against the RPCs captured from the live page
//! (tests/fixtures/README.md). Expected values are the ones recorded in
//! docs/research/google-flights.md or read off the page at capture time.

use chrono::{NaiveDate, NaiveDateTime};
use tm_domain::{Cabin, Currency, Money, Passengers, PosId};
use tm_gflights::{
    PlaceRef,
    Search,
    Trip,
    plan,
    rpc::{self, CalendarCell, DateRange},
    tfs,
    url::{self, Locale},
};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn response(name: &str) -> String {
    fixture(&format!("{name}.response.txt"))
}

fn request(name: &str) -> String {
    fixture(&format!("{name}.request.txt"))
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .unwrap_or_else(|e| panic!("{s}: {e}"))
}

fn dt(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
        .unwrap_or_else(|e| panic!("{s}: {e}"))
}

fn sao() -> PlaceRef {
    PlaceRef::City("/m/022pfm".into())
}

fn lis() -> PlaceRef {
    PlaceRef::Airport("LIS".into())
}

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    r.unwrap_or_else(|e| panic!("{e:?}"))
}

fn cell<'a>(
    cells: &'a [CalendarCell],
    dep: &str,
    ret: Option<&str>,
) -> &'a CalendarCell {
    cells
        .iter()
        .find(|c| c.departure == d(dep) && c.return_date == ret.map(d))
        .unwrap_or_else(|| panic!("no cell {dep} {ret:?}"))
}

/// The tfs strings captured from the page and the ones generated in
/// code that the page loaded (tests/fixtures/tfs_urls.txt).
fn captured_urls() -> Vec<String> {
    fixture("tfs_urls.txt")
        .lines()
        .filter(|l| l.starts_with("https://"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_captured_tfs_round_trips_byte_exactly() {
    let urls = captured_urls();
    assert_eq!(urls.len(), 6);
    for u in urls {
        let tfs_value = url::query_param(&u, "tfs").unwrap_or_default();
        let search = ok(tfs::decode(&tfs_value));
        assert_eq!(tfs::encode(&search), tfs_value, "{u}");
    }
}

#[test]
fn the_booking_url_decodes_to_the_selected_flights() {
    let u = &captured_urls()[1];
    let search = ok(url::search_of(u).unwrap_or_else(|| panic!("no tfs")));
    assert_eq!(search.trip, Trip::RoundTrip);
    assert_eq!(search.cabin, Cabin::Economy);
    assert_eq!(search.passengers, Passengers::default());
    assert_eq!(search.legs.len(), 2);
    let out = &search.legs[0];
    assert_eq!(
        (out.date, &out.from, &out.to),
        (d("2026-11-10"), &sao(), &lis())
    );
    let f = &out.flights[0];
    assert_eq!(
        (
            f.from.as_str(),
            f.date,
            f.to.as_str(),
            f.carrier.as_str(),
            f.number.as_str()
        ),
        ("GRU", d("2026-11-10"), "LIS", "TP", "84")
    );
    assert_eq!(search.legs[1].flights[0].number, "89");

    // Without the selected flights it is the plain search built in code.
    let mut plain = search.clone();
    for leg in &mut plain.legs {
        leg.flights.clear();
    }
    let built = Search::round_trip(
        sao(),
        lis(),
        d("2026-11-10"),
        d("2026-11-17"),
        Cabin::Economy,
        Passengers::default(),
    );
    assert_eq!(plain, built);
    assert_eq!(tfs::encode(&plain), tfs::encode(&built));
}

#[test]
fn rpc_search_matches_the_url_search() {
    // The booking RPC's request carries the same search as the URL.
    let req = ok(rpc::form_request(&request(
        "roundtrip_sao_lis_GetBookingResults_0",
    )));
    let from_rpc = ok(rpc::search_from_rpc(&req[1]));
    let from_url =
        ok(url::search_of(&captured_urls()[1])
            .unwrap_or_else(|| panic!("no tfs")));
    assert_eq!(from_rpc, from_url);
}

#[test]
fn booking_url_from_parsed_itineraries() {
    // Pick TP84 out and TP89 back from the parsed results and rebuild
    // the booking URL the page navigated to.
    let outs = ok(rpc::parse_shopping_results(&response(
        "roundtrip_sao_lis_GetShoppingResults_0",
    )));
    let backs = ok(rpc::parse_shopping_results(&response(
        "roundtrip_sao_lis_GetShoppingResults_1",
    )));
    let tp84 = outs
        .iter()
        .find(|i| i.segments[0].number == "84")
        .unwrap_or_else(|| panic!("TP84"));
    let tp89 = backs
        .iter()
        .find(|i| i.segments[0].number == "89")
        .unwrap_or_else(|| panic!("TP89"));
    let mut search = Search::round_trip(
        sao(),
        lis(),
        d("2026-11-10"),
        d("2026-11-17"),
        Cabin::Economy,
        Passengers::default(),
    );
    search.legs[0].flights = tp84.flight_specs();
    search.legs[1].flights = tp89.flight_specs();
    let captured = &captured_urls()[1];
    let built = url::booking_url(&search, None, &Locale::english("BR"));
    assert_eq!(
        url::query_param(&built, "tfs"),
        url::query_param(captured, "tfs")
    );
    assert!(
        built.starts_with("https://www.google.com/travel/flights/booking?tfs=")
    );
    assert!(built.ends_with("&hl=en&gl=BR&curr=BRL"));
}

#[test]
fn round_trip_price_graph() {
    let cells = ok(rpc::parse_calendar(&response(
        "roundtrip_sao_lis_GetCalendarGraph_0",
    )));
    assert_eq!(cells.len(), 21);
    let c = cell(&cells, "2027-01-02", Some("2027-01-09"));
    assert_eq!(c.price, Some(9326));
    assert_eq!(c.exact, Some(Money::new(932_572, Currency::brl())));
    assert_eq!(c.money(&Currency::brl()), Some(Money::brl_whole(9326)));
    assert!(
        cells
            .iter()
            .all(|c| c.return_date == Some(c.departure + chrono::Days::new(7)))
    );
    assert_eq!(
        cell(&cells, "2027-01-20", Some("2027-01-27")).price,
        Some(4982)
    );

    let req = ok(rpc::parse_calendar_request(&request(
        "roundtrip_sao_lis_GetCalendarGraph_0",
    )));
    assert_eq!(
        req.departures,
        DateRange {
            first: d("2027-01-02"),
            last: d("2027-01-22")
        }
    );
    assert_eq!(req.departures.days(), 21);
    assert_eq!(req.trip_length, Some((7, 7)));
    assert_eq!(req.returns, None);
    assert_eq!(
        req.search,
        Search::round_trip(
            sao(),
            lis(),
            d("2026-11-10"),
            d("2026-11-17"),
            Cabin::Economy,
            Passengers::default()
        )
    );
    // Every page's cells fall in the range its request asked for.
    for n in 0..3 {
        let name = format!("roundtrip_sao_lis_GetCalendarGraph_{n}");
        let req = ok(rpc::parse_calendar_request(&request(&name)));
        let cells = ok(rpc::parse_calendar(&response(&name)));
        assert!(!cells.is_empty());
        assert!(
            cells.iter().all(|c| req.departures.contains(c.departure)),
            "{name}"
        );
    }
}

#[test]
fn round_trip_date_grid() {
    let cells = ok(rpc::parse_calendar(&response(
        "roundtrip_sao_lis_GetCalendarGrid_3",
    )));
    assert_eq!(cells.len(), 7);
    assert_eq!(
        cell(&cells, "2026-11-14", Some("2026-11-15")).price,
        Some(4225)
    );
    assert_eq!(
        cell(&cells, "2026-11-14", Some("2026-11-17")).price,
        Some(4082)
    );
    let same_day = cell(&cells, "2026-11-14", Some("2026-11-14"));
    assert_eq!((same_day.price, &same_day.exact), (None, &None));

    let req = ok(rpc::parse_calendar_request(&request(
        "roundtrip_sao_lis_GetCalendarGrid_3",
    )));
    assert_eq!(req.departures.days(), 1);
    assert_eq!(
        req.returns,
        Some(DateRange {
            first: d("2026-11-14"),
            last: d("2026-11-20")
        })
    );
    assert_eq!(req.trip_length, None);
}

#[test]
fn one_way_grid_and_graph() {
    let cells = ok(rpc::parse_calendar(&response(
        "oneway_sao_lis_GetCalendarGrid_0",
    )));
    assert_eq!(cells.len(), 8);
    assert!(cells.iter().all(|c| c.return_date.is_none()));
    assert_eq!(cell(&cells, "2026-11-08", None).price, Some(1827));
    assert_eq!(cell(&cells, "2026-11-14", None).price, Some(1912));

    let req = ok(rpc::parse_calendar_request(&request(
        "oneway_sao_lis_GetCalendarGrid_0",
    )));
    assert_eq!(req.search.trip, Trip::OneWay);
    assert_eq!(req.search.legs.len(), 1);
    assert_eq!(
        req.search,
        Search::one_way(
            sao(),
            lis(),
            d("2026-11-10"),
            Cabin::Economy,
            Passengers::default()
        )
    );

    // The first graph page covers 60 days; each scroll 21 more.
    let first = ok(rpc::parse_calendar_request(&request(
        "oneway_sao_lis_GetCalendarGraph_1",
    )));
    assert_eq!(first.departures.first, d("2026-11-03"));
    assert_eq!(
        first.departures.days(),
        i64::from(plan::GRAPH_FIRST_PAGE_DAYS)
    );
    let first_cells = ok(rpc::parse_calendar(&response(
        "oneway_sao_lis_GetCalendarGraph_1",
    )));
    assert!(first_cells.len() >= 50, "{}", first_cells.len());
    assert!(
        first_cells
            .iter()
            .all(|c| first.departures.contains(c.departure))
    );

    let page2 = ok(rpc::parse_calendar_request(&request(
        "oneway_sao_lis_GetCalendarGraph_2",
    )));
    let page3 = ok(rpc::parse_calendar_request(&request(
        "oneway_sao_lis_GetCalendarGraph_3",
    )));
    assert_eq!(page2.departures.days(), i64::from(plan::GRAPH_SCROLL_DAYS));
    assert_eq!(
        page2.departures.first,
        first.departures.last + chrono::Days::new(1)
    );
    assert_eq!(
        page3.departures.first,
        page2.departures.last + chrono::Days::new(1)
    );
    let pages = [first, page2, page3];
    assert_eq!(plan::covered_until(&pages), Some(d("2027-02-12")));
    assert!(plan::reaches(&pages, d("2027-02-12")));
    assert!(!plan::reaches(&pages, d("2027-02-13")));
    assert_eq!(plan::graph_days(3), 102);

    let cells = ok(rpc::parse_calendar(&response(
        "oneway_sao_lis_GetCalendarGraph_2",
    )));
    assert_eq!(cells.len(), 21);
    assert_eq!(cell(&cells, "2027-01-02", None).price, Some(5896));
    assert_eq!(cell(&cells, "2027-01-18", None).price, Some(3002));
}

#[test]
fn explore_economy() {
    let all = ok(rpc::parse_explore(&response(
        "explore_sao_economy_GetExploreDestinations_0",
    )));
    assert_eq!(all.len(), 72);
    assert_eq!(all.iter().filter(|x| x.price.is_some()).count(), 71);
    let lis = all
        .iter()
        .find(|x| x.freebase_id == "/m/04llb")
        .unwrap_or_else(|| panic!("LIS"));
    assert_eq!(lis.airport.as_deref(), Some("LIS"));
    assert_eq!(lis.origin_airport.as_deref(), Some("GRU"));
    assert_eq!(lis.price, Some(3825));
    assert_eq!(lis.exact, Some(Money::new(382_485, Currency::brl())));
    assert_eq!(lis.departure, Some(d("2026-11-12")));
    assert_eq!(lis.return_date, Some(d("2026-11-19")));
    assert_eq!(lis.stops, Some(1));
    assert_eq!(lis.duration_minutes, Some(1131));
    assert_eq!(lis.carrier_code.as_deref(), Some("multi"));
    assert_eq!(
        lis.carrier.as_deref(),
        Some("Air France, Transavia, and KLM")
    );
    assert_eq!(lis.name.as_deref(), Some("Lisbon"));
    assert_eq!(lis.country.as_deref(), Some("Portugal"));
    assert!(
        lis.coordinates
            .is_some_and(|(lat, lon)| (lat - 38.72).abs() < 0.01
                && (lon + 9.14).abs() < 0.01)
    );

    let rio = all
        .iter()
        .find(|x| x.airport.as_deref() == Some("GIG"))
        .unwrap_or_else(|| panic!("GIG"));
    assert_eq!(
        (rio.price, rio.stops, rio.carrier.as_deref()),
        (Some(718), Some(0), Some("Gol"))
    );
    let aep = all
        .iter()
        .find(|x| x.airport.as_deref() == Some("AEP"))
        .unwrap_or_else(|| panic!("AEP"));
    assert_eq!(aep.price, Some(1572));

    let unpriced = all
        .iter()
        .find(|x| x.freebase_id == "/m/010f1kyh")
        .unwrap_or_else(|| panic!("unpriced"));
    assert_eq!(
        (
            unpriced.price,
            unpriced.stops,
            unpriced.carrier.as_deref(),
            unpriced.departure
        ),
        (None, None, None, None)
    );
}

#[test]
fn explore_business() {
    let all = ok(rpc::parse_explore(&response(
        "explore_sao_business_GetExploreDestinations_0",
    )));
    // 68 in the first payload, 4 more streamed one by one.
    assert_eq!(all.len(), 72);
    let lis = all
        .iter()
        .find(|x| x.freebase_id == "/m/04llb")
        .unwrap_or_else(|| panic!("LIS"));
    assert_eq!(lis.price, Some(10446));
    assert_eq!(
        (lis.departure, lis.return_date),
        (Some(d("2027-02-18")), Some(d("2027-02-24")))
    );
    assert_eq!((lis.stops, lis.duration_minutes), (Some(0), Some(595)));
    assert_eq!(lis.carrier.as_deref(), Some("Tap Air Portugal"));
    assert!(
        all.iter()
            .any(|x| x.airport.as_deref() == Some("MEL")
                && x.price == Some(29762))
    );

    let req = ok(rpc::form_request(&request(
        "explore_sao_business_GetExploreDestinations_0",
    )));
    assert_eq!(req[3][5], 3, "the business cabin");
}

#[test]
fn shopping_results_round_trip() {
    let outs = ok(rpc::parse_shopping_results(&response(
        "roundtrip_sao_lis_GetShoppingResults_0",
    )));
    assert_eq!(outs.len(), 11);
    assert_eq!(outs.iter().filter(|i| i.best).count(), 5);
    let tp84 = &outs[0];
    assert_eq!(tp84.carrier, "TP");
    assert_eq!(tp84.airlines, vec!["Tap Air Portugal".to_owned()]);
    assert_eq!((tp84.from.as_str(), tp84.to.as_str()), ("GRU", "LIS"));
    assert_eq!(tp84.departure, dt("2026-11-10 01:30"));
    assert_eq!(tp84.arrival, dt("2026-11-10 14:25"));
    assert_eq!(tp84.duration_minutes, 595);
    assert_eq!(tp84.stops(), 0);
    assert_eq!(tp84.price, Some(3951));
    assert!(tp84.best);
    assert!(tp84.token.is_some());
    let seg = &tp84.segments[0];
    assert_eq!((seg.carrier.as_str(), seg.number.as_str()), ("TP", "84"));
    assert_eq!(seg.carrier_name.as_deref(), Some("Tap Air Portugal"));

    let klm = outs
        .iter()
        .find(|i| i.carrier == "multi" && i.price == Some(3920))
        .unwrap_or_else(|| panic!("KL792"));
    assert!(!klm.best);
    assert_eq!(klm.stops(), 1);
    assert_eq!(klm.duration_minutes, 1131);
    assert_eq!(
        klm.segments
            .iter()
            .map(|s| format!("{}{}", s.carrier, s.number))
            .collect::<Vec<_>>(),
        ["KL792", "HV5955"]
    );

    // Returning flights after picking TP84: prices are round-trip totals.
    let backs = ok(rpc::parse_shopping_results(&response(
        "roundtrip_sao_lis_GetShoppingResults_1",
    )));
    assert_eq!(backs.len(), 5);
    let first = &backs[0];
    assert_eq!(first.departure, dt("2026-11-17 10:00"), "[10] is 10:00");
    assert_eq!(first.stops(), 1);
    assert_eq!(
        first.segments[0].operated_by.as_deref(),
        Some("Portugalia - Companhia Portuguesa")
    );
    assert_eq!(first.segments[0].aircraft.as_deref(), Some("Embraer 195"));
    let tp89 = backs
        .iter()
        .find(|i| i.segments[0].number == "89")
        .unwrap_or_else(|| panic!("TP89"));
    assert_eq!(tp89.price, Some(4051));
    assert_eq!(tp89.departure, dt("2026-11-17 12:30"));
}

#[test]
fn shopping_results_from_the_page_html() {
    // A page loaded from a URL embeds its first results as ds:1.
    let script = fixture("roundtrip_sao_lis_initdata_ds1.txt");
    let (key, _) = ok(rpc::parse_init_data(&script));
    assert_eq!(key, "ds:1");
    let embedded = ok(rpc::shopping_results_from_init_data(&script));
    let rpc_list = ok(rpc::parse_shopping_results(&response(
        "roundtrip_sao_lis_GetShoppingResults_0",
    )));
    assert_eq!(embedded.len(), 11);
    let key = |i: &rpc::Itinerary| {
        (i.departure, i.segments[0].number.clone(), i.price)
    };
    assert_eq!(
        embedded.iter().map(key).collect::<Vec<_>>(),
        rpc_list.iter().map(key).collect::<Vec<_>>()
    );

    let one_way = ok(rpc::shopping_results_from_init_data(&fixture(
        "oneway_sao_lis_initdata_ds1.txt",
    )));
    assert_eq!(one_way.len(), 9);
    let ad = &one_way[0];
    assert_eq!((ad.carrier.as_str(), ad.from.as_str()), ("AD", "VCP"));
    assert_eq!(ad.departure, dt("2026-11-10 21:05"));
    assert_eq!(ad.price, Some(1827));
    assert!(
        one_way
            .iter()
            .any(|i| i.segments[0].number == "88" && i.price == Some(1912))
    );
}

#[test]
fn booking_results_gl_br() {
    let offers = ok(rpc::parse_booking_results(&response(
        "roundtrip_sao_lis_GetBookingResults_0",
    )));
    assert_eq!(offers.len(), 11);
    let tap = &offers[0];
    assert_eq!(tap.seller(), "Tap Air Portugal");
    assert!(tap.is_airline());
    assert_eq!(tap.price, Some(4051));
    assert_eq!(
        tap.flights,
        vec![
            ("TP".to_owned(), "84".to_owned()),
            ("TP".to_owned(), "89".to_owned())
        ]
    );
    let kiss = &offers[1];
    assert_eq!(
        (kiss.seller().as_str(), kiss.is_airline(), kiss.price),
        ("kiss&fly", false, Some(3912))
    );
    assert!(offers.iter().all(|o| o.quoted.is_none()));
    assert_eq!(offers.iter().filter(|o| o.is_airline()).count(), 1);
    let kiwi = offers
        .iter()
        .find(|o| o.sellers[0].code == "KIWI")
        .unwrap_or_else(|| panic!("Kiwi"));
    assert_eq!(kiwi.price, Some(4151));

    let quote = tap.to_quote(PosId(1), &Currency::brl());
    assert_eq!(
        quote.map(|q| (q.seller, q.is_airline, q.price)),
        Some(("Tap Air Portugal".into(), true, Money::brl_whole(4051)))
    );
}

#[test]
fn booking_results_gl_us_quote_dollars() {
    let offers = ok(rpc::parse_booking_results(&response(
        "roundtrip_sao_lis_us_GetBookingResults_0",
    )));
    assert_eq!(offers.len(), 18);
    let usd = |n| Some(Money::from_whole(n, Currency::new("USD")));
    let booking = offers
        .iter()
        .find(|o| o.seller() == "Booking.com")
        .unwrap_or_else(|| panic!("Booking"));
    assert_eq!(
        (booking.price, booking.quoted.clone()),
        (Some(3709), usd(742))
    );
    let fnw = offers
        .iter()
        .find(|o| o.seller() == "Flightnetwork")
        .unwrap_or_else(|| panic!("FN"));
    assert_eq!((fnw.price, fnw.quoted.clone()), (Some(3842), usd(769)));
    assert_eq!(fnw.sellers[0].code, "ETRAVELI_FLIGHTNETWORK");
    let kiss = offers
        .iter()
        .find(|o| o.seller() == "kiss&fly")
        .unwrap_or_else(|| panic!("kiss"));
    assert_eq!(kiss.quoted, None);
    let q = booking
        .to_quote(PosId(2), &Currency::brl())
        .unwrap_or_else(|| panic!("quote"));
    assert_eq!((q.price, q.quoted), (Money::brl_whole(3709), usd(742)));

    // The same booking URL loaded directly under gl=US, with the tfu
    // of a BR session: only the airline.
    let direct = ok(rpc::parse_booking_results(&response(
        "roundtrip_sao_lis_us_direct_GetBookingResults_0",
    )));
    assert_eq!(direct.len(), 1);
    assert!(direct[0].is_airline());
}

#[test]
fn parsers_reject_each_others_responses() {
    let graph = response("roundtrip_sao_lis_GetCalendarGraph_0");
    let booking = response("roundtrip_sao_lis_GetBookingResults_0");
    assert!(rpc::parse_shopping_results(&graph).is_err());
    assert!(rpc::parse_booking_results(&graph).is_err());
    assert!(rpc::parse_explore(&graph).is_err());
    assert!(rpc::parse_calendar(&booking).is_err());
    assert!(rpc::parse_calendar("").is_err());
}

#[test]
fn frames_of_a_large_response() {
    let env = ok(rpc::frames(&response(
        "explore_sao_economy_GetExploreDestinations_0",
    )));
    let kinds: Vec<&str> = env.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["wrb.fr", "wrb.fr", "di", "af.httprm", "e"]);
    assert_eq!(
        ok(rpc::payloads(&response(
            "roundtrip_sao_lis_GetShoppingResults_0"
        )))
        .len(),
        6
    );
}

#[test]
fn resolving_a_city() {
    let search = ok(rpc::parse_data_service_requests(&fixture(
        "resolve_rome_dataServiceRequests.txt",
    )));
    assert_eq!(search.trip, Trip::RoundTrip);
    assert_eq!(search.legs[0].from, sao());
    assert_eq!(search.legs[0].to, PlaceRef::City("/m/06c62".into()));
    assert_eq!(search.legs[0].date, d("2026-11-12"));
    assert_eq!(search.legs[1].date, d("2026-11-18"));
}

#[test]
fn passenger_counts_reach_the_rpc() {
    // The generated URL with 3 adults, 2 children, 2 lap and 1 seat
    // infants made the page request [3, 2, 2, 1].
    let u = &captured_urls()[5];
    let search = ok(url::search_of(u).unwrap_or_else(|| panic!("tfs")));
    let rpc_like = serde_json::json!([
        null,
        null,
        2,
        null,
        [],
        1,
        [3, 2, 2, 1],
        null,
        null,
        null,
        null,
        null,
        null,
        [[
            [[["/m/022pfm", 4]]],
            [[["LIS", 0]]],
            null,
            0,
            null,
            null,
            "2026-11-10",
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            3
        ]],
        null,
        null,
        null,
        1
    ]);
    assert_eq!(ok(rpc::search_from_rpc(&rpc_like)), search);
}
