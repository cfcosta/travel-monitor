//! The daemon's glue against an in-memory store and a fake source: no
//! browser, no network.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Days, NaiveDate, TimeDelta, Utc};
use tm_daemon::{
    Config,
    Daemon,
    budget,
    candidates::Only,
    sources::{
        AgentJob,
        CalendarJob,
        Cell,
        ConfirmJob,
        Confirmation,
        ExploreJob,
        Fetch,
        ResolveJob,
        Sources,
    },
};
use tm_domain::{
    Cabin,
    Currency,
    DealStatus,
    FamilyId,
    FamilyKey,
    FamilyKind,
    Leg,
    Money,
    Observation,
    PointOfSale,
    PosId,
    QueryOutcome,
    QueryReason,
    RouteId,
    Source,
};
use tm_gflights::rpc::{ExploreDestination, Seller, SellerOffer};
use tm_store::{DealFilter, LoginState, RequestKind, RequestStatus, Store};
use tokio::sync::watch;

type Answer<J, T> = Box<dyn FnMut(&J) -> Fetch<T> + Send>;

/// What the fake saw and answers with.
#[derive(Default)]
struct Log {
    starts: usize,
    stops: usize,
    calendar: Vec<CalendarJob>,
    explore: Vec<ExploreJob>,
    agent: Vec<AgentJob>,
    confirm: Vec<ConfirmJob>,
    resolve: Vec<ResolveJob>,
}

struct Fake {
    running: bool,
    jev: bool,
    log: Arc<Mutex<Log>>,
    calendar: Answer<CalendarJob, Vec<Cell>>,
    explore: Answer<ExploreJob, Vec<ExploreDestination>>,
    agent: Answer<AgentJob, Vec<Cell>>,
    confirm: Answer<ConfirmJob, Option<Confirmation>>,
}

impl Fake {
    fn new(log: Arc<Mutex<Log>>) -> Self {
        Self {
            running: false,
            jev: false,
            log,
            calendar: Box::new(|_| Fetch::ok(Vec::new(), 2, 0)),
            explore: Box::new(|_| Fetch::ok(Vec::new(), 1, 0)),
            agent: Box::new(|_| Fetch::ok(Vec::new(), 1, 3)),
            confirm: Box::new(|_| Fetch::ok(None, 1, 0)),
        }
    }
}

fn log(fake: &Fake) -> std::sync::MutexGuard<'_, Log> {
    fake.log.lock().expect("the log lock")
}

#[async_trait]
impl Sources for Fake {
    async fn start(&mut self, _: &[PointOfSale]) -> anyhow::Result<()> {
        log(self).starts += 1;
        self.running = true;
        Ok(())
    }
    async fn stop(&mut self) {
        if self.running {
            log(self).stops += 1;
        }
        self.running = false;
    }
    fn is_running(&self) -> bool {
        self.running
    }
    fn has_jev(&self) -> bool {
        self.jev
    }
    async fn calendar(&mut self, job: &CalendarJob) -> Fetch<Vec<Cell>> {
        log(self).calendar.push(job.clone());
        (self.calendar)(job)
    }
    async fn explore(
        &mut self,
        job: &ExploreJob,
    ) -> Fetch<Vec<ExploreDestination>> {
        log(self).explore.push(job.clone());
        (self.explore)(job)
    }
    async fn agent_search(&mut self, job: &AgentJob) -> Fetch<Vec<Cell>> {
        log(self).agent.push(job.clone());
        (self.agent)(job)
    }
    async fn confirm(
        &mut self,
        job: &ConfirmJob,
    ) -> Fetch<Option<Confirmation>> {
        log(self).confirm.push(job.clone());
        (self.confirm)(job)
    }
    async fn resolve(&mut self, job: &ResolveJob) -> Fetch<Option<String>> {
        log(self).resolve.push(job.clone());
        // "Flights to Rio de Janeiro" → "/m/rio-de-janeiro".
        let name = job
            .url
            .split("Flights%20to%20")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .unwrap_or("x")
            .replace("%20", "-")
            .to_lowercase();
        Fetch::ok(Some(format!("/m/{name}")), 1, 0)
    }
}

