//! The query log, observations, hints, deals, requests and daemon
//! bookkeeping.

#![allow(
    clippy::disallowed_methods,
    reason = "`#[tokio::test]` blocks on its runtime: a test is a synchronous entry point"
)]

mod common;

use std::collections::HashSet;

use chrono::Duration;
use common::{at, cell, date, outbound, route, run_query, seeded};
use tm_domain::{
    Cabin,
    Currency,
    Deal,
    DealReason,
    DealStatus,
    ExploreHint,
    FamilyId,
    Money,
    QueryId,
    QueryOutcome,
    QueryReason,
    RouteId,
    SellerQuote,
    Settings,
};
use tm_store::{
    BudgetState,
    CheckInProgress,
    DealFilter,
    Heartbeat,
    RequestKind,
    RequestStatus,
    Store,
    StoreError,
};

#[tokio::test]
async fn query_log_records_reason_and_outcome() {
    let store = seeded().await;
    let lis = route(&store, "SAO", "LIS").await;
    let family = outbound(&store, lis).await;
    let reason = QueryReason::Scheduled {
        score: 0.75,
        runner_up: Some(0.5),
    };
    let q = store
        .start_query(family.id, &reason, at(6, 10))
        .await
        .expect("starts");

    let running = store.recent_queries(10).await.expect("lists");
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].outcome, None);

    store
        .insert_observations(&[cell(
            q,
            family.id,
            at(6, 10),
            date(11, 1),
            Some(3932),
        )])
        .await
        .expect("inserts");
    let blocked = QueryOutcome::Blocked {
        detail: "unusual traffic".into(),
    };
    store
        .finish_query(q, &blocked, 3, at(6, 11))
        .await
        .expect("finishes");
    assert!(matches!(
        store.finish_query(q, &QueryOutcome::Ok, 1, at(6, 12)).await,
        Err(StoreError::NotFound { .. })
    ));

    let e = &store.recent_queries(10).await.expect("lists")[0];
    assert_eq!(e.id, q);
    assert_eq!(e.family, family.id);
    assert_eq!(e.key, family.key);
    assert_eq!(e.origin_code, "SAO");
    assert_eq!(e.destination_code, "LIS");
    assert!(!e.sweep);
    assert_eq!(e.site, "google_flights");
    assert_eq!(e.reason, reason);
    assert_eq!(e.score, Some(0.75));
    assert_eq!(e.runner_up, Some(0.5));
    assert_eq!(e.scheduled_at, at(6, 10));
    assert_eq!(e.finished_at, Some(at(6, 11)));
    assert_eq!(e.outcome, Some(blocked));
    assert_eq!(e.page_loads, Some(3));
    assert_eq!(e.cells, 1);

    let q2 = store
        .start_query(family.id, &QueryReason::Manual, at(6, 12))
        .await
        .expect("starts");
    let log = store.recent_queries(1).await.expect("lists");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].id, q2);
    assert_eq!(log[0].score, None);
}

