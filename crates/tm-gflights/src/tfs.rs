//! The `tfs` URL parameter: a base64url protobuf that describes a
//! search (docs/research/google-flights.md, "Cabins and generated
//! search URLs").
//!
//! ```text
//! 1: 28                 constant
//! 2: 2                  constant
//! 3: leg                repeated, in order
//!    2: "2026-11-10"    date
//!    4: flight          repeated: a selected flight (search → booking)
//!       1: "GRU"  2: "2026-11-10"  3: "LIS"  5: "TP"  6: "84"
//!    13: place          from: { 1: 1 airport | 2 city, 2: "LIS" | "/m/022pfm" }
//!    14: place          to
//! 8: passenger          repeated, one per traveller:
//!                       1 adult, 2 child, 3 infant on lap, 4 infant in seat
//! 9: cabin              1 economy, 2 premium economy, 3 business, 4 first
//! 14: 1                 constant
//! 16: { 1: 2^64-1 }     no maximum price
//! 19: trip              1 round trip, 2 one way
//! ```
//!
//! Fields this module does not model are kept, in order, in `extra`,
//! and written back sorted by field number (the order protobuf
//! serializers use), so decoding and re-encoding a URL Google produced
//! gives the same bytes.

use chrono::NaiveDate;
use thiserror::Error;
use tm_domain::{Cabin, Passengers};

use crate::proto::{self, Field, ProtoError};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TfsError {
    #[error(transparent)]
    Proto(#[from] ProtoError),
    #[error("missing {0}")]
    Missing(&'static str),
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
}

fn invalid(field: &'static str, reason: impl Into<String>) -> TfsError {
    TfsError::Invalid {
        field,
        reason: reason.into(),
    }
}

/// How `tfs` (and the RPCs) name a place.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlaceRef {
    /// A single airport by IATA code (`tfs` type 1, RPC type 0).
    Airport(String),
    /// A metropolitan area by Freebase ID (`tfs` type 2, RPC type 4).
    City(String),
}

impl PlaceRef {
    pub fn id(&self) -> &str {
        match self {
            PlaceRef::Airport(id) | PlaceRef::City(id) => id,
        }
    }

    /// The type number inside `tfs`.
    pub fn tfs_kind(&self) -> u64 {
        match self {
            PlaceRef::Airport(_) => 1,
            PlaceRef::City(_) => 2,
        }
    }

    /// The type number inside the RPC requests.
    pub fn rpc_kind(&self) -> u64 {
        match self {
            PlaceRef::Airport(_) => 0,
            PlaceRef::City(_) => 4,
        }
    }

    pub fn from_rpc(id: &str, kind: u64) -> Option<Self> {
        match kind {
            0 => Some(PlaceRef::Airport(id.to_owned())),
            4 => Some(PlaceRef::City(id.to_owned())),
            _ => None,
        }
    }

    /// From a configured place: its IATA code, or its city's Freebase
    /// ID. `None` for a city whose ID has not been learned yet.
    pub fn from_place(place: &tm_domain::Place) -> Option<Self> {
        match &place.kind {
            tm_domain::PlaceKind::Airport => {
                Some(PlaceRef::Airport(place.code.clone()))
            }
            tm_domain::PlaceKind::City { freebase_id } => {
                freebase_id.clone().map(PlaceRef::City)
            }
        }
    }
}

/// Round trip or one way (`tfs` field 19, RPC search index 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trip {
    RoundTrip,
    OneWay,
}

impl Trip {
    pub fn code(self) -> u64 {
        match self {
            Trip::RoundTrip => 1,
            Trip::OneWay => 2,
        }
    }

    pub fn from_code(code: u64) -> Option<Self> {
        match code {
            1 => Some(Trip::RoundTrip),
            2 => Some(Trip::OneWay),
            _ => None,
        }
    }
}

/// A selected flight inside a leg. Present in the URLs of the
/// returning-flights page and the booking page.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlightSpec {
    pub from: String,
    pub date: NaiveDate,
    pub to: String,
    /// The marketing carrier (`"TP"`).
    pub carrier: String,
    /// The flight number without the carrier (`"84"`).
    pub number: String,
    pub extra: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LegSpec {
    pub date: NaiveDate,
    pub from: PlaceRef,
    pub to: PlaceRef,
    /// The selected flights of this leg, one per segment. Empty for a
    /// plain search.
    pub flights: Vec<FlightSpec>,
    pub extra: Vec<Field>,
}

