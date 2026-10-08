//! Favorite destinations and their deal threshold (DECISIONS.md 0094).

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use common::{route, seeded};
use tm_domain::{PlaceId, Settings};
use tm_store::StoreError;

#[tokio::test]
async fn a_star_marks_every_route_to_the_place() {
    let store = seeded().await;
    let lis = store
        .place_by_code("LIS")
        .await
        .expect("reads")
        .expect("LIS");
    let lis_route = route(&store, "SAO", "LIS").await;
    let mia_route = route(&store, "SAO", "MIA").await;
    // Nothing is a favorite until the user stars it.
    assert!(
        store
            .list_destinations(false)
            .await
            .expect("lists")
            .iter()
            .all(|d| !d.favorite)
    );

    let before = store.config_version().await.expect("reads");
    store
        .set_destination_favorite(lis.id, true)
        .await
        .expect("stars");
    // The daemon reloads on the bumped version (0035).
    assert!(store.config_version().await.expect("reads") > before);

    let destinations = store.list_destinations(true).await.expect("lists");
    let starred: Vec<&str> = destinations
        .iter()
        .filter(|d| d.favorite)
        .map(|d| d.place.code.as_str())
        .collect();
    assert_eq!(starred, ["LIS"]);
    let routes = store.list_routes(true).await.expect("lists");
    let favorite = |id| {
        routes
            .iter()
            .find(|r| r.id == id)
            .is_some_and(|r| r.favorite)
    };
    assert!(favorite(lis_route));
    assert!(!favorite(mia_route));
    let one = store.route(lis_route).await.expect("reads").expect("LIS");
    assert!(one.favorite);
    // Explore sweeps are never favorites.
    assert!(
        store
            .sweep_routes(false)
            .await
            .expect("lists")
            .iter()
            .all(|r| !r.favorite)
    );

    // A removed destination keeps its star for when it comes back.
    store.remove_destination(lis.id).await.expect("removes");
    store.sync_routes().await.expect("syncs");
    let removed = store.list_destinations(false).await.expect("lists");
    assert!(
        removed
            .iter()
            .any(|d| d.place.code == "LIS" && !d.active && d.favorite)
    );

    store
        .set_destination_favorite(lis.id, false)
        .await
        .expect("unstars");
    assert!(
        !store
            .route(lis_route)
            .await
            .expect("reads")
            .expect("LIS")
            .favorite
    );
    assert!(matches!(
        store.set_destination_favorite(PlaceId(999_999), true).await,
        Err(StoreError::NotFound { .. })
    ));
}

#[tokio::test]
async fn the_favorite_threshold_is_a_setting() {
    let store = seeded().await;
    assert_eq!(
        store
            .settings()
            .await
            .expect("reads")
            .favorite_deal_threshold,
        0.10
    );
    let settings = Settings {
        favorite_deal_threshold: 0.12,
        ..Settings::default()
    };
    store.set_settings(&settings).await.expect("saves");
    assert_eq!(
        store
            .settings()
            .await
            .expect("reads")
            .favorite_deal_threshold,
        0.12
    );
    let bad = Settings {
        favorite_deal_threshold: -0.1,
        ..Settings::default()
    };
    assert!(store.set_settings(&bad).await.is_err());
}