struct Harness {
    daemon: Daemon<Fake>,
    log: Arc<Mutex<Log>>,
    _stop: watch::Sender<bool>,
}

async fn harness(only: Only, tweak: impl FnOnce(&mut Fake)) -> Harness {
    let store = Store::open_in_memory().await.expect("opens");
    let log = Arc::new(Mutex::new(Log::default()));
    let mut fake = Fake::new(Arc::clone(&log));
    tweak(&mut fake);
    let config = Config {
        once: true,
        only,
        pace: (0, 0),
        seed: Some(7),
        budget: Some(300),
        max_budget: Some(2000),
        ..Config::default()
    };
    let (stop, shutdown) = watch::channel(false);
    let mut daemon = Daemon::new(store, fake, config, shutdown);
    daemon.startup().await.expect("starts");
    Harness {
        daemon,
        log,
        _stop: stop,
    }
}

impl Harness {
    fn store(&self) -> &Store {
        self.daemon.store()
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().expect("the log lock")
    }
}

async fn route(store: &Store, dest: &str) -> RouteId {
    store
        .route_between("SAO", dest)
        .await
        .expect("reads")
        .expect("the route exists")
}

async fn home(store: &Store) -> PosId {
    store
        .home_point_of_sale()
        .await
        .expect("reads")
        .expect("BR")
        .id
}

async fn family(
    store: &Store,
    route: RouteId,
    cabin: Cabin,
    source: Source,
    pos: PosId,
    kind: FamilyKind,
) -> FamilyId {
    store
        .family_id(&FamilyKey {
            route,
            cabin,
            source,
            pos,
            kind,
        })
        .await
        .expect("reads")
        .expect("the family exists")
}

async fn leg(store: &Store, dest: &str, leg: Leg) -> FamilyId {
    let r = route(store, dest).await;
    let pos = home(store).await;
    family(
        store,
        r,
        Cabin::Economy,
        Source::GoogleFlights,
        pos,
        FamilyKind::Leg { leg },
    )
    .await
}

fn today() -> NaiveDate {
    budget::today()
}

fn cells(first: NaiveDate, n: u64, price: impl Fn(u64) -> i64) -> Vec<Cell> {
    (0..n)
        .map(|i| {
            Cell::priced(
                first + Days::new(i),
                None,
                Some(Money::brl_whole(price(i))),
            )
        })
        .collect()
}

/// Logs a finished query with observations, as if it ran at `at`.
async fn past_check(
    store: &Store,
    family: FamilyId,
    at: DateTime<Utc>,
    first: NaiveDate,
    n: u64,
    whole: i64,
) {
    let q = store
        .start_query(family, &QueryReason::Random, at)
        .await
        .expect("starts");
    let obs: Vec<Observation> = (0..n)
        .map(|i| Observation {
            id: None,
            query: q,
            family,
            observed_at: at,
            departure: first + Days::new(i),
            return_date: None,
            price: Some(Money::brl_whole(whole)),
            miles: None,
            carrier: None,
            stops: None,
            duration_minutes: None,
            seller: None,
        })
        .collect();
    store.insert_observations(&obs).await.expect("inserts");
    store
        .finish_query(q, &QueryOutcome::Ok, 3, 0, at)
        .await
        .expect("finishes");
}

#[tokio::test]
async fn startup_seeds_and_learns_freebase_ids() {
    let h = harness(Only::default(), |_| {}).await;
    assert_eq!(h.log().starts, 1);
    // The seeded cities without an ID: RIO, BUE, NYC, PAR, LON, ROM, TYO.
    assert_eq!(h.log().resolve.len(), 7);
    let rome = h
        .store()
        .place_by_code("ROM")
        .await
        .expect("reads")
        .expect("ROM");
    assert_eq!(rome.google_id(), Some("/m/rome"));
    let beat = h
        .store()
        .heartbeat()
        .await
        .expect("reads")
        .expect("a heartbeat");
    assert_eq!(beat.state, "running");
    assert_eq!(beat.budget, 300);
    assert_eq!(beat.loads_today, 7);
}

