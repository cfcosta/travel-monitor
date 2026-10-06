//! "No flights" as an outcome of its own (DECISIONS.md 0067): the
//! migration that lets the query log hold it, and which families it
//! holds back.

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use std::path::Path;

use common::{at, outbound, route, run_query, seeded};
use sqlx::{
    Connection as _,
    Row as _,
    SqliteConnection,
    migrate::Migrator,
    sqlite::SqliteConnectOptions,
};
use tm_domain::{FamilyId, QueryId, QueryOutcome, QueryReason};
use tm_store::Store;

/// A database file at the schema before 0004 (foreign keys on, as sqlx
/// connects), with a query log, observations and Explore hints.
async fn before_no_flights(path: &Path) {
    let mut conn = connect(path, true).await;
    let migrations =
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"));
    Migrator::new(migrations)
        .await
        .expect("reads the migrations")
        .run_to(3, &mut conn)
        .await
        .expect("migrates to 0003");
    sqlx::raw_sql(SETUP)
        .execute(&mut conn)
        .await
        .expect("sets up");
    conn.close().await.expect("closes");
}

async fn connect(path: &Path, create: bool) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(create)
            .foreign_keys(true),
    )
    .await
    .expect("connects")
}

/// Query ids with gaps, every outcome the old CHECK allowed plus a
/// running query, observations on two of them and a hint on a third.
const SETUP: &str = "
INSERT INTO places (id, code, name, country, kind, lat, lon)
VALUES (1, 'GRU', 'Guarulhos', 'BR', 'airport', -23.4, -46.5),
       (2, 'GOH', 'Nuuk', 'GL', 'airport', 64.2, -51.7);
INSERT INTO routes (id, origin_id, destination_id, sweep)
VALUES (10, 1, 2, 0), (11, 1, 1, 1);
INSERT INTO points_of_sale (id, country, currency, is_home)
VALUES (1, 'BR', 'BRL', 1);
INSERT INTO families (id, key, route_id, cabin, source, pos_id, kind, leg,
    site, chg_a, chg_b, deal_a, deal_b, prec_a, prec_b, cost_loads)
VALUES (5, 'gf/10/economy/1/out', 10, 'economy', 'google_flights', 1,
        'leg', 'outbound', 'google_flights', 1, 1, 1, 1, 1, 1, 8),
       (6, 'gf/11/economy/1/explore', 11, 'economy', 'google_flights', 1,
        'explore', NULL, 'google_flights', 1, 1, 1, 1, 1, 1, 1);
INSERT INTO queries (id, family_id, reason, reason_json, passengers,
    scheduled_at, started_at, finished_at, outcome, outcome_detail,
    outcome_json, page_loads, jev_calls)
VALUES
 (3, 5, 'random', '{\"reason\":\"random\"}', '1', '2026-10-06T10:00:00Z',
  '2026-10-06T10:00:00Z', '2026-10-06T10:01:00Z', 'ok', NULL,
  '{\"outcome\":\"ok\"}', 4, 0),
 (7, 5, 'random', '{\"reason\":\"random\"}', '1', '2026-10-06T11:00:00Z',
  '2026-10-06T11:00:00Z', '2026-10-06T11:01:00Z', 'unexpected',
  'the ''Price graph'' control was not found',
  '{\"outcome\":\"unexpected\",\"detail\":\"x\"}', 1, 0),
 (8, 6, 'bootstrap', '{\"reason\":\"bootstrap\"}', '1',
  '2026-10-06T12:00:00Z', '2026-10-06T12:00:00Z', '2026-10-06T12:00:30Z',
  'ok', NULL, '{\"outcome\":\"ok\"}', 1, 0),
 (12, 5, 'manual', '{\"reason\":\"manual\"}', '1', '2026-10-06T13:00:00Z',
  '2026-10-06T13:00:00Z', NULL, NULL, NULL, NULL, NULL, NULL);
INSERT INTO observations (id, query_id, family_id, observed_at, departure,
    price_minor, currency, passengers)
VALUES (100, 3, 5, '2026-10-06T10:01:00Z', '2026-11-01', 390000, 'BRL', '1'),
       (101, 3, 5, '2026-10-06T10:01:00Z', '2026-11-02', 410000, 'BRL', '1'),
       (102, 7, 5, '2026-10-06T11:01:00Z', '2026-11-03', NULL, NULL, '1');
INSERT INTO explore_hints (id, query_id, origin_id, cabin, pos_id,
    observed_at, destination_freebase_id, destination_airport, price_minor,
    currency, passengers)
VALUES (200, 8, 1, 'economy', 1, '2026-10-06T12:00:30Z', '/m/0nuuk', 'GOH',
        650000, 'BRL', '1');
";

async fn strings(conn: &mut SqliteConnection, sql: &str) -> Vec<String> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_all(conn)
        .await
        .expect("reads")
}

