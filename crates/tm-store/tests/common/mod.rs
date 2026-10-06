//! Shared helpers for the store's integration tests.

#![allow(dead_code)]

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use tm_domain::{
    Cabin,
    FamilyId,
    FamilyKind,
    Leg,
    Money,
    Observation,
    QueryId,
    QueryOutcome,
    QueryReason,
    RouteId,
    Source,
};
use tm_store::{FamilyRecord, Store};

pub async fn seeded() -> Store {
    let store = Store::open_in_memory().await.expect("opens");
    assert!(store.seed_defaults().await.expect("seeds"));
    store
}

pub fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0)
        .single()
        .expect("valid time")
}

pub fn date(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).expect("valid date")
}

pub async fn route(store: &Store, from: &str, to: &str) -> RouteId {
    store
        .route_between(from, to)
        .await
        .expect("reads")
        .expect("route exists")
}

/// The Google Flights outbound economy family of a route under the
/// home point of sale.
pub async fn outbound(store: &Store, route: RouteId) -> FamilyRecord {
    let home = store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("home");
    store
        .list_families(false)
        .await
        .expect("lists")
        .into_iter()
        .find(|f| {
            f.key.route == route
                && f.key.cabin == Cabin::Economy
                && f.key.source == Source::GoogleFlights
                && f.key.pos == home.id
                && f.key.kind == FamilyKind::Leg { leg: Leg::Outbound }
        })
        .expect("family exists")
}

pub fn cell(
    query: QueryId,
    family: FamilyId,
    observed_at: DateTime<Utc>,
    departure: NaiveDate,
    price: Option<i64>,
) -> Observation {
    Observation {
        id: None,
        query,
        family,
        observed_at,
        departure,
        return_date: None,
        price: price.map(Money::brl_whole),
        miles: None,
        carrier: Some("LA".into()),
        stops: Some(0),
        duration_minutes: Some(600),
        seller: None,
    }
}

/// Runs a query that records `prices` (one per day from Nov 1) and ends
/// with `outcome`.
pub async fn run_query(
    store: &Store,
    family: FamilyId,
    when: DateTime<Utc>,
    prices: &[i64],
    outcome: QueryOutcome,
) -> QueryId {
    let q = store
        .start_query(family, &QueryReason::Random, when)
        .await
        .expect("starts");
    let cells: Vec<_> = prices
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let day = u32::try_from(i).expect("small") + 1;
            cell(q, family, when, date(11, day), Some(*p))
        })
        .collect();
    store.insert_observations(&cells).await.expect("inserts");
    store
        .finish_query(q, &outcome, 1, when)
        .await
        .expect("finishes");
    q
}
