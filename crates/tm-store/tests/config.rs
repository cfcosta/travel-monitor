//! Seeding, configuration, routes and families.

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use common::{route, seeded};
use tm_domain::{
    Cabin,
    CountryCode,
    Currency,
    FamilyKind,
    PlaceKind,
    Settings,
    Source,
};
use tm_store::{LoginState, NewPlace, RouteTripState, Store, StoreError};

#[tokio::test]
async fn seeds_once() {
    let store = Store::open_in_memory().await.expect("opens");
    assert!(store.seed_defaults().await.expect("seeds"));
    let families = store.list_families(false).await.expect("lists").len();
    let version = store.config_version().await.expect("reads");
    assert!(!store.seed_defaults().await.expect("seeds again"));
    assert_eq!(
        store.list_families(false).await.expect("lists").len(),
        families
    );
    assert_eq!(store.config_version().await.expect("reads"), version);

    let origins = store.list_origins(true).await.expect("lists");
    assert_eq!(origins.len(), 1);
    let sao = &origins[0].place;
    assert_eq!(sao.code, "SAO");
    assert_eq!(sao.google_id(), Some("/m/022pfm"));
    assert_eq!(sao.country.as_str(), "BR");

    let destinations = store.list_destinations(true).await.expect("lists");
    assert_eq!(destinations.len(), 116);
    let lis = store
        .place_by_code("lis")
        .await
        .expect("reads")
        .expect("LIS");
    assert_eq!(lis.kind, PlaceKind::Airport);
    assert_eq!(lis.country.as_str(), "PT");
    let tyo = store
        .place_by_code("TYO")
        .await
        .expect("reads")
        .expect("TYO");
    assert_eq!(tyo.kind, PlaceKind::City { freebase_id: None });

    let poses = store.list_points_of_sale(true).await.expect("lists");
    let countries: Vec<_> = poses.iter().map(|p| p.country.as_str()).collect();
    assert_eq!(countries, ["BR", "US", "PT", "IN"]);
    assert!(poses[0].is_home);
    assert!(poses.iter().all(|p| p.currency == Currency::brl()));

    assert_eq!(store.settings().await.expect("reads"), Settings::default());

    let airlines = store.list_airline_sources().await.expect("lists");
    assert_eq!(airlines.len(), 11);
    let tap = airlines.iter().find(|a| a.code == "TP").expect("TAP");
    assert!(tap.destinations.contains(&lis.id));

    let miles = store.list_miles_programs().await.expect("lists");
    let programs: Vec<_> = miles.iter().map(|m| m.program.as_str()).collect();
    assert_eq!(programs, ["azul_fidelidade", "latam_pass", "smiles"]);
    assert!(miles.iter().all(|m| m.login_state == LoginState::LoggedOut));

    assert_eq!(store.list_routes(true).await.expect("lists").len(), 116);
    assert_eq!(store.sweep_routes(true).await.expect("lists").len(), 1);
}

#[tokio::test]
async fn seeded_families_follow_the_rules() {
    let store = seeded().await;
    let families = store.list_families(true).await.expect("lists");

    // Explore: SAO × {economy, business} × 4 points of sale.
    let explore: Vec<_> = families
        .iter()
        .filter(|f| f.key.kind == FamilyKind::Explore)
        .collect();
    assert_eq!(explore.len(), 8);
    let sweep = store.sweep_routes(true).await.expect("lists")[0].id;
    assert!(explore.iter().all(|f| f.key.route == sweep));
    assert!(explore.iter().all(|f| f.weight == 1.0));

    // SSA, domestic: economy × (4 GF + LA, AD, G3 + 3 miles) × 3 kinds.
    let ssa = route(&store, "SAO", "SSA").await;
    let n = families.iter().filter(|f| f.key.route == ssa).count();
    assert_eq!(n, 10 * 3);
    assert!(
        families
            .iter()
            .filter(|f| f.key.route == ssa)
            .all(|f| f.key.cabin == Cabin::Economy)
    );

    // LIS: 3 cabins × (4 GF + LA, AD, TP, IB, AF + 3 miles) × 3 kinds.
    let lis = route(&store, "SAO", "LIS").await;
    let n = families.iter().filter(|f| f.key.route == lis).count();
    assert_eq!(n, 3 * 12 * 3);

    let home = store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("BR");
    for f in &families {
        if f.key.source != Source::GoogleFlights {
            assert_eq!(f.key.pos, home.id, "{:?}", f.key);
        }
        assert_eq!(f.site, f.key.source.site());
        if let FamilyKind::RoundTrip { days } = f.key.kind {
            assert_eq!(days, 7);
        }
    }
}

