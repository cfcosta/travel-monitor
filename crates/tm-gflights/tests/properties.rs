//! Rules that hold for every input: the wire codec and `tfs` round-trip,
//! encoding is canonical, and parsers never panic on garbage.

use chrono::{Datelike, Days, NaiveDate};
use hegel::generators::{self as gs, Generator as _};
use tm_domain::{Cabin, Passengers};
use tm_gflights::{
    FlightSpec,
    LegSpec,
    PlaceRef,
    Search,
    Trip,
    page,
    proto::{self, Field, Value},
    rpc,
    tfs,
    url,
};

#[hegel::composite]
fn date(tc: &hegel::TestCase) -> NaiveDate {
    let base = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap_or_default();
    base + Days::new(tc.draw(gs::integers::<u64>().max_value(3 * 366)))
}

#[hegel::composite]
fn place(tc: &hegel::TestCase) -> PlaceRef {
    if tc.draw(gs::booleans()) {
        PlaceRef::Airport(tc.draw(gs::from_regex("[A-Z]{3}").fullmatch(true)))
    } else {
        PlaceRef::City(
            tc.draw(gs::from_regex("/[mg]/0[0-9a-z_]{2,9}").fullmatch(true)),
        )
    }
}

#[hegel::composite]
fn flight(tc: &hegel::TestCase) -> FlightSpec {
    FlightSpec {
        from: tc.draw(gs::from_regex("[A-Z]{3}").fullmatch(true)),
        date: tc.draw(date().print_as_debug()),
        to: tc.draw(gs::from_regex("[A-Z]{3}").fullmatch(true)),
        carrier: tc.draw(gs::from_regex("[A-Z0-9]{2}").fullmatch(true)),
        number: tc.draw(gs::from_regex("[0-9]{1,4}").fullmatch(true)),
        extra: Vec::new(),
    }
}

#[hegel::composite]
fn leg(tc: &hegel::TestCase) -> LegSpec {
    LegSpec {
        date: tc.draw(date().print_as_debug()),
        from: tc.draw(place().print_as_debug()),
        to: tc.draw(place().print_as_debug()),
        flights: tc.draw(gs::vecs(flight().print_as_debug()).max_size(3)),
        extra: Vec::new(),
    }
}

#[hegel::composite]
fn search(tc: &hegel::TestCase) -> Search {
    let trip = if tc.draw(gs::booleans()) {
        Trip::RoundTrip
    } else {
        Trip::OneWay
    };
    let legs =
        tc.draw(gs::vecs(leg().print_as_debug()).min_size(1).max_size(2));
    let cabin = tc.draw(gs::sampled_from(Cabin::ALL.to_vec()).print_as_debug());
    let n = || gs::integers::<u8>().max_value(9);
    let passengers = Passengers {
        adults: tc.draw(n()),
        children: tc.draw(n()),
        infants_in_seat: tc.draw(n()),
        infants_on_lap: tc.draw(n()),
    };
    Search::new(trip, cabin, passengers, legs)
}

#[hegel::composite]
fn fields(tc: &hegel::TestCase) -> Vec<Field> {
    let n: usize = tc.draw(gs::integers::<usize>().max_value(8));
    (0..n)
        .map(|_| {
            let number = tc.draw(
                gs::integers::<u32>().min_value(1).max_value((1 << 29) - 1),
            );
            if tc.draw(gs::booleans()) {
                Field::varint(number, tc.draw(gs::integers::<u64>()))
            } else {
                Field::bytes(number, tc.draw(gs::binary().max_size(40)))
            }
        })
        .collect()
}

#[hegel::test(test_cases = 500)]
fn varints_round_trip(tc: hegel::TestCase) {
    let v: u64 = tc.draw(gs::integers::<u64>());
    let mut out = Vec::new();
    proto::write_varint(&mut out, v);
    assert!(out.len() <= 10);
    assert!(out.iter().take(out.len() - 1).all(|b| b & 0x80 != 0));
    let mut pos = 0;
    assert_eq!(proto::read_varint(&out, &mut pos), Ok(v));
    assert_eq!(pos, out.len());
}

#[hegel::test(test_cases = 300)]
fn messages_round_trip(tc: hegel::TestCase) {
    let fields: Vec<Field> = tc.draw(fields().print_as_debug());
    let bytes = proto::encode(&fields);
    assert_eq!(proto::decode(&bytes), Ok(fields));
    assert_eq!(proto::b64_decode(&proto::b64_encode(&bytes)), Ok(bytes));
}