#[tokio::test]
async fn requests_pause_resume_and_check() {
    let mut h = harness(Only::default(), |_| {}).await;
    let store = h.store().clone();
    let lis = leg(&store, "LIS", Leg::Outbound).await;

    let open = store
        .enqueue(&RequestKind::OpenLogin {
            program: "smiles".into(),
        })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    assert!(h.daemon.is_paused());
    assert_eq!(h.log().stops, 1);
    let r = store.request(open).await.expect("reads").expect("exists");
    assert_eq!(r.status, RequestStatus::Done);
    let beat = store
        .heartbeat()
        .await
        .expect("reads")
        .expect("a heartbeat");
    assert_eq!(beat.state, "paused for login: smiles");

    // While paused, checks wait and the tick does nothing.
    let check = store
        .enqueue(&RequestKind::CheckFamily { family: lis })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await.expect("runs");
    h.daemon.tick().await.expect("ticks");
    assert!(h.log().calendar.is_empty());
    let r = store.request(check).await.expect("reads").expect("exists");
    assert_eq!(r.status, RequestStatus::Claimed);

    let close = store
        .enqueue(&RequestKind::CloseLogin)
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    assert!(!h.daemon.is_paused());
    assert_eq!(h.log().starts, 2);
    let smiles = store
        .list_miles_programs()
        .await
        .expect("reads")
        .into_iter()
        .find(|m| m.program == "smiles")
        .expect("smiles");
    assert_eq!(smiles.login_state, LoginState::LoggedIn);
    assert_eq!(
        store
            .request(close)
            .await
            .expect("reads")
            .expect("exists")
            .status,
        RequestStatus::Done
    );

    // The deferred check runs first, as a manual query.
    h.daemon.run_manual().await.expect("runs");
    assert_eq!(h.log().calendar.len(), 1);
    assert_eq!(
        store
            .request(check)
            .await
            .expect("reads")
            .expect("exists")
            .status,
        RequestStatus::Done
    );
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].family, lis);
    assert_eq!(recent[0].reason, QueryReason::Manual);

    let reload = store
        .enqueue(&RequestKind::ReloadConfig)
        .await
        .expect("enqueues");
    let missing = store
        .enqueue(&RequestKind::ConfirmDeal {
            deal: tm_domain::DealId(42),
        })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    assert_eq!(
        store
            .request(reload)
            .await
            .expect("reads")
            .expect("exists")
            .status,
        RequestStatus::Done
    );
    assert_eq!(
        store
            .request(missing)
            .await
            .expect("reads")
            .expect("exists")
            .status,
        RequestStatus::Failed
    );

    // A route check queues every eligible family of the route.
    let ssa = route(&store, "SSA").await;
    let before = h.log().calendar.len();
    store
        .enqueue(&RequestKind::CheckRoute { route: ssa })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await.expect("runs");
    // Domestic: economy only; legs and round trip under four points of
    // sale. Airline and miles families need Jev, which the fake lacks.
    assert_eq!(h.log().calendar.len() - before, 12);
}

#[tokio::test]
async fn a_tick_checks_records_and_learns() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let id = leg(&store_probe, "LIS", Leg::Outbound).await;
    let first = today() + Days::new(1);
    let mut h = harness(
        Only {
            family: Some(id),
            destination: None,
        },
        move |f| {
            f.calendar = Box::new(move |_| {
                Fetch::ok(cells(first, 60, |i| 2_000 + i as i64), 4, 0)
            })
        },
    )
    .await;
    let store = h.store().clone();
    h.daemon.tick().await.expect("ticks");
    {
        let log = h.log();
        assert_eq!(log.calendar.len(), 1);
        let job = &log.calendar[0];
        assert!(job.url.contains("tfs="));
        assert!(job.url.ends_with("&hl=en&gl=BR&curr=BRL"));
        assert_eq!(job.max_pages, 7);
    }
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].family, id);
    assert_eq!(recent[0].outcome, Some(QueryOutcome::Ok));
    assert_eq!(recent[0].reason, QueryReason::Bootstrap);
    assert_eq!(recent[0].page_loads, Some(4));
    assert_eq!(recent[0].cells, 60);
    let f = store.family(id).await.expect("reads").expect("exists");
    assert_eq!(f.state.checks, 1);
    assert_eq!(f.state.last_min_price_minor, Some(200_000));
    assert!(f.state.last_check.is_some());
    let budget = store.budget().await.expect("reads").expect("a budget");
    assert_eq!(budget.loads_today, 4);

    // Checked within the minimum interval: the next tick leaves it.
    h.daemon.tick().await.expect("ticks");
    assert_eq!(h.log().calendar.len(), 1);
}

