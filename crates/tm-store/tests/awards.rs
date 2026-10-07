//! Award prices (DECISIONS.md 0083): observations with their fare
//! before taxes, brand and seats; the program's implied rate; one-way
//! award deals; and each program's own settings.

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use common::{at, date, route, seeded};
use tm_domain::{
    AwardDates,
    AwardDetail,
    Cabin,
    Currency,
    Deal,
    DealReason,
    DealStatus,
    FamilyId,
    FamilyKind,
    Leg,
    Money,
    Observation,
    ProgramSettings,
    QueryId,
    QueryOutcome,
    QueryReason,
    Seats,
    Series,
    Source,
};
use tm_store::{DealFilter, Store};

fn brl(minor: i64) -> Money {
    Money::new(minor, Currency::brl())
}

async fn latam_outbound(store: &Store, cabin: Cabin) -> FamilyId {
    let lis = route(store, "SAO", "LIS").await;
    store
        .list_families(false)
        .await
        .expect("lists")
        .into_iter()
        .find(|f| {
            f.key.route == lis
                && f.key.cabin == cabin
                && f.key.source
                    == Source::Miles {
                        program: "latam_pass".into(),
                    }
                && f.key.kind == FamilyKind::Leg { leg: Leg::Outbound }
        })
        .expect("LATAM Pass outbound family")
        .id
}

/// One brand of the GRU → LIS fixture (2026-11-10, LA8146).
fn award(
    query: QueryId,
    family: FamilyId,
    miles: i64,
    fare: i64,
    brand: &str,
) -> Observation {
    Observation {
        id: None,
        query,
        family,
        observed_at: at(7, 15),
        departure: date(11, 10),
        return_date: None,
        price: Some(brl(6861)),
        miles: Some(miles),
        carrier: Some("LATAM".into()),
        stops: Some(0),
        duration_minutes: Some(580),
        seller: None,
        flights: Some("LA8146 17:40".into()),
        award: Some(AwardDetail {
            fare: Some(brl(fare)),
            brand: Some(brand.into()),
            fare_basis: Some("AKTSLZ6E".into()),
            seats: Some(Seats {
                count: 9,
                low: false,
            }),
        }),
    }
}

#[tokio::test]
async fn award_observations_keep_their_fare_brand_and_seats() {
    let store = seeded().await;
    let family = latam_outbound(&store, Cabin::Economy).await;
    let q = store
        .start_query(family, &QueryReason::Manual, at(7, 15))
        .await
        .expect("starts");
    let mut few = award(q, family, 71_786, 206_743, "STANDARD");
    if let Some(a) = few.award.as_mut() {
        a.seats = Some(Seats {
            count: 2,
            low: true,
        });
    }
    store
        .insert_observations(&[award(q, family, 63_848, 183_882, "LIGHT"), few])
        .await
        .expect("inserts");
    store
        .finish_query(q, &QueryOutcome::Ok, 1, at(7, 15))
        .await
        .expect("finishes");

    let cells = store.latest_cells(family).await.expect("reads");
    assert_eq!(cells.len(), 2);
    let light = cells
        .iter()
        .find(|c| c.miles == Some(63_848))
        .expect("LIGHT");
    assert_eq!(light.flights.as_deref(), Some("LA8146 17:40"));
    let detail = light.award.as_ref().expect("award detail");
    assert_eq!(detail.fare, Some(brl(183_882)));
    assert_eq!(detail.brand.as_deref(), Some("LIGHT"));
    assert_eq!(detail.fare_basis.as_deref(), Some("AKTSLZ6E"));
    assert_eq!(
        light
            .award_price()
            .map(|a| a.text(tm_domain::MilesUnit::Miles)),
        Some("63,848 mi + R$69".to_owned())
    );
    let standard = cells
        .iter()
        .find(|c| c.miles == Some(71_786))
        .expect("STANDARD");
    assert_eq!(
        standard.award.as_ref().and_then(|a| a.seats),
        Some(Seats {
            count: 2,
            low: true
        })
    );

    // The program's rate: (1,838.82 / 63,848 + 2,067.43 / 71,786) / 2
    // × 1,000, both about R$28.80.
    let rates = store
        .award_rates("latam_pass", at(1, 0))
        .await
        .expect("rates");
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].awards, 2);
    assert!((rates[0].brl_per_1000 - 28.80).abs() < 0.01, "{rates:?}");
    assert!(
        store
            .award_rates("smiles", at(1, 0))
            .await
            .expect("rates")
            .is_empty()
    );
    assert!(
        store
            .award_rates("latam_pass", at(8, 0))
            .await
            .expect("rates")
            .is_empty()
    );
}