#[hegel::test(test_cases = 300)]
fn decoding_garbage_never_panics(tc: hegel::TestCase) {
    let bytes: Vec<u8> = tc.draw(gs::binary().max_size(64));
    if let Ok(fields) = proto::decode(&bytes) {
        // What decodes re-encodes to the same bytes, unless a varint
        // was written in a non-minimal form.
        let again = proto::encode(&fields);
        assert_eq!(proto::decode(&again), Ok(fields));
    }
    let _ = tfs::decode_bytes(&bytes);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let _ = tfs::decode(&text);
    let _ = rpc::frames(&text);
    let _ = rpc::parse_calendar(&text);
    let _ = rpc::parse_shopping_results(&text);
    let _ = rpc::parse_booking_results(&text);
    let _ = rpc::parse_explore(&text);
    let _ = rpc::parse_calendar_request(&text);
    let _ = rpc::parse_init_data(&text);
    let _ = rpc::parse_data_service_requests(&text);
    let _ = page::detect_block(&text, &text);
    let _ = page::parse_grid_label(
        &text,
        NaiveDate::from_ymd_opt(2026, 10, 6).unwrap_or_default(),
    );
}

#[hegel::test(test_cases = 300)]
fn searches_round_trip(tc: hegel::TestCase) {
    let s: Search = tc.draw(search().print_as_debug());
    let encoded = tfs::encode(&s);
    assert!(
        encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    assert_eq!(tfs::decode(&encoded), Ok(s.clone()));
    // A search URL carries the same search.
    let u = url::search_url(&s, &url::Locale::english("BR"));
    assert_eq!(url::search_of(&u), Some(Ok(s)));
}

#[hegel::test(test_cases = 200)]
fn decoded_searches_re_encode_byte_exactly(tc: hegel::TestCase) {
    // Unknown fields, in field-number order as protobuf writes them,
    // survive a decode and re-encode.
    let s: Search = tc.draw(search().print_as_debug());
    let mut extra: Vec<Field> = tc.draw(fields().print_as_debug());
    extra.retain(|f| ![3, 8, 9, 19].contains(&f.number));
    extra.sort_by_key(|f| f.number);
    let mut with_extra = s.clone();
    with_extra.extra = extra;
    let bytes = tfs::encode_bytes(&with_extra);
    let decoded = tfs::decode_bytes(&bytes);
    assert_eq!(decoded.as_ref().map(tfs::encode_bytes), Ok(bytes));
    assert_eq!(decoded, Ok(with_extra));
}

#[hegel::test(test_cases = 300)]
fn components_round_trip(tc: hegel::TestCase) {
    let text: String = tc.draw(gs::text().max_size(40));
    let enc = url::encode_component(&text);
    assert!(
        enc.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.~%".contains(&b))
    );
    assert_eq!(url::decode_component(&enc, true), text);
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
    "Nov", "Dec",
];

fn grouped(n: i64) -> String {
    let raw = n.to_string();
    let mut out = String::new();
    for (i, c) in raw.chars().enumerate() {
        if i > 0 && (raw.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[hegel::test(test_cases = 300)]
fn grid_labels_round_trip(tc: hegel::TestCase) {
    let today = NaiveDate::from_ymd_opt(2026, 10, 6).unwrap_or_default();
    let dep = today + Days::new(tc.draw(gs::integers::<u64>().max_value(330)));
    let ret = dep + Days::new(tc.draw(gs::integers::<u64>().max_value(30)));
    let price: i64 =
        tc.draw(gs::integers::<i64>().min_value(1).max_value(999_999));
    let cheapest: bool = tc.draw(gs::booleans());
    let round_trip: bool = tc.draw(gs::booleans());
    let fmt =
        |d: NaiveDate| format!("{} {}", MONTHS[d.month0() as usize], d.day());
    let mut label = format!("R${}, ", grouped(price));
    if cheapest {
        label.push_str("cheapest price, ");
    }
    label.push_str(&fmt(dep));
    if round_trip {
        label.push_str(&format!(" to {}", fmt(ret)));
    }
    let parsed = page::parse_grid_label(&label, today);
    assert_eq!(
        parsed,
        Ok(page::GridLabel {
            price: Some(price),
            currency_symbol: Some("R$".into()),
            cheapest,
            departure: dep,
            return_date: round_trip.then_some(ret),
        }),
        "{label}"
    );
}

#[test]
fn varint_wire_values() {
    // Exhaustive over the one- and two-byte range.
    for v in 0..(1u64 << 14) {
        let mut out = Vec::new();
        proto::write_varint(&mut out, v);
        assert_eq!(out.len(), if v < 128 { 1 } else { 2 });
        let mut pos = 0;
        assert_eq!(proto::read_varint(&out, &mut pos), Ok(v));
    }
    assert_eq!(
        proto::decode(&[0x08, 0x96, 0x01]),
        Ok(vec![Field {
            number: 1,
            value: Value::Varint(150)
        }])
    );
}