#[tokio::test]
async fn observations_round_trip() {
    let store = seeded().await;
    let lis = route(&store, "SAO", "LIS").await;
    let family = outbound(&store, lis).await;
    let q = store
        .start_query(family.id, &QueryReason::Bootstrap, at(6, 10))
        .await
        .expect("starts");
    let mut full = cell(q, family.id, at(6, 10), date(11, 1), Some(3932));
    full.return_date = Some(date(11, 8));
    full.miles = Some(42_000);
    full.seller = Some("Mytrip".into());
    let empty = cell(q, family.id, at(6, 10), date(11, 2), None);
    let mut usd = cell(q, family.id, at(6, 10), date(11, 3), None);
    usd.price = Some(Money::new(74_650, Currency::new("USD")));
    let ids = store
        .insert_observations(&[full.clone(), empty.clone(), usd.clone()])
        .await
        .expect("inserts");
    assert_eq!(ids.len(), 3);
    store
        .finish_query(q, &QueryOutcome::Ok, 1, at(6, 10))
        .await
        .expect("finishes");

    let read = store.history(family.id, at(1, 0)).await.expect("reads");
    let expected: Vec<_> = [full, empty, usd]
        .into_iter()
        .zip(&ids)
        .map(|(mut o, id)| {
            o.id = Some(*id);
            o
        })
        .collect();
    assert_eq!(read, expected);
    assert!(
        store
            .history(family.id, at(7, 0))
            .await
            .expect("reads")
            .is_empty()
    );

    // An observation must belong to its query's family.
    let other = FamilyId(family.id.0 + 1);
    assert!(matches!(
        store
            .insert_observations(&[cell(
                q,
                other,
                at(6, 10),
                date(11, 1),
                Some(1)
            )])
            .await,
        Err(StoreError::NotFound { .. })
    ));
    // The failed batch left nothing behind.
    assert_eq!(
        store
            .history(family.id, at(1, 0))
            .await
            .expect("reads")
            .len(),
        3
    );
}

#[tokio::test]
async fn latest_cells_come_from_the_latest_good_query() {
    let store = seeded().await;
    let lis = route(&store, "SAO", "LIS").await;
    let family = outbound(&store, lis).await;
    assert!(
        store
            .latest_cells(family.id)
            .await
            .expect("reads")
            .is_empty()
    );

    run_query(&store, family.id, at(6, 8), &[100, 200], QueryOutcome::Ok).await;
    let good = run_query(
        &store,
        family.id,
        at(6, 9),
        &[110, 210, 310],
        QueryOutcome::Ok,
    )
    .await;
    let error = QueryOutcome::Error {
        detail: "timeout".into(),
    };
    run_query(&store, family.id, at(6, 10), &[1], error).await;

    let cells = store.latest_cells(family.id).await.expect("reads");
    assert_eq!(cells.len(), 3);
    assert!(cells.iter().all(|c| c.query == good));
    let prices: Vec<_> = cells
        .iter()
        .map(|c| c.price.as_ref().map(Money::whole))
        .collect();
    assert_eq!(prices, [Some(110), Some(210), Some(310)]);

    // Every family of the route and cabin.
    let other = store
        .list_families(true)
        .await
        .expect("lists")
        .into_iter()
        .find(|f| {
            f.key.route == lis
                && f.key.cabin == Cabin::Economy
                && f.id != family.id
        })
        .expect("another family");
    run_query(&store, other.id, at(6, 11), &[500], QueryOutcome::Ok).await;
    let all = store
        .history_for_route(lis, Cabin::Economy, at(1, 0))
        .await
        .expect("reads");
    assert_eq!(all.len(), 2 + 3 + 1 + 1);
    assert!(
        store
            .history_for_route(lis, Cabin::Business, at(1, 0))
            .await
            .expect("reads")
            .is_empty()
    );

    // A new party starts new series (0036).
    let mut settings = store.settings().await.expect("reads");
    settings.passengers.adults = 2;
    store.set_settings(&settings).await.expect("saves");
    assert!(
        store
            .latest_cells(family.id)
            .await
            .expect("reads")
            .is_empty()
    );
    assert!(
        store
            .history(family.id, at(1, 0))
            .await
            .expect("reads")
            .is_empty()
    );
    store
        .set_settings(&Settings::default())
        .await
        .expect("saves");
    assert_eq!(store.latest_cells(family.id).await.expect("reads").len(), 3);
}

