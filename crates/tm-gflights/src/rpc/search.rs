//! The search array inside RPC requests and the page's embedded
//! `AF_dataServiceRequests`:
//!
//! ```text
//! [null, null, trip (1 round trip, 2 one way), null, [], cabin,
//!  [adults, children, infants on lap, infants in seat],
//!  null × 6,
//!  [leg, …],            index 13
//!  …]
//! leg: [[[[id, kind], …]] from, [[[id, kind], …]] to, null, 0, null, null,
//!       "2026-11-10", null, [[from, date, to, null, carrier, number], …] | null,
//!       …]
//! kind: 0 airport, 4 city (Freebase ID)
//! ```

use serde_json::Value;
use tm_domain::{Cabin, Passengers};

use super::{RpcError, at, date_str, first_json, str_at, unrecognized};
use crate::tfs::{FlightSpec, LegSpec, PlaceRef, Search, Trip};

fn small(v: Option<&Value>, what: &'static str) -> Result<u8, RpcError> {
    v.and_then(Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .ok_or_else(|| unrecognized(what, format!("{v:?}")))
}

fn place(v: Option<&Value>, what: &'static str) -> Result<PlaceRef, RpcError> {
    // [[[id, kind], …]]: the first place of the first group.
    let p = v
        .and_then(|v| at(v, &[0, 0]))
        .ok_or_else(|| unrecognized(what, format!("{v:?}")))?;
    let id =
        str_at(p, &[0]).ok_or_else(|| unrecognized(what, p.to_string()))?;
    let kind = at(p, &[1])
        .and_then(Value::as_u64)
        .ok_or_else(|| unrecognized(what, p.to_string()))?;
    PlaceRef::from_rpc(id, kind)
        .ok_or_else(|| unrecognized(what, p.to_string()))
}

fn flight(v: &Value) -> Result<FlightSpec, RpcError> {
    let s = |i: usize| {
        str_at(v, &[i])
            .map(str::to_owned)
            .ok_or_else(|| unrecognized("selected flight", v.to_string()))
    };
    Ok(FlightSpec {
        from: s(0)?,
        date: date_str(&s(1)?)
            .ok_or_else(|| unrecognized("selected flight", v.to_string()))?,
        to: s(2)?,
        carrier: s(4)?,
        number: s(5)?,
        extra: Vec::new(),
    })
}

fn leg(v: &Value) -> Result<LegSpec, RpcError> {
    let date = str_at(v, &[6])
        .and_then(date_str)
        .ok_or_else(|| unrecognized("leg date", v.to_string()))?;
    let flights = match at(v, &[8]) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => {
            list.iter().map(flight).collect::<Result<_, _>>()?
        }
        Some(other) => {
            return Err(unrecognized("selected flights", other.to_string()));
        }
    };
    Ok(LegSpec {
        date,
        from: place(at(v, &[0]), "leg origin")?,
        to: place(at(v, &[1]), "leg destination")?,
        flights,
        extra: Vec::new(),
    })
}

/// The search array of a request, as a [`Search`] (with the default
/// `tfs` constants, so `tfs::encode` gives a loadable URL).
pub fn search_from_rpc(v: &Value) -> Result<Search, RpcError> {
    let trip = at(v, &[2])
        .and_then(Value::as_u64)
        .and_then(Trip::from_code)
        .ok_or_else(|| {
            unrecognized("trip type", format!("{:?}", at(v, &[2])))
        })?;
    let cabin = Cabin::from_code(small(at(v, &[5]), "cabin")?)
        .ok_or_else(|| unrecognized("cabin", format!("{:?}", at(v, &[5]))))?;
    let p = at(v, &[6]);
    let passengers = Passengers {
        adults: small(p.and_then(|p| at(p, &[0])), "passengers")?,
        children: small(p.and_then(|p| at(p, &[1])), "passengers")?,
        infants_on_lap: small(p.and_then(|p| at(p, &[2])), "passengers")?,
        infants_in_seat: small(p.and_then(|p| at(p, &[3])), "passengers")?,
    };
    let legs = at(v, &[13])
        .and_then(Value::as_array)
        .ok_or_else(|| unrecognized("legs", format!("{:?}", at(v, &[13]))))?
        .iter()
        .map(leg)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Search::new(trip, cabin, passengers, legs))
}

/// The search a `q=` page resolved, from its `AF_dataServiceRequests`
/// script ([`crate::page::DATA_SERVICE_REQUESTS_JS`]): the first
/// request whose second element is a search. On `q=Flights to Rome`,
/// the first leg goes from the user's area to `City("/m/06c62")`.
pub fn parse_data_service_requests(script: &str) -> Result<Search, RpcError> {
    let mut last_err =
        unrecognized("data service requests", "no search request");
    for (_, rest) in script
        .match_indices("request:")
        .map(|(i, _)| script.split_at(i + 8))
    {
        let Ok(request) = first_json(rest) else {
            continue;
        };
        if let Some(search) = at(&request, &[1]).filter(|s| s.is_array()) {
            match search_from_rpc(search) {
                Ok(search) => return Ok(search),
                Err(e) => last_err = e,
            }
        }
    }
    Err(last_err)
}