/// 0004 rebuilds `queries` with the wider CHECK: rows, ids, indexes,
/// triggers and the foreign keys pointing at it all survive, and the
/// new outcome is accepted.
#[tokio::test]
async fn the_rebuilt_query_log_keeps_rows_and_foreign_keys() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("tm.db");
    before_no_flights(&path).await;

    let store = Store::open(&path).await.expect("opens and migrates");
    // The running query can now end as NoFlights.
    store
        .finish_query(QueryId(12), &QueryOutcome::NoFlights, 1, at(6, 13))
        .await
        .expect("no_flights is allowed");
    let log = store.recent_queries(10).await.expect("reads");
    let ids: Vec<i64> = log.iter().map(|q| q.id.0).collect();
    assert_eq!(ids, [12, 8, 7, 3]);
    assert_eq!(log[0].outcome, Some(QueryOutcome::NoFlights));
    assert_eq!(log[0].page_loads, Some(1));
    // Observations are still counted through the foreign key.
    let cells: Vec<u32> = log.iter().map(|q| q.cells).collect();
    assert_eq!(cells, [0, 0, 1, 2]);
    assert_eq!(
        store.no_flights_families().await.expect("reads"),
        [FamilyId(5)]
    );
    // New queries carry on after the highest id.
    let next = store
        .start_query(FamilyId(6), &QueryReason::Random, at(6, 14))
        .await
        .expect("starts");
    assert_eq!(next, QueryId(13));
    store.close().await;

    let mut conn = connect(&path, false).await;
    // Every column of the old rows is kept.
    let row = sqlx::query(
        "SELECT family_id, reason, outcome, outcome_detail, page_loads,
                started_at, finished_at
         FROM queries WHERE id = 7",
    )
    .fetch_one(&mut conn)
    .await
    .expect("reads");
    assert_eq!(row.get::<i64, _>(0), 5);
    assert_eq!(row.get::<String, _>(1), "random");
    assert_eq!(row.get::<String, _>(2), "unexpected");
    assert_eq!(
        row.get::<String, _>(3),
        "the 'Price graph' control was not found"
    );
    assert_eq!(row.get::<i64, _>(4), 1);
    assert_eq!(row.get::<String, _>(5), "2026-10-06T11:00:00Z");
    assert_eq!(row.get::<String, _>(6), "2026-10-06T11:01:00Z");

    // No dangling reference, and nothing left of the copy.
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut conn)
        .await
        .expect("checks");
    assert!(violations.is_empty(), "{} violations", violations.len());
    let tables = strings(
        &mut conn,
        "SELECT name FROM sqlite_master WHERE name LIKE 'queries%' ORDER BY name",
    )
    .await;
    assert_eq!(tables, ["queries", "queries_family", "queries_started"]);
    let triggers = strings(
        &mut conn,
        "SELECT name FROM sqlite_master
         WHERE type = 'trigger' AND tbl_name = 'observations' ORDER BY name",
    )
    .await;
    assert_eq!(
        triggers,
        ["observations_no_delete", "observations_no_update"]
    );
    // The children still point at `queries` by name, and it holds them.
    for table in ["observations", "explore_hints"] {
        let parents = strings(
            &mut conn,
            &format!(
                "SELECT \"table\" FROM pragma_foreign_key_list('{table}')
                 WHERE \"from\" = 'query_id'"
            ),
        )
        .await;
        assert_eq!(parents, ["queries"], "{table}");
    }
    let orphan = sqlx::query(
        "INSERT INTO observations (query_id, family_id, observed_at,
             departure, passengers)
         VALUES (999, 5, '2026-10-06T15:00:00Z', '2026-11-04', '1')",
    )
    .execute(&mut conn)
    .await;
    assert!(orphan.is_err(), "the foreign key is enforced");
    let bad_family = sqlx::query(
        "INSERT INTO queries (family_id, reason, reason_json, passengers,
             scheduled_at, started_at)
         VALUES (999, 'random', '{}', '1', 'x', 'x')",
    )
    .execute(&mut conn)
    .await;
    assert!(bad_family.is_err(), "queries still references families");
    let bad_outcome =
        sqlx::query("UPDATE queries SET outcome = 'nope' WHERE id = 3")
            .execute(&mut conn)
            .await;
    assert!(bad_outcome.is_err(), "the CHECK still holds");
    conn.close().await.expect("closes");
}

/// A family is held back while its last check that loaded found no
/// flights; a failure after it changes nothing, prices end it.
#[tokio::test]
async fn no_flights_lasts_until_a_check_finds_prices() {
    let store = seeded().await;
    let lis = route(&store, "SAO", "LIS").await;
    let family = outbound(&store, lis).await.id;
    assert!(store.no_flights_families().await.expect("reads").is_empty());

    run_query(&store, family, at(6, 8), &[100], QueryOutcome::Ok).await;
    run_query(&store, family, at(6, 9), &[], QueryOutcome::NoFlights).await;
    assert_eq!(store.no_flights_families().await.expect("reads"), [family]);

    let blocked = QueryOutcome::Blocked {
        detail: "unusual traffic page".into(),
    };
    run_query(&store, family, at(6, 10), &[], blocked).await;
    assert_eq!(store.no_flights_families().await.expect("reads"), [family]);

    run_query(&store, family, at(6, 11), &[120], QueryOutcome::Ok).await;
    assert!(store.no_flights_families().await.expect("reads").is_empty());
}
