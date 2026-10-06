//! Per-site budgets and status (DECISIONS.md 0060, 0062), and the
//! migration of the single budget row they replace.

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use std::path::Path;

use common::{at, date, outbound, route, seeded};
use sqlx::{
    Connection as _,
    SqliteConnection,
    migrate::Migrator,
    sqlite::SqliteConnectOptions,
};
use tm_domain::{GOOGLE_FLIGHTS, QueryReason, SiteLimits};
use tm_store::{CheckInProgress, SiteBudget, SiteStatus, Store};

/// A database file at the schema before 0003, with `setup` run on it.
async fn before_site_budgets(path: &Path, setup: String) {
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
    .expect("connects");
    let migrations =
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"));
    Migrator::new(migrations)
        .await
        .expect("reads the migrations")
        .run_to(2, &mut conn)
        .await
        .expect("migrates to 0002");
    sqlx::raw_sql(sqlx::AssertSqlSafe(setup))
        .execute(&mut conn)
        .await
        .expect("sets up");
    conn.close().await.expect("closes");
}

const OLD_BUDGET: &str = "INSERT INTO budget
    (id, daily_budget, loads_today, day, last_ramp_at)
    VALUES (1, 5280, 1234, '2026-10-06', '2026-10-05T12:00:00+00:00');";

const OLD_SETTINGS: &str = "INSERT INTO settings (id, adults, children,
    infants_in_seat, infants_on_lap, trip_min_days, trip_max_days,
    round_trip_days, horizon_days, iof, card_spread, daily_budget,
    max_daily_budget, max_interval_hours, min_interval_hours,
    random_share, deal_threshold, min_history_weeks,
    additive_checks_to_stop, language)
    VALUES (1, 1, 0, 0, 0, 5, 10, 7, 180, 0.035, 0.01, 4800, 12000, 168,
    6, 0.1, 0.2, 4, 5, 'en');";

/// The one budget row becomes Google Flights' (0060): its count, day,
/// ramped budget and ramp clock are kept, its start and maximum come
/// from the settings, and the old table goes.
#[tokio::test]
async fn the_old_budget_row_becomes_google_flights() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("tm.db");
    before_site_budgets(&path, format!("{OLD_SETTINGS}{OLD_BUDGET}")).await;

    let store = Store::open(&path).await.expect("opens and migrates");
    let budgets = store.site_budgets().await.expect("reads");
    assert_eq!(
        budgets,
        vec![SiteBudget {
            site: GOOGLE_FLIGHTS.into(),
            start_budget: 4800,
            daily_budget: 5280,
            max_daily_budget: 12000,
            loads_today: 1234,
            day: date(10, 6),
            last_ramp_at: Some(at(5, 12)),
        }]
    );
    // Other sites start from their own defaults, with nothing counted.
    let tap = store
        .site_budget_or_default("airline:TP", date(10, 6))
        .await
        .expect("reads");
    assert_eq!(
        (tap.daily_budget, tap.max_daily_budget, tap.loads_today),
        (300, 1000, 0)
    );
    store.close().await;

    // The old table is gone.
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(&path),
    )
    .await
    .expect("connects");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'budget'",
    )
    .fetch_all(&mut conn)
    .await
    .expect("reads");
    assert!(tables.is_empty(), "{tables:?}");
}

/// Without a settings row the old budget is its own start and maximum;
/// without a budget row nothing is created.
#[tokio::test]
async fn the_migration_needs_neither_row() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("budget-only.db");
    before_site_budgets(&path, OLD_BUDGET.to_owned()).await;
    let store = Store::open(&path).await.expect("opens");
    let gf = store
        .site_budget(GOOGLE_FLIGHTS)
        .await
        .expect("reads")
        .expect("migrated");
    assert_eq!(
        (gf.start_budget, gf.daily_budget, gf.max_daily_budget),
        (5280, 5280, 5280)
    );
    store.close().await;

    let path = dir.path().join("empty.db");
    before_site_budgets(&path, OLD_SETTINGS.to_owned()).await;
    let store = Store::open(&path).await.expect("opens");
    assert!(store.site_budgets().await.expect("reads").is_empty());
    store.close().await;
}