fn one_way(route: tm_domain::RouteId, pos: tm_domain::PosId) -> Deal {
    Deal {
        id: None,
        route,
        cabin: Cabin::Business,
        pos,
        series: Series::Award {
            program: "latam_pass".into(),
        },
        leg: Some(Leg::Outbound),
        departure: date(11, 10),
        return_date: None,
        price: brl(6861),
        effective_brl: brl(6861),
        baseline: None,
        miles: Some(263_848),
        baseline_miles: Some(338_000),
        brand: Some("PREMIUM BUSINESS STANDARD".into()),
        seats: Some(Seats {
            count: 2,
            low: false,
        }),
        cash: Some(Money::brl_whole(7_667)),
        reason: DealReason::BelowMedian,
        change: Some(-0.22),
        estimated: false,
        status: DealStatus::Candidate,
        found_at: at(7, 15),
        confirmed_at: None,
        seen: false,
        carrier: Some("LA8148 nonstop".into()),
        seller: None,
        sellers: Vec::new(),
        url: None,
    }
}

#[tokio::test]
async fn one_way_award_deals_have_no_return_date() {
    let store = seeded().await;
    let lis = route(&store, "SAO", "LIS").await;
    let home = store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("home");
    let deal = one_way(lis, home.id);
    let id = store.insert_deal(&deal).await.expect("inserts");
    let back = store.deal(id).await.expect("reads").expect("exists");
    assert_eq!(
        back,
        Deal {
            id: Some(id),
            ..deal.clone()
        }
    );
    assert_eq!(
        back.award_price()
            .map(|a| a.text(tm_domain::MilesUnit::Miles)),
        Some("263,848 mi + R$69".to_owned())
    );
    let listed = store
        .list_deals(&DealFilter::default())
        .await
        .expect("lists");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].series, deal.series);

    // A cash deal is a round trip: it needs its return date.
    let cash = Deal {
        series: Series::Cash,
        leg: None,
        miles: None,
        baseline_miles: None,
        ..deal.clone()
    };
    assert!(store.insert_deal(&cash).await.is_err());
    // An award deal needs its miles.
    let no_miles = Deal {
        miles: None,
        ..deal
    };
    assert!(store.insert_deal(&no_miles).await.is_err());
}

#[tokio::test]
async fn programs_keep_their_own_settings() {
    let store = seeded().await;
    let programs = store.list_miles_programs().await.expect("lists");
    let latam = programs
        .iter()
        .find(|p| p.program == "latam_pass")
        .expect("seeded");
    // Nothing set until the user sets it (0083).
    assert_eq!(
        latam.settings(),
        ProgramSettings {
            active: true,
            value_per_1000: None,
            buy_per_1000: None,
            dates: AwardDates::CheapTripsAndWeekly,
        }
    );

    assert_eq!(latam.session_until, None);
    store
        .set_miles_session_until("latam_pass", Some(at(12, 0)))
        .await
        .expect("records");
    let before = store.config_version().await.expect("version");
    let set = ProgramSettings {
        active: true,
        value_per_1000: Some(brl(2000)),
        buy_per_1000: Some(brl(1750)),
        dates: AwardDates::CheapTrips,
    };
    store
        .save_miles_settings("latam_pass", &set)
        .await
        .expect("saves");
    assert_eq!(
        store.config_version().await.expect("version"),
        before,
        "values do not change the families"
    );
    let read = |programs: Vec<tm_store::MilesProgram>| {
        programs
            .into_iter()
            .find(|p| p.program == "latam_pass")
            .map(|p| p.settings())
    };
    assert_eq!(
        read(store.list_miles_programs().await.expect("lists")),
        Some(set.clone())
    );
    let latam = store
        .list_miles_programs()
        .await
        .expect("lists")
        .into_iter()
        .find(|p| p.program == "latam_pass")
        .expect("seeded");
    assert_eq!(latam.session_until, Some(at(12, 0)));

    let paused = ProgramSettings {
        active: false,
        ..set
    };
    store
        .save_miles_settings("latam_pass", &paused)
        .await
        .expect("saves");
    assert!(store.config_version().await.expect("version") > before);
    assert_eq!(
        read(store.list_miles_programs().await.expect("lists")),
        Some(paused.clone())
    );

    let usd = ProgramSettings {
        buy_per_1000: Some(Money::from_whole(5, Currency::new("USD"))),
        ..paused.clone()
    };
    assert!(store.save_miles_settings("latam_pass", &usd).await.is_err());
    assert!(store.save_miles_settings("nope", &paused).await.is_err());
}

#[tokio::test]
async fn the_unusual_rate_is_a_setting() {
    let store = seeded().await;
    let mut settings = store.settings().await.expect("reads");
    assert_eq!(settings.unusual_rate, 0.15);
    settings.unusual_rate = 0.25;
    store.set_settings(&settings).await.expect("saves");
    assert_eq!(store.settings().await.expect("reads").unusual_rate, 0.25);
}