#[tokio::test]
async fn a_block_backs_off() {
    let mut h = harness(
        Only {
            family: None,
            destination: Some("LIS".into()),
        },
        |f| {
            f.calendar = Box::new(|_| {
                Fetch::failed(
                    QueryOutcome::Blocked {
                        detail: "unusual traffic page".into(),
                    },
                    1,
                    0,
                )
            })
        },
    )
    .await;
    let store = h.store().clone();
    h.daemon.tick().await.expect("ticks");
    // The first block cools Google Flights down: no further page.
    assert_eq!(h.log().calendar.len(), 1);
    let blocks = store
        .blocks_since(Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].site, "google_flights");
    assert_eq!(blocks[0].detail, "unusual traffic page");
    let budget = store.budget().await.expect("reads").expect("a budget");
    assert_eq!(budget.daily_budget, 150);
    let recent = store.recent_queries(1).await.expect("reads");
    assert!(
        recent[0]
            .outcome
            .as_ref()
            .is_some_and(QueryOutcome::is_block)
    );
}

#[tokio::test]
async fn deals_are_found_and_confirmed() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let out_id = leg(&store_probe, "LIS", Leg::Outbound).await;
    let first = today() + Days::new(50);
    let mut h = harness(
        Only {
            family: Some(out_id),
            destination: None,
        },
        move |f| {
            // The outbound drops 40% on two departures.
            f.calendar = Box::new(move |_| {
                Fetch::ok(
                    cells(first, 20, |i| if i < 2 { 600 } else { 1_000 }),
                    3,
                    0,
                )
            });
            f.confirm = Box::new(|job| {
                let offers = vec![
                    SellerOffer {
                        sellers: vec![Seller {
                            code: "TP".into(),
                            name: "TAP Air Portugal".into(),
                            is_airline: true,
                        }],
                        price: Some(1_700),
                        exact: None,
                        quoted: None,
                        flights: vec![("TP".into(), "84".into())],
                    },
                    SellerOffer {
                        sellers: vec![Seller {
                            code: "KIWI".into(),
                            name: "Kiwi.com".into(),
                            is_airline: false,
                        }],
                        price: Some(1_610),
                        exact: None,
                        quoted: Some(Money::from_whole(
                            322,
                            Currency::new("USD"),
                        )),
                        flights: vec![],
                    },
                ];
                Fetch::ok(
                    Some(Confirmation {
                        cheapest: Some(1_620),
                        carrier: Some("TP".into()),
                        search_url: format!(
                            "search for {}",
                            job.search.legs[0].date
                        ),
                        booking_url: Some("booking".into()),
                        offers,
                    }),
                    3,
                    0,
                )
            });
        },
    )
    .await;
    let store = h.store().clone();
    let ret_id = leg(&store, "LIS", Leg::Return).await;
    // Five weeks of steady legs, R$1,000 each way.
    let now = Utc::now();
    for k in (1..=36).rev() {
        let at = now - TimeDelta::days(k) - TimeDelta::hours(1);
        past_check(&store, out_id, at, first, 20, 1_000 + (k % 3) * 10).await;
        past_check(&store, ret_id, at, first, 20, 1_000).await;
    }
    // The return leg was read an hour ago.
    past_check(&store, ret_id, now - TimeDelta::hours(1), first, 20, 1_000)
        .await;

    let world = h.daemon.world().await.expect("loads");
    h.daemon
        .run_check(out_id, QueryReason::Manual, &world)
        .await
        .expect("checks");

    let confirms = h.log().confirm.len();
    assert_eq!(confirms, 2, "two departures, one confirmation each");
    {
        let log = h.log();
        let job = &log.confirm[0];
        assert_eq!(job.search.legs.len(), 2);
        assert_eq!(job.search.legs[0].date, first);
        assert_eq!(job.locale.gl.as_str(), "BR");
    }
    let deals = store
        .list_deals(&DealFilter::default())
        .await
        .expect("lists");
    assert_eq!(deals.len(), 2);
    for d in &deals {
        assert_eq!(d.status, DealStatus::Confirmed, "{d:?}");
        assert!(d.estimated);
        assert_eq!(d.price, Money::brl_whole(1_620));
        assert_eq!(d.carrier.as_deref(), Some("TP"));
        assert_eq!(d.url.as_deref(), Some("booking"));
        assert_eq!(d.sellers.len(), 2);
        assert!(
            d.sellers
                .iter()
                .any(|s| s.is_airline && s.price == Money::brl_whole(1_700))
        );
        // Home point of sale, but Kiwi quotes USD: IOF and spread apply,
        // so TAP's BRL price... is still dearer; Kiwi wins at
        // 1,610 × 1.035 × 1.01.
        assert_eq!(d.seller.as_deref(), Some("Kiwi.com"));
        assert!(
            d.effective_brl.whole() >= 1_682
                && d.effective_brl.whole() <= 1_684,
            "{:?}",
            d.effective_brl
        );
        assert!(d.departure == first || d.departure == first + Days::new(1));
    }
    let usd = store
        .latest_exchange_rate(&Currency::new("USD"))
        .await
        .expect("reads")
        .expect("a rate");
    assert!((usd.brl_per_unit - 5.0).abs() < 0.001, "{usd:?}");
    let recent = store.recent_queries(3).await.expect("reads");
    assert_eq!(recent[0].reason, QueryReason::Confirmation);
    let trip_state = store
        .route_trip_state(route(&store, "LIS").await)
        .await
        .expect("reads")
        .expect("a state");
    assert_eq!(trip_state.rt_discount_n, 2);

    // The same check again finds the same trips: already known.
    let world = h.daemon.world().await.expect("loads");
    h.daemon
        .run_check(out_id, QueryReason::Manual, &world)
        .await
        .expect("checks");
    assert_eq!(h.log().confirm.len(), 2);
}