/// Each site counts its own loads, from its own defaults (0060).
#[tokio::test]
async fn each_site_counts_its_own_loads() {
    let store = seeded().await;
    assert!(store.site_budgets().await.expect("reads").is_empty());
    let gf = store
        .site_budget_or_default(GOOGLE_FLIGHTS, date(10, 6))
        .await
        .expect("reads");
    assert_eq!((gf.daily_budget, gf.max_daily_budget), (4800, 12000));
    assert!(store.site_budgets().await.expect("reads").is_empty());

    let b = store
        .add_site_loads(GOOGLE_FLIGHTS, 5, date(10, 6))
        .await
        .expect("adds");
    assert_eq!((b.daily_budget, b.loads_today), (4800, 5));
    let b = store
        .add_site_loads(GOOGLE_FLIGHTS, 7, date(10, 6))
        .await
        .expect("adds");
    assert_eq!(b.loads_today, 12);
    let tap = store
        .add_site_loads("airline:TP", 8, date(10, 6))
        .await
        .expect("adds");
    assert_eq!(
        (tap.start_budget, tap.daily_budget, tap.max_daily_budget),
        (300, 300, 1000)
    );
    assert_eq!(tap.loads_today, 8);
    // A new day starts a new count, per site.
    let b = store
        .add_site_loads(GOOGLE_FLIGHTS, 2, date(10, 7))
        .await
        .expect("adds");
    assert_eq!((b.loads_today, b.day), (2, date(10, 7)));
    assert_eq!(b.loads_on(date(10, 7)), 2);
    assert_eq!(b.loads_on(date(10, 8)), 0);
    let tap = store
        .site_budget("airline:TP")
        .await
        .expect("reads")
        .expect("a row");
    assert_eq!((tap.loads_today, tap.day), (8, date(10, 6)));

    // Google Flights first, then by site.
    let sites: Vec<String> = store
        .site_budgets()
        .await
        .expect("reads")
        .into_iter()
        .map(|b| b.site)
        .collect();
    assert_eq!(sites, ["google_flights", "airline:TP"]);

    let ramped = SiteBudget {
        daily_budget: 330,
        last_ramp_at: Some(at(7, 1)),
        ..tap
    };
    store.save_site_budget(&ramped).await.expect("saves");
    assert_eq!(
        store.site_budget("airline:TP").await.expect("reads"),
        Some(ramped)
    );
}

/// A changed start resets the budget in force; an unchanged one keeps
/// the ramp; a lower maximum caps it.
#[tokio::test]
async fn configuring_a_site_keeps_or_resets_its_budget() {
    let store = seeded().await;
    let limits = SiteLimits {
        start: 300,
        max: 1000,
    };
    let b = store
        .configure_site_budget("airline:TP", limits, date(10, 6), at(6, 0))
        .await
        .expect("configures");
    assert_eq!(b, SiteBudget::new("airline:TP", limits, date(10, 6)));
    store
        .save_site_budget(&SiteBudget {
            daily_budget: 400,
            last_ramp_at: Some(at(6, 1)),
            ..b.clone()
        })
        .await
        .expect("saves");

    let same = store
        .configure_site_budget("airline:TP", limits, date(10, 7), at(7, 0))
        .await
        .expect("configures");
    assert_eq!(same.daily_budget, 400, "the ramp is kept");
    assert_eq!(same.last_ramp_at, Some(at(6, 1)));

    let capped = store
        .configure_site_budget(
            "airline:TP",
            SiteLimits {
                start: 300,
                max: 350,
            },
            date(10, 7),
            at(7, 0),
        )
        .await
        .expect("configures");
    assert_eq!((capped.daily_budget, capped.max_daily_budget), (350, 350));

    let reset = store
        .configure_site_budget(
            "airline:TP",
            SiteLimits {
                start: 600,
                max: 100,
            },
            date(10, 7),
            at(7, 2),
        )
        .await
        .expect("configures");
    assert_eq!(
        (
            reset.start_budget,
            reset.daily_budget,
            reset.max_daily_budget
        ),
        (600, 600, 600)
    );
    assert_eq!(reset.last_ramp_at, Some(at(7, 2)));
}

/// Several sites check at once, each with its own row (0062).
#[tokio::test]
async fn site_statuses_carry_their_checks() {
    let store = seeded().await;
    let lis = outbound(&store, route(&store, "SAO", "LIS").await).await;
    let query = store
        .start_query(lis.id, &QueryReason::Manual, at(6, 12))
        .await
        .expect("starts");
    let checking = SiteStatus {
        site: GOOGLE_FLIGHTS.into(),
        updated_at: at(6, 12),
        state: "checking".into(),
        next_check_at: None,
        check: Some(CheckInProgress {
            family: lis.id,
            query: Some(query),
            reason: QueryReason::Scheduled {
                score: 0.41,
                runner_up: Some(0.2),
            },
            started_at: at(6, 12),
        }),
    };
    let tap = SiteStatus {
        site: "airline:TP".into(),
        updated_at: at(6, 12),
        state: "paused for login: smiles".into(),
        next_check_at: Some(at(6, 13)),
        check: None,
    };
    store.write_site_status(&tap).await.expect("writes");
    store.write_site_status(&checking).await.expect("writes");
    assert_eq!(
        store.site_statuses().await.expect("reads"),
        vec![checking.clone(), tap.clone()]
    );

    let idle = SiteStatus {
        updated_at: at(6, 13),
        state: "idle".into(),
        next_check_at: Some(at(6, 14)),
        check: None,
        ..checking
    };
    store.write_site_status(&idle).await.expect("writes");
    let read = store.site_statuses().await.expect("reads");
    assert_eq!(read, vec![idle, tap]);

    store.clear_site_statuses().await.expect("clears");
    assert!(store.site_statuses().await.expect("reads").is_empty());
}