#[tokio::test]
async fn places_are_edited() {
    let store = Store::open_in_memory().await.expect("opens");
    let vcp = store
        .add_origin(&NewPlace::airport("vcp", "Campinas", "br", -23.0, -47.13))
        .await
        .expect("adds");
    assert_eq!(vcp.code, "VCP");
    assert_eq!(vcp.country.as_str(), "BR");

    let mut changed = vcp.clone();
    changed.name = "Viracopos".into();
    store.update_place(&changed).await.expect("updates");
    let read = store.place(vcp.id).await.expect("reads").expect("exists");
    assert_eq!(read.name, "Viracopos");

    // Airports have no Freebase ID.
    assert!(matches!(
        store.set_freebase_id(vcp.id, "/m/x").await,
        Err(StoreError::NotFound { .. })
    ));
    let rio = store
        .add_destination(
            &NewPlace::city("RIO", "Rio", "BR", None, -22.9, -43.2),
            2.5,
        )
        .await
        .expect("adds");
    store
        .set_freebase_id(rio.id, "/m/06gmr")
        .await
        .expect("sets");
    let rio = store.place(rio.id).await.expect("reads").expect("exists");
    assert_eq!(rio.google_id(), Some("/m/06gmr"));

    // Re-adding with an unknown ID keeps the learned one.
    store
        .add_destination(
            &NewPlace::city("RIO", "Rio de Janeiro", "BR", None, -22.9, -43.2),
            1.0,
        )
        .await
        .expect("re-adds");
    let rio = store.place(rio.id).await.expect("reads").expect("exists");
    assert_eq!(rio.google_id(), Some("/m/06gmr"));
    assert_eq!(rio.name, "Rio de Janeiro");

    store
        .set_destination_weight(rio.id, 3.0)
        .await
        .expect("sets");
    let d = store.list_destinations(true).await.expect("lists");
    assert_eq!(d[0].weight, 3.0);

    assert!(matches!(
        store
            .add_origin(&NewPlace::airport("", "x", "BR", 0.0, 0.0))
            .await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store
            .add_origin(&NewPlace::airport("XXX", "x", "BR", 91.0, 0.0))
            .await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.set_destination_weight(rio.id, -1.0).await,
        Err(StoreError::Invalid(_))
    ));
}

#[tokio::test]
async fn routes_deactivate_and_come_back() {
    let store = seeded().await;
    let lis_route = route(&store, "SAO", "LIS").await;
    let lis = store
        .place_by_code("LIS")
        .await
        .expect("reads")
        .expect("LIS");
    let before = store.config_version().await.expect("reads");

    store.remove_destination(lis.id).await.expect("removes");
    assert!(store.config_version().await.expect("reads") > before);
    assert_eq!(store.sync_routes().await.expect("syncs"), 115);
    let r = store.route(lis_route).await.expect("reads").expect("kept");
    assert!(!r.active);
    assert_eq!(store.list_routes(false).await.expect("lists").len(), 116);

    store
        .add_destination(
            &NewPlace::airport("LIS", "Lisbon", "PT", 38.72, -9.14),
            1.0,
        )
        .await
        .expect("re-adds");
    assert_eq!(store.sync_routes().await.expect("syncs"), 116);
    assert_eq!(route(&store, "SAO", "LIS").await, lis_route);
    assert!(
        store
            .route(lis_route)
            .await
            .expect("reads")
            .expect("kept")
            .active
    );

    // A second origin gets routes to every destination and a sweep.
    let vcp = store
        .add_origin(&NewPlace::airport("VCP", "Campinas", "BR", -23.0, -47.1))
        .await
        .expect("adds");
    assert_eq!(store.sync_routes().await.expect("syncs"), 232);
    let sweep = store
        .sweep_route(vcp.id)
        .await
        .expect("reads")
        .expect("sweep");
    let sweep = store.route(sweep).await.expect("reads").expect("exists");
    assert_eq!(sweep.origin, sweep.destination);
    assert_eq!(store.sweep_routes(true).await.expect("lists").len(), 2);

    store.remove_origin(vcp.id).await.expect("removes");
    assert_eq!(store.sync_routes().await.expect("syncs"), 116);
    assert_eq!(store.sweep_routes(true).await.expect("lists").len(), 1);
    assert_eq!(store.sweep_routes(false).await.expect("lists").len(), 2);
}