#[tokio::test]
async fn a_rejected_confirmation_does_not_alert() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let out_id = leg(&store_probe, "MIA", Leg::Outbound).await;
    let first = today() + Days::new(50);
    let mut h = harness(Only::default(), move |f| {
        f.calendar = Box::new(move |_| {
            Fetch::ok(
                cells(first, 20, |i| if i == 0 { 500 } else { 1_000 }),
                3,
                0,
            )
        });
        f.confirm = Box::new(|_| {
            Fetch::ok(
                Some(Confirmation {
                    cheapest: Some(1_900),
                    ..Confirmation::default()
                }),
                1,
                0,
            )
        });
    })
    .await;
    let store = h.store().clone();
    let ret_id = leg(&store, "MIA", Leg::Return).await;
    let now = Utc::now();
    for k in (0..=36).rev() {
        let at = now - TimeDelta::days(k) - TimeDelta::hours(1);
        past_check(&store, out_id, at, first, 20, 1_000).await;
        past_check(&store, ret_id, at, first, 20, 1_000).await;
    }
    let world = h.daemon.world().await.expect("loads");
    h.daemon
        .run_check(out_id, QueryReason::Manual, &world)
        .await
        .expect("checks");
    let deals = store
        .list_deals(&DealFilter::default())
        .await
        .expect("lists");
    assert_eq!(deals.len(), 1);
    assert_eq!(deals[0].status, DealStatus::Rejected);
    assert!(store.unseen_confirmed().await.expect("reads").is_empty());
}