#[tokio::test]
async fn explore_hints_round_trip() {
    let store = seeded().await;
    let sao = store
        .place_by_code("SAO")
        .await
        .expect("reads")
        .expect("SAO");
    let sweep = store
        .sweep_route(sao.id)
        .await
        .expect("reads")
        .expect("sweep");
    let family = store
        .list_families(true)
        .await
        .expect("lists")
        .into_iter()
        .find(|f| f.key.route == sweep)
        .expect("explore family");
    let q = store
        .start_query(
            family.id,
            &QueryReason::Scheduled {
                score: 1.0,
                runner_up: None,
            },
            at(6, 6),
        )
        .await
        .expect("starts");
    let hint = ExploreHint {
        query: q,
        origin: sao.id,
        cabin: Cabin::Business,
        pos: family.key.pos,
        observed_at: at(6, 6),
        destination_freebase_id: "/m/04llb".into(),
        destination_airport: "LIS".into(),
        price: Money::brl_whole(14_870),
        departure: Some(date(11, 3)),
        return_date: Some(date(11, 10)),
        stops: Some(1),
        duration_minutes: Some(720),
        carrier: Some("TP".into()),
    };
    let mut bare = hint.clone();
    bare.departure = None;
    bare.return_date = None;
    bare.stops = None;
    bare.duration_minutes = None;
    bare.carrier = None;
    bare.observed_at = at(5, 6);
    store
        .insert_explore_hints(&[hint.clone(), bare.clone()])
        .await
        .expect("inserts");
    assert_eq!(
        store.explore_hints_since(at(1, 0)).await.expect("reads"),
        vec![bare, hint.clone()]
    );
    assert_eq!(
        store.explore_hints_since(at(6, 0)).await.expect("reads"),
        vec![hint.clone()]
    );
    let mut orphan = hint;
    orphan.query = QueryId(9999);
    assert!(store.insert_explore_hints(&[orphan]).await.is_err());

    let log = store.recent_queries(1).await.expect("lists");
    assert!(log[0].sweep);
    assert_eq!(log[0].destination_code, "SAO");
}

fn deal(route: RouteId, pos: tm_domain::PosId) -> Deal {
    Deal {
        id: None,
        route,
        cabin: Cabin::Business,
        pos,
        departure: date(11, 3),
        return_date: date(11, 10),
        price: Money::brl_whole(9_000),
        effective_brl: Money::brl_whole(9_000),
        baseline: Some(Money::brl_whole(12_000)),
        reason: DealReason::BelowMedian,
        change: Some(-0.25),
        estimated: true,
        status: DealStatus::Candidate,
        found_at: at(6, 10),
        confirmed_at: None,
        seen: false,
        carrier: Some("TP".into()),
        seller: Some("Mytrip".into()),
        sellers: vec![SellerQuote {
            pos,
            seller: "Mytrip".into(),
            is_airline: false,
            price: Money::brl_whole(9_000),
            quoted: None,
        }],
        url: Some("https://www.google.com/travel/flights?tfs=x".into()),
    }
}