#[tokio::test]
async fn families_deactivate_and_come_back() {
    let store = seeded().await;
    let settings = store.settings().await.expect("reads");
    let all = store.list_families(false).await.expect("lists");
    let lis_route = route(&store, "SAO", "LIS").await;
    let lis_ids: Vec<_> = all
        .iter()
        .filter(|f| f.key.route == lis_route)
        .map(|f| f.id)
        .collect();
    let active = store.list_families(true).await.expect("lists").len();

    let lis = store
        .place_by_code("LIS")
        .await
        .expect("reads")
        .expect("LIS");
    store.remove_destination(lis.id).await.expect("removes");
    let sync = store.sync_families(&settings).await.expect("syncs");
    assert_eq!(sync.created, 0);
    assert_eq!(sync.active, active - lis_ids.len());
    for id in &lis_ids {
        let f = store.family(*id).await.expect("reads").expect("kept");
        assert!(!f.active);
    }

    store
        .add_destination(
            &NewPlace::airport("LIS", "Lisbon", "PT", 38.72, -9.14),
            1.0,
        )
        .await
        .expect("re-adds");
    let sync = store.sync_families(&settings).await.expect("syncs");
    assert_eq!(sync.created, 0);
    assert_eq!(sync.active, active);
    for id in &lis_ids {
        assert!(
            store
                .family(*id)
                .await
                .expect("reads")
                .expect("kept")
                .active
        );
    }

    // An additive route loses its round-trip families.
    let mut trip = store
        .route_trip_state(lis_route)
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(trip, RouteTripState::default());
    trip.additive = true;
    trip.additive_streak = 5;
    store
        .save_route_trip_state(lis_route, &trip)
        .await
        .expect("saves");
    store.sync_families(&settings).await.expect("syncs");
    let lis_active: Vec<_> = store
        .list_families(true)
        .await
        .expect("lists")
        .into_iter()
        .filter(|f| f.key.route == lis_route)
        .collect();
    assert_eq!(lis_active.len(), 3 * 12 * 2);
    assert!(
        lis_active
            .iter()
            .all(|f| !matches!(f.key.kind, FamilyKind::RoundTrip { .. }))
    );
    assert!(lis_active.iter().all(|f| f.state.additive));
    assert!(lis_active.iter().all(|f| f.state.additive_streak == 5));

    // Changing the round-trip length makes new families.
    let mut longer = settings.clone();
    longer.trips.round_trip_days = 8;
    store.set_settings(&longer).await.expect("saves");
    let sync = store.sync_families(&longer).await.expect("syncs");
    assert!(sync.created > 0);
    assert!(store.list_families(true).await.expect("lists").iter().all(|f| {
        !matches!(f.key.kind, FamilyKind::RoundTrip { days } if days != 8)
    }));

    // A point of sale switched off takes its Google Flights families.
    let us = store
        .list_points_of_sale(true)
        .await
        .expect("lists")
        .into_iter()
        .find(|p| p.country.as_str() == "US")
        .expect("US");
    store
        .set_point_of_sale_active(us.id, false)
        .await
        .expect("sets");
    store.sync_families(&longer).await.expect("syncs");
    assert!(
        store
            .list_families(true)
            .await
            .expect("lists")
            .iter()
            .all(|f| f.key.pos != us.id)
    );
}

#[tokio::test]
async fn family_state_round_trips() {
    let store = seeded().await;
    let ssa = route(&store, "SAO", "SSA").await;
    let family = common::outbound(&store, ssa).await;
    assert_eq!(family.state, tm_store::FamilyStateRecord::default());
    assert_eq!(
        store.family_id(&family.key).await.expect("reads"),
        Some(family.id)
    );

    let mut state = family.state.clone();
    state.chg_a = 3.5;
    state.chg_b = 70.0;
    state.deal_a = 2.0;
    state.deal_b = 4.0;
    state.prec_a = 1.5;
    state.prec_b = 2.5;
    state.n_hints = 2;
    state.last_check = Some(common::at(6, 12));
    state.last_min_price_minor = Some(91_600);
    state.checks = 9;
    state.cost_loads = 6.5;
    // Route-level fields are not saved through the family.
    state.additive = true;
    store
        .save_family_state(family.id, &state)
        .await
        .expect("saves");
    let read = store
        .family(family.id)
        .await
        .expect("reads")
        .expect("exists");
    let mut expected = state.clone();
    expected.additive = false;
    assert_eq!(read.state, expected);

    store
        .save_route_trip_state(
            ssa,
            &RouteTripState {
                rt_discount_ewma: 0.12,
                rt_discount_n: 3,
                additive_streak: 1,
                additive: false,
            },
        )
        .await
        .expect("saves");
    let read = store
        .family(family.id)
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(read.state.rt_discount_ewma, 0.12);
    assert_eq!(read.state.rt_discount_n, 3);
    assert_eq!(read.state.route_trip().additive_streak, 1);
}

