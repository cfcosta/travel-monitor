//! Destinations the Explore sweep priced outside the watchlist, and
//! their drops (DECISIONS.md 0087).

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use common::{at, seeded};
use tm_domain::{Cabin, ExploreHint, Money, PlaceKind, QueryId, QueryReason};
use tm_store::{NewPlace, Store};

/// An Explore query from SAO, logged so hints can name it.
async fn sweep(store: &Store, day: u32) -> QueryId {
    let sao = store
        .place_by_code("SAO")
        .await
        .expect("reads")
        .expect("SAO");
    let route = store
        .sweep_route(sao.id)
        .await
        .expect("reads")
        .expect("sweep");
    let family = store
        .list_families(true)
        .await
        .expect("lists")
        .into_iter()
        .find(|f| f.key.route == route)
        .expect("explore family");
    store
        .start_query(
            family.id,
            &QueryReason::Scheduled {
                score: 1.0,
                runner_up: None,
            },
            at(day, 6),
        )
        .await
        .expect("starts")
}

async fn hint(
    store: &Store,
    query: QueryId,
    day: u32,
    freebase: &str,
    airport: &str,
    price: i64,
) -> ExploreHint {
    let sao = store
        .place_by_code("SAO")
        .await
        .expect("reads")
        .expect("SAO");
    let home = store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("home");
    ExploreHint {
        query,
        origin: sao.id,
        cabin: Cabin::Economy,
        pos: home.id,
        observed_at: at(day, 6),
        destination_freebase_id: freebase.into(),
        destination_airport: airport.into(),
        price: Money::brl_whole(price),
        departure: Some(common::date(11, 3)),
        return_date: Some(common::date(11, 10)),
        stops: Some(1),
        duration_minutes: Some(900),
        carrier: Some("Garuda Indonesia".into()),
        destination_name: Some("Jakarta".into()),
        destination_country: Some("Indonesia".into()),
        destination_coordinates: Some((-6.2, 106.85)),
    }
}

#[tokio::test]
async fn only_unwatched_destinations_are_finds() {
    let store = seeded().await;
    let q = sweep(&store, 6).await;
    let hints = vec![
        // Watched airports and cities.
        hint(&store, q, 6, "/m/04llb", "LIS", 3_600).await,
        hint(&store, q, 6, "/m/0ply0", "SFB", 2_600).await,
        // Explore filing a watched airport under another ID, and that
        // ID with another airport.
        hint(&store, q, 6, "/m/0rj0z", "MIA", 2_400).await,
        hint(&store, q, 6, "/m/0rj0z", "OPF", 2_300).await,
        // Not watched.
        hint(&store, q, 6, "/m/044rv", "CGK", 6_600).await,
        hint(&store, q, 6, "/m/062qg", "PER", 9_500).await,
    ];
    assert_eq!(store.latest_explore_hint_at().await.expect("reads"), None);
    store.insert_explore_hints(&hints).await.expect("inserts");
    assert_eq!(
        store.latest_explore_hint_at().await.expect("reads"),
        Some(at(6, 6))
    );
    let found = store
        .unwatched_explore_hints(at(1, 0))
        .await
        .expect("reads");
    let airports: Vec<&str> = found
        .iter()
        .map(|h| h.destination_airport.as_str())
        .collect();
    assert_eq!(airports, ["CGK", "PER"]);
    assert_eq!(found[0], hints[4]);

    // Watching Jakarta takes it out; so does a city with its ID.
    store
        .add_destination(
            &NewPlace::airport("CGK", "Jakarta", "ID", -6.2, 106.85),
            1.0,
        )
        .await
        .expect("adds");
    store
        .add_destination(
            &NewPlace {
                code: "PER".into(),
                name: "Perth".into(),
                country: tm_domain::CountryCode::new("AU"),
                kind: PlaceKind::City {
                    freebase_id: Some("/m/062qg".into()),
                },
                lat: -31.95,
                lon: 115.86,
            },
            1.0,
        )
        .await
        .expect("adds");
    assert!(
        store
            .unwatched_explore_hints(at(1, 0))
            .await
            .expect("reads")
            .is_empty()
    );
    // A removed destination stays a choice, not a find.
    let cgk = store
        .place_by_code("CGK")
        .await
        .expect("reads")
        .expect("CGK");
    store.remove_destination(cgk.id).await.expect("removes");
    assert!(
        store
            .unwatched_explore_hints(at(1, 0))
            .await
            .expect("reads")
            .is_empty()
    );
}

#[tokio::test]
async fn a_drop_opens_once_and_is_notified_once() {
    let store = seeded().await;
    let q = sweep(&store, 6).await;
    let h = hint(&store, q, 6, "/m/044rv", "CGK", 5_000).await;
    let usual = Money::brl_whole(7_000);
    let id = store
        .start_explore_drop(&h, &usual, at(6, 7))
        .await
        .expect("starts")
        .expect("a new drop");
    // A second one for the same destination waits for the first to end.
    assert_eq!(
        store
            .start_explore_drop(&h, &usual, at(6, 8))
            .await
            .expect("starts"),
        None
    );
    let open = store.open_explore_drops(h.origin).await.expect("reads");
    assert_eq!(open.len(), 1);
    let drop = &open[0];
    assert_eq!(drop.id, id);
    assert_eq!(drop.price, Money::brl_whole(5_000));
    assert_eq!(drop.usual, usual);
    assert_eq!(drop.destination_name.as_deref(), Some("Jakarta"));
    assert_eq!(drop.stops, Some(1));
    assert!(drop.change().is_some_and(|c| (c + 2.0 / 7.0).abs() < 1e-9));

    assert_eq!(
        store.unnotified_explore_drops().await.expect("reads").len(),
        1
    );
    store
        .mark_explore_drop_notified(id, at(6, 7))
        .await
        .expect("marks");
    assert!(
        store
            .unnotified_explore_drops()
            .await
            .expect("reads")
            .is_empty()
    );

    store.end_explore_drop(id, at(7, 6)).await.expect("ends");
    assert!(
        store
            .open_explore_drops(h.origin)
            .await
            .expect("reads")
            .is_empty()
    );
    // The next drop is a new one, to be notified again.
    let again = store
        .start_explore_drop(&h, &usual, at(8, 6))
        .await
        .expect("starts");
    assert!(again.is_some_and(|a| a != id));
    assert_eq!(
        store.unnotified_explore_drops().await.expect("reads").len(),
        1
    );
}