#[tokio::test]
async fn deals_go_through_their_lifecycle() {
    let store = seeded().await;
    let lis = route(&store, "SAO", "LIS").await;
    let pos = store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("BR")
        .id;
    let candidate = deal(lis, pos);
    let id = store.insert_deal(&candidate).await.expect("inserts");
    let read = store.deal(id).await.expect("reads").expect("exists");
    let mut expected = candidate.clone();
    expected.id = Some(id);
    assert_eq!(read, expected);

    let other = store.insert_deal(&deal(lis, pos)).await.expect("inserts");
    store
        .set_deal_status(other, DealStatus::Rejected, at(6, 11))
        .await
        .expect("rejects");

    assert!(store.unseen_confirmed().await.expect("lists").is_empty());
    store
        .set_deal_status(id, DealStatus::Confirmed, at(6, 12))
        .await
        .expect("confirms");
    let tap = SellerQuote {
        pos,
        seller: "TAP Air Portugal".into(),
        is_airline: true,
        price: Money::brl_whole(9_400),
        quoted: Some(Money::new(165_000, Currency::new("EUR"))),
    };
    store
        .attach_seller_quotes(id, std::slice::from_ref(&tap))
        .await
        .expect("attaches");

    let unseen = store.unseen_confirmed().await.expect("lists");
    assert_eq!(unseen.len(), 1);
    assert_eq!(unseen[0].id, Some(id));
    assert_eq!(unseen[0].status, DealStatus::Confirmed);
    assert_eq!(unseen[0].confirmed_at, Some(at(6, 12)));
    assert_eq!(unseen[0].sellers.len(), 2);
    assert_eq!(unseen[0].sellers[1], tap);

    let rejected = store
        .list_deals(&DealFilter {
            status: Some(DealStatus::Rejected),
            ..DealFilter::default()
        })
        .await
        .expect("lists");
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].id, Some(other));
    assert_eq!(rejected[0].confirmed_at, None);

    store.mark_deal_seen(id).await.expect("marks");
    assert!(store.unseen_confirmed().await.expect("lists").is_empty());
    let unseen_any = store
        .list_deals(&DealFilter {
            unseen_only: true,
            ..DealFilter::default()
        })
        .await
        .expect("lists");
    assert_eq!(unseen_any.len(), 1);
    assert_eq!(
        store
            .list_deals(&DealFilter {
                limit: Some(1),
                ..DealFilter::default()
            })
            .await
            .expect("lists")
            .len(),
        1
    );

    store
        .update_deal_quote(
            id,
            &Money::brl_whole(9_100),
            &Money::brl_whole(9_100),
            Some("TP"),
            None,
            Some("https://example.test/booking"),
        )
        .await
        .expect("updates");
    let updated = store.deal(id).await.expect("reads").expect("exists");
    assert_eq!(updated.price, Money::brl_whole(9_100));
    assert_eq!(updated.effective_brl, Money::brl_whole(9_100));
    assert_eq!(updated.carrier.as_deref(), Some("TP"));
    assert_eq!(updated.seller, candidate.seller);
    assert_eq!(updated.url.as_deref(), Some("https://example.test/booking"));
    assert!(matches!(
        store
            .update_deal_quote(
                tm_domain::DealId(999),
                &Money::brl_whole(1),
                &Money::brl_whole(1),
                None,
                None,
                None,
            )
            .await,
        Err(StoreError::NotFound { .. })
    ));

    store
        .set_deal_status(id, DealStatus::Expired, at(7, 0))
        .await
        .expect("expires");
    let expired = store.deal(id).await.expect("reads").expect("exists");
    assert_eq!(expired.status, DealStatus::Expired);
    assert_eq!(expired.confirmed_at, Some(at(6, 12)));
    assert!(expired.seen);
    assert!(matches!(
        store.mark_deal_seen(tm_domain::DealId(999)).await,
        Err(StoreError::NotFound { .. })
    ));
}