#[tokio::test]
async fn explore_hints_raise_their_families() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let pos = home(&store_probe).await;
    let sao = store_probe
        .place_by_code("SAO")
        .await
        .expect("reads")
        .expect("SAO");
    let sweep = store_probe
        .sweep_route(sao.id)
        .await
        .expect("reads")
        .expect("a sweep");
    let explore = family(
        &store_probe,
        sweep,
        Cabin::Economy,
        Source::GoogleFlights,
        pos,
        FamilyKind::Explore,
    )
    .await;
    let mut h = harness(
        Only {
            family: Some(explore),
            destination: None,
        },
        |f| {
            f.explore = Box::new(|_| {
                Fetch::ok(
                    vec![ExploreDestination {
                        freebase_id: "/m/04llb".into(),
                        airport: Some("LIS".into()),
                        origin_airport: Some("GRU".into()),
                        price: Some(1_500),
                        exact: None,
                        departure: NaiveDate::from_ymd_opt(2026, 11, 12),
                        return_date: NaiveDate::from_ymd_opt(2026, 11, 19),
                        stops: Some(0),
                        duration_minutes: Some(600),
                        carrier_code: Some("TP".into()),
                        carrier: Some("TAP".into()),
                        name: Some("Lisbon".into()),
                        country: Some("Portugal".into()),
                        coordinates: None,
                    }],
                    1,
                    0,
                )
            })
        },
    )
    .await;
    let store = h.store().clone();
    // The route's legs last cost R$1,000 each: R$1,500 is a 25% drop.
    for leg_kind in [Leg::Outbound, Leg::Return] {
        let id = leg(&store, "LIS", leg_kind).await;
        let mut f = store.family(id).await.expect("reads").expect("exists");
        f.state.last_min_price_minor = Some(100_000);
        f.state.last_check = Some(Utc::now());
        store.save_family_state(id, &f.state).await.expect("saves");
    }
    h.daemon.tick().await.expect("ticks");
    assert_eq!(h.log().explore.len(), 1);
    assert!(
        h.log().explore[0]
            .url
            .contains("explore?q=Flights%20from%20SAO&hl=en&gl=BR")
    );
    let hints = store
        .explore_hints_since(Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].destination_airport, "LIS");
    let out = store
        .family(leg(&store, "LIS", Leg::Outbound).await)
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(out.state.n_hints, 1);
    let mia = store
        .family(leg(&store, "MIA", Leg::Outbound).await)
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(mia.state.n_hints, 0);
}

#[tokio::test]
async fn miles_sessions_that_ended_are_flagged() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let pos = home(&store_probe).await;
    let lis = route(&store_probe, "LIS").await;
    let miles = family(
        &store_probe,
        lis,
        Cabin::Economy,
        Source::Miles {
            program: "smiles".into(),
        },
        pos,
        FamilyKind::Leg { leg: Leg::Outbound },
    )
    .await;
    let airline = family(
        &store_probe,
        lis,
        Cabin::Economy,
        Source::Airline { code: "TP".into() },
        pos,
        FamilyKind::Leg { leg: Leg::Outbound },
    )
    .await;
    let mut h = harness(Only::default(), |f| {
        f.jev = true;
        f.agent = Box::new(|job| {
            if job.miles {
                Fetch::failed(QueryOutcome::LoggedOut, 1, 2)
            } else {
                Fetch::ok(
                    vec![Cell {
                        seller: Some(format!("unverified:{}", job.site)),
                        ..Cell::priced(
                            job.departure,
                            None,
                            Some(Money::brl_whole(2_345)),
                        )
                    }],
                    2,
                    5,
                )
            }
        });
    })
    .await;
    let store = h.store().clone();
    store
        .set_miles_login("smiles", LoginState::LoggedIn, Utc::now())
        .await
        .expect("sets");
    let world = h.daemon.world().await.expect("loads");
    h.daemon
        .run_check(miles, QueryReason::Manual, &world)
        .await
        .expect("checks");
    let smiles = store
        .list_miles_programs()
        .await
        .expect("reads")
        .into_iter()
        .find(|m| m.program == "smiles")
        .expect("smiles");
    assert_eq!(smiles.login_state, LoginState::Expired);
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].outcome, Some(QueryOutcome::LoggedOut));
    assert_eq!(recent[0].jev_calls, Some(2));

    h.daemon
        .run_check(airline, QueryReason::Manual, &world)
        .await
        .expect("checks");
    {
        let log = h.log();
        let job = log.agent.last().expect("an agent job");
        assert_eq!(job.site, "airline:TP");
        assert!(!job.miles);
    }
    let since = Utc::now() - TimeDelta::hours(1);
    let obs = store.history(airline, since).await.expect("reads");
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].seller.as_deref(), Some("unverified:airline:TP"));
    assert_eq!(obs[0].price, Some(Money::brl_whole(2_345)));
}