#[tokio::test]
async fn settings_are_validated() {
    let store = Store::open_in_memory().await.expect("opens");
    assert_eq!(store.settings().await.expect("reads"), Settings::default());
    let mut s = Settings::default();
    s.passengers.adults = 2;
    s.passengers.children = 1;
    s.iof = 0.038;
    s.language = "pt-BR".into();
    store.set_settings(&s).await.expect("saves");
    assert_eq!(store.settings().await.expect("reads"), s);

    let mut bad = s.clone();
    bad.passengers.adults = 0;
    assert!(matches!(
        store.set_settings(&bad).await,
        Err(StoreError::Settings(_))
    ));
    assert_eq!(store.settings().await.expect("reads"), s);
}

#[tokio::test]
async fn sources_and_points_of_sale_are_edited() {
    let store = seeded().await;
    let before = store.config_version().await.expect("reads");

    store
        .upsert_airline_source("ux", "Air Europa", true)
        .await
        .expect("adds");
    let mad = store
        .place_by_code("MAD")
        .await
        .expect("reads")
        .expect("MAD");
    store
        .set_airline_destinations("UX", &[mad.id])
        .await
        .expect("sets");
    let ux = store
        .list_airline_sources()
        .await
        .expect("lists")
        .into_iter()
        .find(|a| a.code == "UX")
        .expect("UX");
    assert_eq!(ux.destinations, vec![mad.id]);
    assert!(matches!(
        store.set_airline_destinations("ZZ", &[mad.id]).await,
        Err(StoreError::NotFound { .. })
    ));

    store.set_airline_active("UX", false).await.expect("sets");
    store
        .set_miles_login("smiles", LoginState::LoggedIn, common::at(6, 9))
        .await
        .expect("sets");
    let smiles = store
        .list_miles_programs()
        .await
        .expect("lists")
        .into_iter()
        .find(|m| m.program == "smiles")
        .expect("smiles");
    assert_eq!(smiles.login_state, LoginState::LoggedIn);
    assert_eq!(smiles.last_checked, Some(common::at(6, 9)));
    store
        .upsert_miles_program("smiles", "Smiles", false)
        .await
        .expect("updates");

    let settings = store.settings().await.expect("reads");
    store.sync_families(&settings).await.expect("syncs");
    let active = store.list_families(true).await.expect("lists");
    assert!(
        active
            .iter()
            .all(|f| f.key.source != Source::Airline { code: "UX".into() })
    );
    assert!(active.iter().all(|f| f.key.source
        != Source::Miles {
            program: "smiles".into()
        }));

    // A new home market takes the flag from BR.
    let ar = store
        .add_point_of_sale(&CountryCode::new("ar"), &Currency::new("ars"), true)
        .await
        .expect("adds");
    assert!(ar.is_home);
    assert_eq!(ar.currency.as_str(), "ARS");
    let home = store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("home");
    assert_eq!(home.id, ar.id);
    assert_eq!(
        store
            .list_points_of_sale(false)
            .await
            .expect("lists")
            .iter()
            .filter(|p| p.is_home)
            .count(),
        1
    );
    assert!(store.config_version().await.expect("reads") > before);
}

#[tokio::test]
async fn file_database_persists() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("nested.db");
    {
        let store = Store::open(&path).await.expect("creates");
        assert!(store.seed_defaults().await.expect("seeds"));
        store.close().await;
    }
    let store = Store::open(&path).await.expect("reopens");
    assert!(!store.seed_defaults().await.expect("already seeded"));
    assert_eq!(
        store.list_destinations(true).await.expect("lists").len(),
        116
    );
}