#[tokio::test]
async fn requests_are_claimed_in_order() {
    let store = Store::open_in_memory().await.expect("opens");
    let kinds = [
        RequestKind::CheckRoute { route: RouteId(4) },
        RequestKind::CheckFamily {
            family: FamilyId(9),
        },
        RequestKind::ConfirmDeal {
            deal: tm_domain::DealId(2),
        },
        RequestKind::OpenLogin {
            program: "smiles".into(),
        },
        RequestKind::CloseLogin,
        RequestKind::ReloadConfig,
    ];
    let mut ids = Vec::new();
    for kind in &kinds {
        ids.push(store.enqueue(kind).await.expect("enqueues"));
    }
    assert_eq!(store.pending_requests().await.expect("lists").len(), 6);

    let first = store.claim_next().await.expect("claims").expect("one");
    assert_eq!(first.id, ids[0]);
    assert_eq!(first.kind, kinds[0]);
    assert_eq!(first.status, RequestStatus::Claimed);
    assert!(first.claimed_at.is_some());

    store
        .complete(first.id, true, Some("checked 9 families"))
        .await
        .expect("completes");
    let done = store
        .request(first.id)
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(done.status, RequestStatus::Done);
    assert_eq!(done.detail.as_deref(), Some("checked 9 families"));
    assert!(done.finished_at.is_some());
    assert!(store.complete(first.id, true, None).await.is_err());

    let second = store.claim_next().await.expect("claims").expect("one");
    store
        .complete(second.id, false, Some("blocked"))
        .await
        .expect("fails");
    assert_eq!(
        store
            .request(second.id)
            .await
            .expect("reads")
            .expect("exists")
            .status,
        RequestStatus::Failed
    );

    // A crashed daemon's claim goes back to the queue.
    let third = store.claim_next().await.expect("claims").expect("one");
    assert_eq!(store.requeue_claimed().await.expect("requeues"), 1);
    let again = store.claim_next().await.expect("claims").expect("one");
    assert_eq!(again.id, third.id);

    let mut rest = vec![again.kind];
    while let Some(r) = store.claim_next().await.expect("claims") {
        rest.push(r.kind);
    }
    assert_eq!(rest, kinds[2..]);
    assert_eq!(store.pending_requests().await.expect("lists").len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claimers_never_share_a_request() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("tm.db");
    let app = Store::open(&path).await.expect("opens");
    let n = 200;
    for i in 0..n {
        app.enqueue(&RequestKind::CheckFamily {
            family: FamilyId(i),
        })
        .await
        .expect("enqueues");
    }
    let mut tasks = Vec::new();
    for _ in 0..4 {
        // Separate pools, as separate processes would have.
        let daemon = Store::open(&path).await.expect("opens");
        tasks.push(tokio::spawn(async move {
            let mut got = Vec::new();
            while let Some(r) = daemon.claim_next().await.expect("claims") {
                got.push(r.id);
            }
            got
        }));
    }
    let mut all = Vec::new();
    for task in tasks {
        all.extend(task.await.expect("joins"));
    }
    let unique: HashSet<_> = all.iter().collect();
    assert_eq!(all.len(), usize::try_from(n).expect("small"));
    assert_eq!(unique.len(), all.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_reads_while_a_writer_writes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("tm.db");
    let daemon = Store::open(&path).await.expect("opens");
    daemon.seed_defaults().await.expect("seeds");
    let lis = route(&daemon, "SAO", "LIS").await;
    let family = outbound(&daemon, lis).await.id;
    let app = Store::open(&path).await.expect("opens");

    let writer = tokio::spawn(async move {
        for i in 0..40u32 {
            let when = at(6, 0) + Duration::minutes(i64::from(i));
            let prices: Vec<i64> =
                (0..30).map(|d| 1000 + d + i64::from(i)).collect();
            run_query(&daemon, family, when, &prices, QueryOutcome::Ok).await;
        }
    });
    let reader = tokio::spawn(async move {
        let mut seen = 0;
        let mut reads = 0;
        while seen < 40 * 30 {
            let cells = app.latest_cells(family).await.expect("reads");
            // A query's cells appear all at once (one transaction).
            assert!(cells.is_empty() || cells.len() == 30);
            seen = app.history(family, at(1, 0)).await.expect("reads").len();
            assert_eq!(seen % 30, 0);
            app.list_families(true).await.expect("reads");
            reads += 1;
            tokio::task::yield_now().await;
        }
        reads
    });
    writer.await.expect("writer");
    assert!(reader.await.expect("reader") > 0);
}

#[tokio::test]
async fn budget_blocks_rates_and_heartbeat() {
    let store = seeded().await;
    assert_eq!(store.budget().await.expect("reads"), None);
    let b = store.add_loads(5, date(10, 6)).await.expect("adds");
    assert_eq!(b.daily_budget, 2400); // Settings default (0058)
    assert_eq!(b.loads_today, 5);
    let b = store.add_loads(7, date(10, 6)).await.expect("adds");
    assert_eq!(b.loads_today, 12);
    let b = store.add_loads(2, date(10, 7)).await.expect("adds");
    assert_eq!((b.loads_today, b.day), (2, date(10, 7)));

    let ramped = BudgetState {
        daily_budget: 360,
        loads_today: 2,
        day: date(10, 7),
        last_ramp_at: Some(at(7, 1)),
    };
    store.save_budget(&ramped).await.expect("saves");
    assert_eq!(store.budget().await.expect("reads"), Some(ramped));

    store
        .record_block("google_flights", at(6, 3), "unusual traffic", 41)
        .await
        .expect("records");
    store
        .record_block("airline:LA", at(6, 5), "captcha", 3)
        .await
        .expect("records");
    let blocks = store.blocks_since(at(6, 4)).await.expect("reads");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].site, "airline:LA");
    assert_eq!(blocks[0].loads_last_hour, 3);
    assert_eq!(store.blocks_since(at(1, 0)).await.expect("reads").len(), 2);

    let usd = Currency::new("USD");
    assert_eq!(store.latest_exchange_rate(&usd).await.expect("reads"), None);
    store
        .set_exchange_rate(&usd, 5.4, at(5, 0))
        .await
        .expect("sets");
    store
        .set_exchange_rate(&usd, 5.5, at(6, 0))
        .await
        .expect("sets");
    store
        .set_exchange_rate(&Currency::new("eur"), 6.1, at(6, 0))
        .await
        .expect("sets");
    let latest = store
        .latest_exchange_rate(&usd)
        .await
        .expect("reads")
        .expect("rate");
    assert_eq!(latest.brl_per_unit, 5.5);
    assert_eq!(latest.at, at(6, 0));
    let all = store.latest_exchange_rates().await.expect("reads");
    let pairs: Vec<_> = all
        .iter()
        .map(|r| (r.currency.as_str(), r.brl_per_unit))
        .collect();
    assert_eq!(pairs, [("EUR", 6.1), ("USD", 5.5)]);
    assert!(store.set_exchange_rate(&usd, 0.0, at(6, 0)).await.is_err());

    assert_eq!(store.heartbeat().await.expect("reads"), None);
    let beat = Heartbeat {
        pid: 4242,
        updated_at: at(6, 12),
        state: "running".into(),
        next_check: Some("SAO→LIS economy in 4 min".into()),
        next_check_at: Some(at(6, 13)),
        check: None,
        loads_today: 12,
        budget: 300,
    };
    store.write_heartbeat(&beat).await.expect("writes");
    let mut later = beat.clone();
    later.updated_at = at(6, 13);
    later.next_check = None;
    later.next_check_at = None;
    store.write_heartbeat(&later).await.expect("writes");
    assert_eq!(store.heartbeat().await.expect("reads"), Some(later));
}

/// A check in progress is written with the heartbeat and cleared by the
/// next one (0057).
#[tokio::test]
async fn heartbeat_carries_the_check_in_progress() {
    let store = seeded().await;
    let lis = outbound(&store, route(&store, "SAO", "LIS").await).await;
    let query = store
        .start_query(lis.id, &QueryReason::Manual, at(6, 12))
        .await
        .expect("starts");
    let checking = Heartbeat {
        pid: 7,
        updated_at: at(6, 12),
        state: "checking".into(),
        next_check: None,
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
        loads_today: 3,
        budget: 300,
    };
    store.write_heartbeat(&checking).await.expect("writes");
    assert_eq!(
        store.heartbeat().await.expect("reads"),
        Some(checking.clone())
    );

    let idle = Heartbeat {
        updated_at: at(6, 13),
        state: "running".into(),
        next_check_at: Some(at(6, 14)),
        check: None,
        ..checking
    };
    store.write_heartbeat(&idle).await.expect("writes");
    let read = store.heartbeat().await.expect("reads").expect("a beat");
    assert_eq!(read, idle);
    assert_eq!(read.check, None);
}