impl LegSpec {
    pub fn new(date: NaiveDate, from: PlaceRef, to: PlaceRef) -> Self {
        Self {
            date,
            from,
            to,
            flights: Vec::new(),
            extra: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Search {
    pub legs: Vec<LegSpec>,
    pub trip: Trip,
    pub cabin: Cabin,
    pub passengers: Passengers,
    /// Fields not modelled above (the constants 1, 2, 14 and 16 among
    /// them). [`Search::new`] fills in what Google's own URLs carry.
    pub extra: Vec<Field>,
}

/// Passenger codes in field 8. Verified on 2026-10-06 by loading a
/// generated URL with `1,1,1,2,2,3,3,4` and reading the passenger
/// dialog: 3 adults, 2 children, 2 infants on lap, 1 infant in seat.
const ADULT: u64 = 1;
const CHILD: u64 = 2;
const INFANT_ON_LAP: u64 = 3;
const INFANT_IN_SEAT: u64 = 4;

impl Search {
    pub fn new(
        trip: Trip,
        cabin: Cabin,
        passengers: Passengers,
        legs: Vec<LegSpec>,
    ) -> Self {
        Self {
            legs,
            trip,
            cabin,
            passengers,
            extra: Self::default_extra(),
        }
    }

    /// The constant fields of every `tfs` Google generated in the
    /// research: `1: 28`, `2: 2`, `14: 1`, `16: { 1: 2^64-1 }`.
    pub fn default_extra() -> Vec<Field> {
        vec![
            Field::varint(1, 28),
            Field::varint(2, 2),
            Field::varint(14, 1),
            Field::message(16, &[Field::varint(1, u64::MAX)]),
        ]
    }

    pub fn round_trip(
        from: PlaceRef,
        to: PlaceRef,
        departure: NaiveDate,
        return_date: NaiveDate,
        cabin: Cabin,
        passengers: Passengers,
    ) -> Self {
        Self::new(
            Trip::RoundTrip,
            cabin,
            passengers,
            vec![
                LegSpec::new(departure, from.clone(), to.clone()),
                LegSpec::new(return_date, to, from),
            ],
        )
    }

    pub fn one_way(
        from: PlaceRef,
        to: PlaceRef,
        date: NaiveDate,
        cabin: Cabin,
        passengers: Passengers,
    ) -> Self {
        Self::new(
            Trip::OneWay,
            cabin,
            passengers,
            vec![LegSpec::new(date, from, to)],
        )
    }
}

fn merge(known: Vec<Field>, extra: &[Field]) -> Vec<Field> {
    let mut all = known;
    all.extend(extra.iter().cloned());
    // Stable: known fields stay ahead of extras with the same number,
    // and repeated fields keep their order.
    all.sort_by_key(|f| f.number);
    all
}

fn encode_place(place: &PlaceRef) -> Vec<Field> {
    vec![
        Field::varint(1, place.tfs_kind()),
        Field::bytes(2, place.id().as_bytes()),
    ]
}

fn date_string(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn encode_flight(flight: &FlightSpec) -> Vec<u8> {
    let known = vec![
        Field::bytes(1, flight.from.as_bytes()),
        Field::bytes(2, date_string(flight.date).into_bytes()),
        Field::bytes(3, flight.to.as_bytes()),
        Field::bytes(5, flight.carrier.as_bytes()),
        Field::bytes(6, flight.number.as_bytes()),
    ];
    proto::encode(&merge(known, &flight.extra))
}

fn encode_leg(leg: &LegSpec) -> Vec<u8> {
    let mut known = vec![Field::bytes(2, date_string(leg.date).into_bytes())];
    known.extend(
        leg.flights
            .iter()
            .map(|f| Field::bytes(4, encode_flight(f))),
    );
    known.push(Field::message(13, &encode_place(&leg.from)));
    known.push(Field::message(14, &encode_place(&leg.to)));
    proto::encode(&merge(known, &leg.extra))
}

/// The protobuf bytes of a search.
pub fn encode_bytes(search: &Search) -> Vec<u8> {
    let mut known: Vec<Field> = search
        .legs
        .iter()
        .map(|leg| Field::bytes(3, encode_leg(leg)))
        .collect();
    let p = &search.passengers;
    for (code, count) in [
        (ADULT, p.adults),
        (CHILD, p.children),
        (INFANT_ON_LAP, p.infants_on_lap),
        (INFANT_IN_SEAT, p.infants_in_seat),
    ] {
        known.extend((0..count).map(|_| Field::varint(8, code)));
    }
    known.push(Field::varint(9, u64::from(search.cabin.code())));
    known.push(Field::varint(19, search.trip.code()));
    proto::encode(&merge(known, &search.extra))
}

/// The `tfs` value: URL-safe base64 without padding.
pub fn encode(search: &Search) -> String {
    proto::b64_encode(&encode_bytes(search))
}

pub fn decode(tfs: &str) -> Result<Search, TfsError> {
    decode_bytes(&proto::b64_decode(tfs)?)
}

fn text(field: &Field, name: &'static str) -> Result<String, TfsError> {
    field
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid(name, "not a UTF-8 string"))
}

fn date(field: &Field, name: &'static str) -> Result<NaiveDate, TfsError> {
    let s = text(field, name)?;
    NaiveDate::parse_from_str(&s, "%Y-%m-%d")
        .map_err(|e| invalid(name, format!("{s:?}: {e}")))
}

fn submessage(
    field: &Field,
    name: &'static str,
) -> Result<Vec<Field>, TfsError> {
    let bytes = field
        .as_bytes()
        .ok_or_else(|| invalid(name, "expected a message"))?;
    Ok(proto::decode(bytes)?)
}

fn varint(field: &Field, name: &'static str) -> Result<u64, TfsError> {
    field
        .as_varint()
        .ok_or_else(|| invalid(name, "expected a number"))
}

fn decode_place(field: &Field) -> Result<PlaceRef, TfsError> {
    let mut kind = None;
    let mut id = None;
    for f in submessage(field, "place")? {
        match f.number {
            1 => kind = Some(varint(&f, "place type")?),
            2 => id = Some(text(&f, "place id")?),
            n => return Err(invalid("place", format!("unexpected field {n}"))),
        }
    }
    let id = id.ok_or(TfsError::Missing("place id"))?;
    match kind.ok_or(TfsError::Missing("place type"))? {
        1 => Ok(PlaceRef::Airport(id)),
        2 => Ok(PlaceRef::City(id)),
        k => Err(invalid("place type", format!("unknown type {k}"))),
    }
}

fn decode_flight(field: &Field) -> Result<FlightSpec, TfsError> {
    let (mut from, mut day, mut to, mut carrier, mut number) =
        (None, None, None, None, None);
    let mut extra = Vec::new();
    for f in submessage(field, "flight")? {
        match f.number {
            1 => from = Some(text(&f, "flight origin")?),
            2 => day = Some(date(&f, "flight date")?),
            3 => to = Some(text(&f, "flight destination")?),
            5 => carrier = Some(text(&f, "flight carrier")?),
            6 => number = Some(text(&f, "flight number")?),
            _ => extra.push(f),
        }
    }
    Ok(FlightSpec {
        from: from.ok_or(TfsError::Missing("flight origin"))?,
        date: day.ok_or(TfsError::Missing("flight date"))?,
        to: to.ok_or(TfsError::Missing("flight destination"))?,
        carrier: carrier.ok_or(TfsError::Missing("flight carrier"))?,
        number: number.ok_or(TfsError::Missing("flight number"))?,
        extra,
    })
}

fn decode_leg(field: &Field) -> Result<LegSpec, TfsError> {
    let (mut day, mut from, mut to) = (None, None, None);
    let mut flights = Vec::new();
    let mut extra = Vec::new();
    for f in submessage(field, "leg")? {
        match f.number {
            2 if day.is_none() => day = Some(date(&f, "leg date")?),
            4 => flights.push(decode_flight(&f)?),
            13 if from.is_none() => from = Some(decode_place(&f)?),
            14 if to.is_none() => to = Some(decode_place(&f)?),
            _ => extra.push(f),
        }
    }
    Ok(LegSpec {
        date: day.ok_or(TfsError::Missing("leg date"))?,
        from: from.ok_or(TfsError::Missing("leg origin"))?,
        to: to.ok_or(TfsError::Missing("leg destination"))?,
        flights,
        extra,
    })
}

fn count(n: &mut u8) -> Result<(), TfsError> {
    *n = n
        .checked_add(1)
        .ok_or_else(|| invalid("passengers", "more than 255 of one kind"))?;
    Ok(())
}

pub fn decode_bytes(bytes: &[u8]) -> Result<Search, TfsError> {
    let mut legs = Vec::new();
    let mut passengers = Passengers {
        adults: 0,
        children: 0,
        infants_in_seat: 0,
        infants_on_lap: 0,
    };
    let (mut cabin, mut trip) = (None, None);
    let mut extra = Vec::new();
    for f in proto::decode(bytes)? {
        match f.number {
            3 => legs.push(decode_leg(&f)?),
            8 => match varint(&f, "passenger")? {
                ADULT => count(&mut passengers.adults)?,
                CHILD => count(&mut passengers.children)?,
                INFANT_ON_LAP => count(&mut passengers.infants_on_lap)?,
                INFANT_IN_SEAT => count(&mut passengers.infants_in_seat)?,
                k => {
                    return Err(invalid(
                        "passenger",
                        format!("unknown kind {k}"),
                    ));
                }
            },
            9 if cabin.is_none() => {
                let code = varint(&f, "cabin")?;
                cabin = Some(
                    u8::try_from(code)
                        .ok()
                        .and_then(Cabin::from_code)
                        .ok_or_else(|| {
                            invalid("cabin", format!("unknown cabin {code}"))
                        })?,
                );
            }
            19 if trip.is_none() => {
                let code = varint(&f, "trip")?;
                trip = Some(Trip::from_code(code).ok_or_else(|| {
                    invalid("trip", format!("unknown trip type {code}"))
                })?);
            }
            _ => extra.push(f),
        }
    }
    Ok(Search {
        legs,
        trip: trip.ok_or(TfsError::Missing("trip type"))?,
        cabin: cabin.ok_or(TfsError::Missing("cabin"))?,
        passengers,
        extra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    const SAO: &str = "/m/022pfm";

    /// Generated with fixtures/tfs_reference.py and loaded in the
    /// browser on 2026-10-06 (fixtures/tfs_urls.txt).
    const ONE_WAY: &str = "CBwQAhokEgoyMDI2LTExLTEwag0IAhIJL20vMDIycGZtcgcIARIDTElTQAFIAXABggELCP___________wGYAQI";

    #[test]
    fn builds_the_one_way_url_the_page_loaded() {
        let search = Search::one_way(
            PlaceRef::City(SAO.into()),
            PlaceRef::Airport("LIS".into()),
            d("2026-11-10"),
            Cabin::Economy,
            Passengers::default(),
        );
        assert_eq!(encode(&search), ONE_WAY);
        assert_eq!(decode(ONE_WAY), Ok(search));
    }

    #[test]
    fn passenger_kinds() {
        let tfs = "CBwQAhokEgoyMDI2LTExLTEwag0IAhIJL20vMDIycGZtcgcIARIDTElTQAFAAUABQAJAAkADQANABEgBcAGCAQsI____________AZgBAg";
        let search = decode(tfs).map_err(|e| e.to_string());
        let p = search.as_ref().map(|s| s.passengers).ok();
        assert_eq!(
            p,
            Some(Passengers {
                adults: 3,
                children: 2,
                infants_on_lap: 2,
                infants_in_seat: 1,
            })
        );
        assert_eq!(search.map(|s| encode(&s)).ok().as_deref(), Some(tfs));
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("!!!").is_err());
        assert!(decode("").is_err(), "no trip type");
        // A leg with no date.
        let bytes = proto::encode(&[
            Field::message(3, &[Field::varint(5, 1)]),
            Field::varint(9, 1),
            Field::varint(19, 1),
        ]);
        assert_eq!(decode_bytes(&bytes), Err(TfsError::Missing("leg date")));
        let bytes = proto::encode(&[Field::varint(9, 7), Field::varint(19, 1)]);
        assert!(matches!(
            decode_bytes(&bytes),
            Err(TfsError::Invalid { field: "cabin", .. })
        ));
    }
}
