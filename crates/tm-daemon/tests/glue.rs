//! The daemon's glue against an in-memory store and a fake source: no
//! browser, no network.

#![allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on: a test is the runtime's entry point"
)]

use std::{
    sync::{
        Arc,
        Mutex,
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::{DateTime, Days, NaiveDate, TimeDelta, Utc};
use tm_daemon::{
    Config,
    Daemon,
    budget,
    candidates::Only,
    notify::Recorder,
    sources::{
        CalendarJob,
        Cell,
        ConfirmJob,
        Confirmation,
        ExploreJob,
        Fetch,
        LatamFound,
        LatamJob,
        ResolveJob,
        Sources,
        TapJob,
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
    SiteLimits,
    Source,
};
use tm_gflights::rpc::{ExploreDestination, Seller, SellerOffer};
use tm_store::{
    DealFilter,
    LoginState,
    RequestKind,
    RequestStatus,
    SiteStatus,
    Store,
};
use tokio::sync::watch;

type Answer<J, T> = Box<dyn Fn(&J) -> Fetch<T> + Send + Sync>;

/// One fetch, timed: which site ran what, from when to when.
#[derive(Debug, Clone)]
struct Span {
    site: String,
    start: Instant,
    end: Instant,
}

/// What the fakes saw, every site's together.
#[derive(Default)]
struct Log {
    /// The site of each browser start and stop.
    starts: Vec<String>,
    stops: Vec<String>,
    spans: Vec<Span>,
    calendar: Vec<CalendarJob>,
    explore: Vec<ExploreJob>,
    tap: Vec<TapJob>,
    latam: Vec<LatamJob>,
    confirm: Vec<ConfirmJob>,
    resolve: Vec<ResolveJob>,
}

/// How every site's fake answers.
struct Answers {
    calendar: Answer<CalendarJob, Vec<Cell>>,
    explore: Answer<ExploreJob, Vec<ExploreDestination>>,
    tap: Answer<TapJob, Vec<Cell>>,
    latam: Answer<LatamJob, LatamFound>,
    confirm: Answer<ConfirmJob, Option<Confirmation>>,
    /// How long a calendar or TAP fetch takes.
    delay: Duration,
}

impl Default for Answers {
    fn default() -> Self {
        Self {
            calendar: Box::new(|_| Fetch::ok(Vec::new(), 2)),
            explore: Box::new(|_| Fetch::ok(Vec::new(), 1)),
            tap: Box::new(|_| Fetch::ok(Vec::new(), 1)),
            latam: Box::new(|_| Fetch::ok(LatamFound::default(), 1)),
            confirm: Box::new(|_| Fetch::ok(None, 1)),
            delay: Duration::ZERO,
        }
    }
}

/// One site's sources (0062): the daemon makes one per site.
struct Fake {
    site: String,
    running: AtomicBool,
    log: Arc<Mutex<Log>>,
    answers: Arc<Answers>,
}

impl Fake {
    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().expect("the log lock")
    }

    /// Waits the fetch's delay and records its span.
    async fn timed(&self) {
        let start = Instant::now();
        if !self.answers.delay.is_zero() {
            tokio::time::sleep(self.answers.delay).await;
        }
        self.log().spans.push(Span {
            site: self.site.clone(),
            start,
            end: Instant::now(),
        });
    }
}

#[async_trait]
impl Sources for Fake {
    async fn start(&self, _: &[PointOfSale], _: u64) -> anyhow::Result<()> {
        let site = self.site.clone();
        self.log().starts.push(site);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            let site = self.site.clone();
            self.log().stops.push(site);
        }
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    async fn calendar(&self, job: &CalendarJob) -> Fetch<Vec<Cell>> {
        assert_eq!(self.site, "google_flights");
        self.log().calendar.push(job.clone());
        self.timed().await;
        (self.answers.calendar)(job)
    }
    async fn explore(
        &self,
        job: &ExploreJob,
    ) -> Fetch<Vec<ExploreDestination>> {
        assert_eq!(self.site, "google_flights");
        self.log().explore.push(job.clone());
        (self.answers.explore)(job)
    }
    async fn tap(&self, job: &TapJob) -> Fetch<Vec<Cell>> {
        assert_eq!(self.site, "airline:TP");
        self.log().tap.push(job.clone());
        self.timed().await;
        (self.answers.tap)(job)
    }
    async fn latam(&self, job: &LatamJob) -> Fetch<LatamFound> {
        // Cash and awards share LATAM Pass's worker and profile (0084).
        assert_eq!(self.site, "miles:latam_pass");
        self.log().latam.push(job.clone());
        self.timed().await;
        (self.answers.latam)(job)
    }
    async fn confirm(&self, job: &ConfirmJob) -> Fetch<Option<Confirmation>> {
        assert_eq!(self.site, "google_flights");
        self.log().confirm.push(job.clone());
        (self.answers.confirm)(job)
    }
    async fn resolve(&self, job: &ResolveJob) -> Fetch<Option<String>> {
        assert_eq!(self.site, "google_flights");
        self.log().resolve.push(job.clone());
        // "Flights to Rio de Janeiro" → "/m/rio-de-janeiro".
        let name = job
            .url
            .split("Flights%20to%20")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .unwrap_or("x")
            .replace("%20", "-")
            .to_lowercase();
        Fetch::ok(Some(format!("/m/{name}")), 1)
    }
}

struct Harness {
    daemon: Daemon<Fake>,
    log: Arc<Mutex<Log>>,
    _stop: watch::Sender<bool>,
}

/// The test configuration: one tick at a time, one check per site at a
/// time (so checks land in pick order), no pauses, Google Flights at 300
/// loads a day.
fn test_config(only: Only) -> Config {
    Config {
        once: true,
        only,
        pace: (0, 0),
        parallel: 1,
        seed: Some(7),
        budget: Some(300),
        max_budget: Some(2000),
        ..Config::default()
    }
}

/// A daemon over a fresh in-memory store whose sites answer with
/// `answers`. Not started.
async fn daemon(
    config: Config,
    answers: Answers,
) -> (Daemon<Fake>, Arc<Mutex<Log>>, watch::Sender<bool>, Store) {
    let log = Arc::new(Mutex::new(Log::default()));
    let answers = Arc::new(answers);
    let (stop, shutdown) = watch::channel(false);
    let store = Store::open_in_memory().await.expect("opens");
    let sources = {
        let log = Arc::clone(&log);
        move |site: &str| Fake {
            site: site.to_owned(),
            running: AtomicBool::new(false),
            log: Arc::clone(&log),
            answers: Arc::clone(&answers),
        }
    };
    let daemon = Daemon::new(store.clone(), sources, config, shutdown);
    (daemon, log, stop, store)
}

async fn harness(only: Only, tweak: impl FnOnce(&mut Answers)) -> Harness {
    harness_with(test_config(only), tweak).await
}

/// [`harness`] with a configuration of its own.
async fn harness_with(
    config: Config,
    tweak: impl FnOnce(&mut Answers),
) -> Harness {
    let mut answers = Answers::default();
    tweak(&mut answers);
    let (mut daemon, log, stop, _) = daemon(config, answers).await;
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

    async fn site_budget(&self, site: &str) -> tm_store::SiteBudget {
        self.store()
            .site_budget(site)
            .await
            .expect("reads")
            .expect("the site has a budget")
    }

    async fn status(&self, site: &str) -> SiteStatus {
        self.store()
            .site_statuses()
            .await
            .expect("reads")
            .into_iter()
            .find(|s| s.site == site)
            .expect("the site has a status")
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
            flights: None,
            award: None,
        })
        .collect();
    store.insert_observations(&obs).await.expect("inserts");
    store
        .finish_query(q, &QueryOutcome::Ok, 3, at)
        .await
        .expect("finishes");
}

#[tokio::test]
async fn startup_seeds_and_learns_freebase_ids() {
    let h = harness(Only::default(), |_| {}).await;
    // Only Google Flights had work: learning the IDs (0062).
    assert_eq!(h.log().starts, ["google_flights"]);
    // The seeded cities without an ID: RIO, BUE, NYC, PAR, LON, ROM, TYO,
    // MIL and SEL (SAO and WAS are seeded with theirs).
    assert_eq!(h.log().resolve.len(), 9);
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
    // One page load per city learned, counted on Google Flights.
    assert_eq!(beat.loads_today, 9);
    let google = h.site_budget("google_flights").await;
    assert_eq!((google.daily_budget, google.loads_today), (300, 9));
    // A worker per site with a reader and eligible families.
    // LATAM's cash and awards share one worker, named for LATAM Pass
    // (0084).
    assert_eq!(
        h.daemon.sites(),
        ["airline:TP", "google_flights", "miles:latam_pass"]
    );
    assert_eq!(h.status("google_flights").await.state, "idle");
    assert_eq!(h.status("airline:TP").await.state, "idle");
    assert_eq!(h.status("miles:latam_pass").await.state, "idle");
}

/// The site's status says what it checks while the source runs, and
/// goes back to idle when it is done (0057, 0062).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_check_writes_start_and_end_statuses() {
    let slot: Arc<OnceLock<Store>> = Arc::new(OnceLock::new());
    let during: Arc<Mutex<Vec<SiteStatus>>> = Arc::default();
    let mut h = harness(Only::default(), |fake| {
        let (slot, during) = (Arc::clone(&slot), Arc::clone(&during));
        fake.calendar = Box::new(move |_| {
            // What the app would read while the page loads.
            if let Some(store) = slot.get() {
                let statuses = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(store.site_statuses())
                })
                .expect("reads");
                let google = statuses
                    .into_iter()
                    .find(|s| s.site == "google_flights")
                    .expect("Google Flights' status");
                during.lock().expect("lock").push(google);
            }
            Fetch::ok(Vec::new(), 2)
        });
    })
    .await;
    let store = h.store().clone();
    assert!(slot.set(store.clone()).is_ok());
    let lis = leg(&store, "LIS", Leg::Outbound).await;
    let before = h.status("google_flights").await;
    assert_eq!(before.state, "idle");
    assert!(before.checks.is_empty());

    h.daemon
        .run_check(lis, QueryReason::Manual)
        .await
        .expect("checks");

    let during = during.lock().expect("lock").clone();
    assert_eq!(during.len(), 1, "the source ran once");
    let status = &during[0];
    assert_eq!(status.state, "checking");
    assert_eq!(status.checks.len(), 1, "the check in progress");
    let check = &status.checks[0];
    assert_eq!(check.family, lis);
    assert_eq!(check.reason, QueryReason::Manual);
    assert!(check.started_at >= before.updated_at);
    let logged = store.recent_queries(1).await.expect("reads");
    assert_eq!(check.query, Some(logged[0].id));
    assert!(logged[0].finished_at.is_some());

    let after = h.status("google_flights").await;
    assert_eq!(after.state, "idle");
    assert!(after.checks.is_empty());
    assert!(after.updated_at >= status.updated_at);
    // The other site was not touched.
    assert!(h.status("airline:TP").await.checks.is_empty());
}

/// A login pauses its own site only (0062): Google Flights and TAP keep
/// checking while Smiles' profile is open.
#[tokio::test]
async fn a_login_pauses_only_its_site() {
    let mut h = harness_with(
        Config {
            budget: Some(60),
            max_budget: Some(60),
            site_budgets: vec![(
                "airline:TP".into(),
                SiteLimits { start: 20, max: 20 },
            )],
            ..test_config(Only::default())
        },
        |_| {},
    )
    .await;
    let store = h.store().clone();
    let lis = leg(&store, "LIS", Leg::Outbound).await;

    let open = store
        .enqueue(&RequestKind::OpenLogin {
            program: "smiles".into(),
        })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    assert!(h.daemon.is_paused("miles:smiles"));
    assert!(!h.daemon.is_paused("google_flights"));
    assert_eq!(h.daemon.paused_sites(), ["miles:smiles"]);
    // Nothing else closed its browser.
    assert!(h.log().stops.is_empty());
    let r = store.request(open).await.expect("reads").expect("exists");
    assert_eq!(r.status, RequestStatus::Done);
    assert_eq!(
        h.status("miles:smiles").await.state,
        "paused for login: smiles"
    );
    assert_eq!(h.status("google_flights").await.state, "idle");
    let beat = store
        .heartbeat()
        .await
        .expect("reads")
        .expect("a heartbeat");
    assert_eq!(beat.state, "running");

    // While Smiles is paused, Google Flights' manual checks and every
    // site's tick run.
    let check = store
        .enqueue(&RequestKind::CheckFamily { family: lis })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await;
    assert_eq!(h.log().calendar.len(), 1);
    let r = store.request(check).await.expect("reads").expect("exists");
    assert_eq!(r.status, RequestStatus::Done);
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].family, lis);
    assert_eq!(recent[0].reason, QueryReason::Manual);
    h.daemon.tick().await.expect("ticks");
    assert!(h.log().calendar.len() > 1);
    assert!(!h.log().tap.is_empty());
    assert!(h.daemon.is_paused("miles:smiles"));

    let close = store
        .enqueue(&RequestKind::CloseLogin {
            program: Some("smiles".into()),
        })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    assert!(h.daemon.paused_sites().is_empty());
    let smiles = store
        .list_miles_programs()
        .await
        .expect("reads")
        .into_iter()
        .find(|m| m.program == "smiles")
        .expect("smiles");
    // No profile to check: a closed window is not taken for a login.
    assert_eq!(smiles.login_state, LoginState::LoggedOut);
    assert_eq!(
        store
            .request(close)
            .await
            .expect("reads")
            .expect("exists")
            .status,
        RequestStatus::Done
    );
    assert_eq!(h.status("miles:smiles").await.state, "idle");
    // Smiles has no reader: it never started a browser.
    assert!(!h.log().starts.iter().any(|s| s == "miles:smiles"));

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

    // A route check queues every eligible family of the route, on each
    // site, and is done when the last site finishes.
    let ssa = route(&store, "SSA").await;
    let before = h.log().calendar.len();
    let request = store
        .enqueue(&RequestKind::CheckRoute { route: ssa })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await;
    // Domestic: economy only; legs and round trip under four points of
    // sale. Airline and miles families other than TAP have no reader.
    assert_eq!(h.log().calendar.len() - before, 12);
    assert_eq!(
        store
            .request(request)
            .await
            .expect("reads")
            .expect("exists")
            .status,
        RequestStatus::Done
    );
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
                Fetch::ok(cells(first, 60, |i| 2_000 + i as i64), 4)
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
    let budget = h.site_budget("google_flights").await;
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
    let budget = h.site_budget("google_flights").await;
    assert_eq!(budget.daily_budget, 150);
    // The block is per site: TAP's legs on the route still run after it,
    // on TAP's own budget (0060).
    let tap = h.site_budget("airline:TP").await;
    assert_eq!(tap.daily_budget, 1200);
    assert!(tap.loads_today > 0);
    assert_eq!(h.status("google_flights").await.state, "cooling down");
    assert_eq!(h.status("airline:TP").await.state, "idle");
    let recent = store.recent_queries(20).await.expect("reads");
    let blocked: Vec<_> = recent
        .iter()
        .filter(|q| q.outcome.as_ref().is_some_and(QueryOutcome::is_block))
        .collect();
    assert_eq!(blocked.len(), 1);
    assert!(!h.log().tap.is_empty());
}

/// A page that says there are no flights ends the check as NoFlights
/// (0067): its load is spent, nothing is observed, nothing is blocked,
/// and the family (a daily core one) waits a week. Prices end that.
#[tokio::test]
async fn no_flights_waits_a_week() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let id = leg(&store_probe, "LIS", Leg::Outbound).await;
    let first = today() + Days::new(1);
    let empty = Arc::new(AtomicBool::new(true));
    let answer = Arc::clone(&empty);
    let mut h = harness(
        Only {
            family: Some(id),
            destination: None,
        },
        move |f| {
            f.calendar = Box::new(move |_| {
                if answer.load(Ordering::SeqCst) {
                    Fetch::failed(QueryOutcome::NoFlights, 1)
                } else {
                    Fetch::ok(cells(first, 5, |_| 2_000), 3)
                }
            })
        },
    )
    .await;
    let store = h.store().clone();
    // A core family: due daily.
    assert_eq!(intervals(&h, id).await, (None, Some(TimeDelta::hours(24))));

    h.daemon.tick().await.expect("ticks");
    assert_eq!(h.log().calendar.len(), 1);
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].family, id);
    assert_eq!(recent[0].outcome, Some(QueryOutcome::NoFlights));
    assert_eq!(recent[0].page_loads, Some(1));
    assert_eq!(recent[0].cells, 0);
    // The load counts; nothing is a block.
    let budget = h.site_budget("google_flights").await;
    assert_eq!((budget.daily_budget, budget.loads_today), (300, 1));
    let blocks = store
        .blocks_since(Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert!(blocks.is_empty());
    assert_eq!(h.status("google_flights").await.state, "idle");
    let f = store.family(id).await.expect("reads").expect("exists");
    assert!(f.state.last_check.is_some());
    assert_eq!(f.state.last_min_price_minor, None);
    assert_eq!(store.no_flights_families().await.expect("reads"), [id]);

    // A week's wait, without the daily cadence.
    assert_eq!(intervals(&h, id).await, (Some(TimeDelta::days(7)), None));
    let checked = |days_ago: i64, minutes: i64| {
        let store = store.clone();
        async move {
            let mut state = store
                .family(id)
                .await
                .expect("reads")
                .expect("exists")
                .state;
            state.last_check = Some(
                Utc::now() - TimeDelta::days(days_ago)
                    + TimeDelta::minutes(minutes),
            );
            store.save_family_state(id, &state).await.expect("saves");
        }
    };
    h.daemon.tick().await.expect("ticks");
    // Six days on (past the daily maximum): still waiting.
    checked(6, 0).await;
    h.daemon.tick().await.expect("ticks");
    assert_eq!(h.log().calendar.len(), 1);

    // A week on it is checked again; prices bring it back to normal.
    checked(7, -1).await;
    empty.store(false, Ordering::SeqCst);
    h.daemon.tick().await.expect("ticks");
    assert_eq!(h.log().calendar.len(), 2);
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].outcome, Some(QueryOutcome::Ok));
    assert_eq!(recent[0].reason, QueryReason::Overdue);
    assert_eq!(recent[0].cells, 5);
    assert!(store.no_flights_families().await.expect("reads").is_empty());
    assert_eq!(intervals(&h, id).await, (None, Some(TimeDelta::hours(24))));
}

/// The (minimum, maximum) intervals of `id`'s candidate this tick.
async fn intervals(
    h: &Harness,
    id: FamilyId,
) -> (Option<TimeDelta>, Option<TimeDelta>) {
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    let (cands, _) = tm_daemon::candidates::build(&world.families, &cx);
    let c = cands
        .into_iter()
        .find(|c| c.family == id)
        .expect("still a candidate");
    (c.min_interval, c.max_interval)
}

#[tokio::test]
async fn deals_are_found_and_confirmed() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let out_id = leg(&store_probe, "LIS", Leg::Outbound).await;
    let first = today() + Days::new(50);
    let notices = Arc::new(Recorder::default());
    let mut h = harness_with(
        Config {
            notifier: Some(notices.clone()),
            ..test_config(Only {
                family: Some(out_id),
                destination: None,
            })
        },
        move |f| {
            // The outbound drops 40% on two departures.
            f.calendar = Box::new(move |_| {
                Fetch::ok(
                    cells(first, 20, |i| if i < 2 { 600 } else { 1_000 }),
                    3,
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

    h.daemon
        .run_check(out_id, QueryReason::Manual)
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
    // Each confirmed deal is announced once, and then seen (0080).
    let shown = notices.shown();
    assert_eq!(shown.len(), 2);
    for n in &shown {
        assert!(n.title.starts_with("SAO → LIS: R$1,68"), "{n:?}");
        assert!(n.body.contains("via Kiwi.com"), "{n:?}");
    }
    assert!(store.unseen_confirmed().await.expect("reads").is_empty());

    // The same check again finds the same trips: already known.
    h.daemon
        .run_check(out_id, QueryReason::Manual)
        .await
        .expect("checks");
    assert_eq!(h.log().confirm.len(), 2);
    assert_eq!(notices.shown().len(), 2);
}

#[tokio::test]
async fn a_rejected_confirmation_does_not_alert() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let out_id = leg(&store_probe, "MIA", Leg::Outbound).await;
    let first = today() + Days::new(50);
    let notices = Arc::new(Recorder::default());
    let config = Config {
        notifier: Some(notices.clone()),
        ..test_config(Only::default())
    };
    let mut h = harness_with(config, move |f| {
        f.calendar = Box::new(move |_| {
            Fetch::ok(cells(first, 20, |i| if i == 0 { 500 } else { 1_000 }), 3)
        });
        f.confirm = Box::new(|_| {
            Fetch::ok(
                Some(Confirmation {
                    cheapest: Some(1_900),
                    ..Confirmation::default()
                }),
                1,
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
    h.daemon
        .run_check(out_id, QueryReason::Manual)
        .await
        .expect("checks");
    let deals = store
        .list_deals(&DealFilter::default())
        .await
        .expect("lists");
    assert_eq!(deals.len(), 1);
    assert_eq!(deals[0].status, DealStatus::Rejected);
    assert!(store.unseen_confirmed().await.expect("reads").is_empty());
    assert!(notices.shown().is_empty());
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

/// An unwatched destination that Explore prices well under its own
/// Explore history is announced once per drop (0087).
#[tokio::test]
async fn an_unwatched_drop_is_announced_once() {
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
    let price = Arc::new(std::sync::atomic::AtomicI64::new(5_000));
    let notices = Arc::new(Recorder::default());
    let mut h = harness_with(
        Config {
            notifier: Some(notices.clone()),
            ..test_config(Only {
                family: Some(explore),
                destination: None,
            })
        },
        {
            let price = Arc::clone(&price);
            move |f| {
                f.explore = Box::new(move |_| {
                    let jakarta = ExploreDestination {
                        freebase_id: "/m/044rv".into(),
                        airport: Some("CGK".into()),
                        origin_airport: Some("GRU".into()),
                        price: Some(price.load(Ordering::SeqCst)),
                        exact: None,
                        departure: NaiveDate::from_ymd_opt(2026, 11, 3),
                        return_date: NaiveDate::from_ymd_opt(2026, 11, 10),
                        stops: Some(1),
                        duration_minutes: Some(1_500),
                        carrier_code: Some("GA".into()),
                        carrier: Some("Garuda Indonesia".into()),
                        name: Some("Jakarta".into()),
                        country: Some("Indonesia".into()),
                        coordinates: Some((-6.2, 106.85)),
                    };
                    // A watched destination dropping is a deal's
                    // business, not a find's.
                    let lisbon = ExploreDestination {
                        freebase_id: "/m/04llb".into(),
                        airport: Some("LIS".into()),
                        price: Some(100),
                        name: Some("Lisbon".into()),
                        ..jakarta.clone()
                    };
                    Fetch::ok(vec![jakarta, lisbon], 1)
                })
            }
        },
    )
    .await;
    let store = h.store().clone();
    // Ten earlier sweeps priced both at R$7,000, two to seven days ago.
    for k in 0..10 {
        let at = Utc::now() - TimeDelta::hours(48 + 12 * k);
        let q = store
            .start_query(explore, &QueryReason::Manual, at)
            .await
            .expect("starts");
        let past = |freebase: &str, airport: &str| tm_domain::ExploreHint {
            query: q,
            origin: sao.id,
            cabin: Cabin::Economy,
            pos,
            observed_at: at,
            destination_freebase_id: freebase.into(),
            destination_airport: airport.into(),
            price: Money::brl_whole(7_000),
            departure: None,
            return_date: None,
            stops: Some(1),
            duration_minutes: None,
            carrier: None,
            destination_name: None,
            destination_country: None,
            destination_coordinates: None,
        };
        store
            .insert_explore_hints(&[
                past("/m/044rv", "CGK"),
                past("/m/04llb", "LIS"),
            ])
            .await
            .expect("inserts");
    }
    async fn check(h: &mut Harness, explore: FamilyId) {
        h.daemon
            .run_check(explore, QueryReason::Manual)
            .await
            .expect("checks");
    }

    // 29% under: announced.
    check(&mut h, explore).await;
    let shown = notices.shown();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(
        shown[0].title,
        "Found by Explore: SAO → CGK, R$5,000 (\u{2212}29%)"
    );
    assert!(shown[0].body.starts_with("Jakarta · Economy · Nov 3 – 10"));
    // Still down: the same drop, not announced again.
    check(&mut h, explore).await;
    assert_eq!(notices.shown().len(), 1);
    // Back to 7% under: the drop is over.
    price.store(6_500, Ordering::SeqCst);
    check(&mut h, explore).await;
    assert!(
        store
            .open_explore_drops(sao.id)
            .await
            .expect("reads")
            .is_empty()
    );
    // Down again: a new drop, announced again.
    price.store(5_000, Ordering::SeqCst);
    check(&mut h, explore).await;
    assert_eq!(notices.shown().len(), 2);
}

/// Airline and miles sites other than TAP have no reader (0059): a
/// check of one, even a manual one, loads nothing and records no query.
#[tokio::test]
async fn sites_without_a_reader_load_nothing() {
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
        Source::Airline { code: "IB".into() },
        pos,
        FamilyKind::Leg { leg: Leg::Outbound },
    )
    .await;
    let mut h = harness(Only::default(), |_| {}).await;
    let store = h.store().clone();
    store
        .set_miles_login("smiles", LoginState::LoggedIn, Utc::now())
        .await
        .expect("sets");
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    let before = store.recent_queries(100).await.expect("reads").len();
    for id in [miles, airline] {
        let f = world
            .families
            .iter()
            .find(|f| f.id == id)
            .expect("the family");
        assert_eq!(
            tm_daemon::candidates::eligibility(f, &cx),
            Err(tm_daemon::candidates::Skip::Unprobed)
        );
        h.daemon
            .run_check(id, QueryReason::Manual)
            .await
            .expect("checks");
        let state = store.family(id).await.expect("reads").expect("exists");
        assert!(state.state.last_check.is_some());
    }
    {
        let log = h.log();
        assert!(log.calendar.is_empty());
        assert!(log.tap.is_empty());
    }
    assert_eq!(
        store.recent_queries(100).await.expect("reads").len(),
        before
    );
}

#[tokio::test]
async fn tap_legs_run_and_record_cells() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let pos = home(&store_probe).await;
    let lis = route(&store_probe, "LIS").await;
    let outbound = family(
        &store_probe,
        lis,
        Cabin::Economy,
        Source::Airline { code: "TP".into() },
        pos,
        FamilyKind::Leg { leg: Leg::Outbound },
    )
    .await;
    let mut h = harness(Only::default(), |f| {
        f.tap = Box::new(|job| {
            let mut c = cells(job.first, 3, |i| 1_900 + 10 * i as i64);
            for cell in &mut c {
                cell.seller = Some("airline:TP".into());
            }
            Fetch::ok(c, 4)
        });
    })
    .await;
    let store = h.store().clone();
    // TAP's legs are candidates; its round trips are not.
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    let tap_families: Vec<_> = world
        .families
        .iter()
        .filter(|f| f.site == "airline:TP")
        .collect();
    assert!(!tap_families.is_empty());
    for f in &tap_families {
        let ok = tm_daemon::candidates::eligibility(f, &cx).is_ok();
        assert_eq!(ok, matches!(f.key.kind, FamilyKind::Leg { .. }), "{f:?}");
    }
    assert!(
        world
            .families
            .iter()
            .filter(|f| f.site.starts_with("airline:")
                && f.site != "airline:TP"
                || f.site.starts_with("miles:"))
            .filter(|f| !tm_daemon::sites::is_latam(&f.site))
            .all(|f| tm_daemon::candidates::eligibility(f, &cx)
                == Err(tm_daemon::candidates::Skip::Unprobed))
    );

    h.daemon
        .run_check(outbound, QueryReason::Manual)
        .await
        .expect("checks");
    {
        let log = h.log();
        let job = log.tap.last().expect("a TAP job");
        assert_eq!(job.origin, "GRU");
        assert_eq!(job.destination, "LIS");
        assert!(job.url.contains("market=BR&"));
        assert!(job.url.contains("&origin=GRU&destination=LIS&"));
    }
    let since = Utc::now() - TimeDelta::hours(1);
    let obs = store.history(outbound, since).await.expect("reads");
    assert_eq!(obs.len(), 3);
    assert!(
        obs.iter()
            .all(|o| o.seller.as_deref() == Some("airline:TP"))
    );
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].page_loads, Some(4));
    let state = store
        .family(outbound)
        .await
        .expect("reads")
        .expect("exists")
        .state;
    assert_eq!(state.last_min_price_minor, Some(190_000));
}

/// A cookie jar shaped like Chromium's `Default/Network/Cookies` (the
/// columns the daemon reads), with one cookie per `(host, name)` that
/// expires in a month.
async fn cookie_jar(profile: &std::path::Path, cookies: &[(&str, &str)]) {
    use sqlx::{
        Connection as _,
        sqlite::{SqliteConnectOptions, SqliteConnection},
    };
    let network = profile.join("Default").join("Network");
    std::fs::create_dir_all(&network).expect("creates");
    let file = network.join("Cookies");
    let _ = std::fs::remove_file(&file);
    let options = SqliteConnectOptions::new()
        .filename(file)
        .create_if_missing(true);
    let mut c = SqliteConnection::connect_with(&options)
        .await
        .expect("opens");
    sqlx::query(
        "CREATE TABLE cookies (host_key TEXT NOT NULL, name TEXT NOT NULL, \
         expires_utc INTEGER NOT NULL)",
    )
    .execute(&mut c)
    .await
    .expect("creates");
    // Chromium counts microseconds from 1601-01-01.
    let expires = (Utc::now() + TimeDelta::days(30)).timestamp_micros()
        + 11_644_473_600_000_000;
    for (host, name) in cookies {
        sqlx::query("INSERT INTO cookies VALUES (?1, ?2, ?3)")
            .bind(host)
            .bind(name)
            .bind(expires)
            .execute(&mut c)
            .await
            .expect("inserts");
    }
    c.close().await.expect("closes");
}

/// After the login window, the daemon records what the program's own
/// profile holds (`<profile-dir>/miles-smiles`, 0062), not that the
/// window closed (0024, 0064).
#[tokio::test]
async fn closing_the_login_records_the_profiles_session() {
    let profile = tempfile::tempdir().expect("tempdir");
    let mut h = harness_with(
        Config {
            profile_dir: Some(profile.path().into()),
            ..test_config(Only::default())
        },
        |_| {},
    )
    .await;
    let store = h.store().clone();
    let smiles_state = async |store: &Store| {
        store
            .list_miles_programs()
            .await
            .expect("reads")
            .into_iter()
            .find(|m| m.program == "smiles")
            .expect("smiles")
            .login_state
    };
    // A session in the old shared profile (before 0062) is not read.
    cookie_jar(profile.path(), &[("www.smiles.com.br", "token")]).await;
    let smiles_profile =
        tm_browser::profile::site_profile_dir(profile.path(), "miles:smiles");
    assert_eq!(smiles_profile, profile.path().join("miles-smiles"));
    for (cookies, expected, named) in [
        // Only what any visit sets: Akamai and analytics.
        (
            &[(".smiles.com.br", "_abck"), (".smiles.com.br", "_ga")][..],
            LoginState::LoggedOut,
            true,
        ),
        (
            &[(".smiles.com.br", "_abck"), ("www.smiles.com.br", "token")][..],
            LoginState::LoggedIn,
            true,
        ),
        // Another program's session is not Smiles'.
        (
            &[(".latamairlines.com", "token")][..],
            LoginState::LoggedOut,
            true,
        ),
        // An app before 0062 does not name the program.
        (
            &[("www.smiles.com.br", "token")][..],
            LoginState::LoggedIn,
            false,
        ),
    ] {
        cookie_jar(&smiles_profile, cookies).await;
        store
            .enqueue(&RequestKind::OpenLogin {
                program: "smiles".into(),
            })
            .await
            .expect("enqueues");
        h.daemon.process_requests().await.expect("processes");
        assert!(h.daemon.is_paused("miles:smiles"));
        store
            .enqueue(&RequestKind::CloseLogin {
                program: named.then(|| "smiles".to_owned()),
            })
            .await
            .expect("enqueues");
        h.daemon.process_requests().await.expect("processes");
        assert!(h.daemon.paused_sites().is_empty());
        assert_eq!(smiles_state(&store).await, expected, "{cookies:?}");
    }
    // Google Flights never paused.
    assert!(h.log().stops.is_empty());
}

/// Whether two spans overlap in time.
fn overlap(a: &Span, b: &Span) -> bool {
    a.start < b.end && b.start < a.end
}

/// Different sites check at the same time; each site one check at a
/// time; and each counts its own loads (0060, 0062).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sites_check_in_parallel_one_check_each_at_a_time() {
    let mut h = harness_with(
        Config {
            budget: Some(12),
            max_budget: Some(12),
            site_budgets: vec![
                ("airline:TP".into(), SiteLimits { start: 12, max: 12 }),
                ("miles:latam_pass".into(), SiteLimits { start: 12, max: 12 }),
            ],
            ..test_config(Only {
                family: None,
                destination: Some("LIS".into()),
            })
        },
        |a| {
            a.calendar = Box::new(|_| Fetch::ok(Vec::new(), 3));
            a.tap = Box::new(|_| Fetch::ok(Vec::new(), 2));
            a.delay = Duration::from_millis(150);
        },
    )
    .await;
    h.daemon.tick().await.expect("ticks");
    let spans = h.log().spans.clone();
    let google: Vec<&Span> = spans
        .iter()
        .filter(|s| s.site == "google_flights")
        .collect();
    let tap: Vec<&Span> =
        spans.iter().filter(|s| s.site == "airline:TP").collect();
    assert!(!google.is_empty() && !tap.is_empty(), "{spans:?}");
    // The two sites' checks ran at the same time...
    assert!(
        google.iter().any(|g| tap.iter().any(|t| overlap(g, t))),
        "no overlap: {spans:?}"
    );
    // ...but never two of the same site.
    for site in [&google, &tap] {
        for (i, a) in site.iter().enumerate() {
            for b in &site[i + 1..] {
                assert!(!overlap(a, b), "{a:?} and {b:?}");
            }
        }
    }
    // Each site counted its own loads, against its own budget; LATAM's
    // cash checks count on LATAM Pass's (0084).
    let latam: Vec<&Span> = spans
        .iter()
        .filter(|s| s.site == "miles:latam_pass")
        .collect();
    let google_budget = h.site_budget("google_flights").await;
    let tap_budget = h.site_budget("airline:TP").await;
    let latam_budget = h.site_budget("miles:latam_pass").await;
    assert_eq!(google_budget.loads_today, 3 * google.len() as u32);
    assert_eq!(tap_budget.loads_today, 2 * tap.len() as u32);
    assert_eq!(latam_budget.loads_today, latam.len() as u32);
    assert_eq!(
        (google_budget.daily_budget, tap_budget.daily_budget),
        (12, 12)
    );
    assert!(
        h.store()
            .site_budget("airline:LA")
            .await
            .expect("reads")
            .is_none()
    );
    let beat = h
        .store()
        .heartbeat()
        .await
        .expect("reads")
        .expect("a heartbeat");
    assert_eq!(
        beat.loads_today,
        google_budget.loads_today
            + tap_budget.loads_today
            + latam_budget.loads_today
    );
    assert_eq!(beat.budget, 36);
    // Each site's browser is its own; LATAM's two sites have one.
    let starts = h.log().starts.clone();
    assert_eq!(starts.iter().filter(|s| *s == "google_flights").count(), 1);
    assert_eq!(starts.iter().filter(|s| *s == "airline:TP").count(), 1);
    assert!(starts.iter().filter(|s| *s == "miles:latam_pass").count() <= 1);
    assert!(!starts.iter().any(|s| s == "airline:LA"));
}

/// The most spans running at any one instant.
fn most_at_once(spans: &[&Span]) -> usize {
    spans
        .iter()
        .map(|a| {
            spans
                .iter()
                .filter(|b| b.start <= a.start && a.start < b.end)
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// With three lanes, each site runs up to three checks at once and
/// never more; every check is recorded and counted (0072).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_site_runs_up_to_three_checks_at_once() {
    let mut h = harness_with(
        Config {
            parallel: 3,
            // New families are charged a full check (8 loads, 0078).
            budget: Some(80),
            max_budget: Some(80),
            site_budgets: vec![(
                "airline:TP".into(),
                SiteLimits { start: 40, max: 40 },
            )],
            ..test_config(Only {
                family: None,
                destination: Some("LIS".into()),
            })
        },
        |a| {
            a.calendar = Box::new(|_| Fetch::ok(Vec::new(), 3));
            a.tap = Box::new(|_| Fetch::ok(Vec::new(), 2));
            a.delay = Duration::from_millis(150);
        },
    )
    .await;
    h.daemon.tick().await.expect("ticks");
    let spans = h.log().spans.clone();
    for (site, loads) in [("google_flights", 3), ("airline:TP", 2)] {
        let ran: Vec<&Span> = spans.iter().filter(|s| s.site == site).collect();
        assert!(ran.len() > 3, "{site}: {spans:?}");
        assert_eq!(most_at_once(&ran), 3, "{site}: {spans:?}");
        let budget = h.site_budget(site).await;
        assert_eq!(budget.loads_today, loads * ran.len() as u32, "{site}");
    }
    // Every check finished, once each, and no site says it still checks.
    let queries = h.store().recent_queries(100).await.expect("reads");
    assert_eq!(queries.len(), spans.len());
    assert!(queries.iter().all(|q| q.finished_at.is_some()));
    let mut families: Vec<FamilyId> =
        queries.iter().map(|q| q.family).collect();
    families.sort();
    families.dedup();
    assert_eq!(families.len(), queries.len(), "a family checked twice");
    for site in ["google_flights", "airline:TP"] {
        let status = h.status(site).await;
        assert!(status.checks.is_empty(), "{status:?}");
        assert_eq!(status.state, "idle");
    }
}

/// A block on TAP halves and cools TAP alone; Google Flights keeps its
/// budget and keeps checking (0060).
#[tokio::test]
async fn a_block_on_one_site_leaves_the_others_alone() {
    let mut h = harness_with(
        Config {
            budget: Some(20),
            max_budget: Some(20),
            ..test_config(Only {
                family: None,
                destination: Some("LIS".into()),
            })
        },
        |a| {
            a.tap = Box::new(|_| {
                Fetch::failed(
                    QueryOutcome::Blocked {
                        detail: "captcha".into(),
                    },
                    1,
                )
            });
        },
    )
    .await;
    h.daemon.tick().await.expect("ticks");
    assert_eq!(h.log().tap.len(), 1, "TAP stops at its first block");
    let calendars = h.log().calendar.len();
    assert!(calendars > 1);
    let blocks = h
        .store()
        .blocks_since(Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].site, "airline:TP");
    let tap = h.site_budget("airline:TP").await;
    assert_eq!(tap.daily_budget, 600, "halved from 1,200 (0088)");
    let google = h.site_budget("google_flights").await;
    assert_eq!(google.daily_budget, 20);
    assert_eq!(h.status("airline:TP").await.state, "cooling down");
    assert_eq!(h.status("google_flights").await.state, "idle");
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    assert!(cx.cooling.contains("airline:TP"));
    assert!(!cx.cooling.contains("google_flights"));

    // The next tick: TAP waits out its cooldown, Google Flights runs.
    h.store()
        .enqueue(&RequestKind::CheckRoute {
            route: route(h.store(), "LIS").await,
        })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await;
    assert_eq!(h.log().tap.len(), 1);
    assert!(h.log().calendar.len() > calendars);
}

/// The daemon's loop runs each site on its own timer; a shutdown stops
/// every site's worker and closes every browser.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_stops_every_site() {
    let config = Config {
        once: false,
        tick: Duration::from_millis(200),
        request_poll: Duration::from_millis(50),
        // A tick gets the day's budget over the seconds left in it, so
        // just after midnight 100,000 loads came to about one a tick:
        // too little for a check, and the test failed by the clock.
        budget: Some(100_000_000),
        max_budget: Some(100_000_000),
        site_budgets: vec![(
            "airline:TP".into(),
            SiteLimits {
                start: 100_000_000,
                max: 100_000_000,
            },
        )],
        ..test_config(Only {
            family: None,
            destination: Some("LIS".into()),
        })
    };
    let (mut daemon, log, stop, store) =
        daemon(config, Answers::default()).await;
    let running = tokio::spawn(async move {
        let result = daemon.run().await;
        (daemon, result)
    });
    let started = async {
        loop {
            {
                let log = log.lock().expect("the log lock");
                if log.starts.iter().any(|s| s == "google_flights")
                    && log.starts.iter().any(|s| s == "airline:TP")
                    && !log.tap.is_empty()
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    // Generous: with the whole suite running side by side (now with
    // LATAM's tests too) a site can take well over 20 s to get going.
    tokio::time::timeout(Duration::from_secs(90), started)
        .await
        .expect("both sites start checking");
    stop.send(true).expect("signals");
    let (daemon, result) =
        tokio::time::timeout(Duration::from_secs(60), running)
            .await
            .expect("stops in time")
            .expect("joins");
    result.expect("runs");
    assert!(daemon.sites().is_empty());
    {
        let log = log.lock().expect("the log lock");
        for site in ["google_flights", "airline:TP"] {
            let starts = log.starts.iter().filter(|s| *s == site).count();
            let stops = log.stops.iter().filter(|s| *s == site).count();
            assert_eq!(starts, stops, "{site}: every browser closed");
        }
    }
    let statuses = store.site_statuses().await.expect("reads");
    assert!(statuses.len() >= 2);
    for status in &statuses {
        assert_eq!(status.state, "stopped", "{status:?}");
        assert!(status.checks.is_empty());
    }
    let beat = store
        .heartbeat()
        .await
        .expect("reads")
        .expect("a heartbeat");
    assert_eq!(beat.state, "stopped");
}

// ---------------------------------------------------------------------
// LATAM and LATAM Pass (0084, 0085)
// ---------------------------------------------------------------------

/// An offers response from `docs/research/sites/fixtures/`.
fn latam_offers(name: &str) -> tm_daemon::sites::latam::Offers {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/research/sites/fixtures")
        .join(name);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    tm_daemon::sites::latam::parse_offers(&body)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The real pages' cells for a job, from the GRU → LIS captures: the
/// award response for an award job, the cash one otherwise, for the
/// job's date.
fn latam_cells(job: &LatamJob) -> Vec<Cell> {
    let name = if job.award {
        "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json"
    } else {
        "latam/offers-search-ow-gru-lis-2026-11-10-br-headed-warm.json"
    };
    tm_daemon::sources::latam::leg_cells(
        &latam_offers(name),
        job.cabin,
        job.departure,
        &job.seller,
    )
}

async fn latam_family(
    store: &Store,
    dest: &str,
    source: Source,
    kind: FamilyKind,
) -> FamilyId {
    let r = route(store, dest).await;
    let pos = home(store).await;
    family(store, r, Cabin::Economy, source, pos, kind).await
}

fn pass() -> Source {
    Source::Miles {
        program: "latam_pass".into(),
    }
}

fn latam_airline() -> Source {
    Source::Airline { code: "LA".into() }
}

const OUTBOUND: FamilyKind = FamilyKind::Leg { leg: Leg::Outbound };

async fn log_in(store: &Store) {
    store
        .set_miles_login("latam_pass", LoginState::LoggedIn, Utc::now())
        .await
        .expect("sets");
}

async fn latam_pass(store: &Store) -> tm_store::MilesProgram {
    store
        .list_miles_programs()
        .await
        .expect("reads")
        .into_iter()
        .find(|m| m.program == "latam_pass")
        .expect("latam_pass")
}

/// LATAM's cash and LATAM Pass's awards run on one worker, named for
/// LATAM Pass, with its profile, browser, budget and pace (0084): never
/// two Chromiums on `miles-latam_pass`, never two LATAM checks at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn latam_cash_and_awards_share_one_worker() {
    let mut h = harness_with(
        Config {
            parallel: 3,
            // Google Flights all but idle: the tick is LATAM's.
            budget: Some(1),
            max_budget: Some(1),
            site_budgets: vec![(
                "miles:latam_pass".into(),
                SiteLimits { start: 80, max: 80 },
            )],
            ..test_config(Only {
                family: None,
                destination: Some("LIS".into()),
            })
        },
        |a| {
            a.latam = Box::new(|job| {
                Fetch::ok(
                    LatamFound {
                        cells: latam_cells(job),
                        ..LatamFound::default()
                    },
                    1,
                )
            });
            a.delay = Duration::from_millis(60);
        },
    )
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    let cash = latam_family(&store, "LIS", latam_airline(), OUTBOUND).await;
    let award = latam_family(&store, "LIS", pass(), OUTBOUND).await;
    assert!(!h.daemon.sites().iter().any(|s| s == "airline:LA"));

    h.daemon
        .run_check(cash, QueryReason::Manual)
        .await
        .expect("checks");
    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    h.daemon.tick().await.expect("ticks");

    {
        let log = h.log();
        let starts: Vec<&String> =
            log.starts.iter().filter(|s| s.contains("la")).collect();
        assert!(
            starts.iter().all(|s| *s == "miles:latam_pass"),
            "{starts:?}"
        );
        // One browser: started once, never twice without a stop between.
        let latam_starts =
            log.starts.iter().filter(|s| *s == "miles:latam_pass");
        let latam_stops = log.stops.iter().filter(|s| *s == "miles:latam_pass");
        assert!(latam_starts.count() <= latam_stops.count() + 1);
        // One check at a time, though the other sites run three.
        let spans: Vec<&Span> = log
            .spans
            .iter()
            .filter(|s| s.site == "miles:latam_pass")
            .collect();
        assert!(spans.len() >= 2, "{spans:?}");
        assert_eq!(most_at_once(&spans), 1);
        // Cash and award jobs on LATAM's results page.
        let cash_job = log.latam.iter().find(|j| !j.award).expect("a cash job");
        assert_eq!(cash_job.seller, "airline:LA");
        assert_eq!(
            (cash_job.origin.as_str(), cash_job.destination.as_str()),
            ("GRU", "LIS")
        );
        assert!(cash_job.url.contains("redemption=false"));
        assert_eq!(cash_job.strip.max_clicks, 0);
        let award_job =
            log.latam.iter().find(|j| j.award).expect("an award job");
        assert_eq!(award_job.seller, "miles:latam_pass");
        assert!(award_job.url.contains("redemption=true"));
    }
    // Both counted on LATAM Pass's budget; none on airline:LA's.
    let budget = h.site_budget("miles:latam_pass").await;
    assert_eq!(budget.loads_today, h.log().latam.len() as u32);
    assert!(
        store
            .site_budget("airline:LA")
            .await
            .expect("reads")
            .is_none()
    );

    // Cash: one cell per date, the cheapest brand, with its itinerary.
    let since = Utc::now() - TimeDelta::hours(1);
    let cash_obs = store.history(cash, since).await.expect("reads");
    let first_cash = cash_obs
        .iter()
        .find(|o| o.query == cash_obs[0].query)
        .expect("a cash cell");
    assert_eq!(first_cash.price, Some(Money::new(190_743, Currency::brl())));
    assert_eq!(
        first_cash.flights.as_deref(),
        Some("LA8146@2026-11-10T17:40")
    );
    assert_eq!(first_cash.stops, Some(0));
    assert_eq!(first_cash.seller.as_deref(), Some("airline:LA"));
    assert!(first_cash.award.is_none());
}

/// Award cells keep everything the model expects (0083): miles, taxes
/// as the price, the itinerary, stops, and the brand's fare, basis and
/// seats; one per itinerary × brand of the cabin.
#[tokio::test]
async fn award_cells_keep_every_field() {
    let mut h = harness(Only::default(), |a| {
        a.latam = Box::new(|job| {
            Fetch::ok(
                LatamFound {
                    cells: latam_cells(job),
                    ..LatamFound::default()
                },
                1,
            )
        });
    })
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    let award = latam_family(&store, "LIS", pass(), OUTBOUND).await;
    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    let obs = store
        .history(award, Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    let fixture = latam_offers(
        "latam-pass/offers-search-redemption-ow-gru-lis-2026-11-10-br.json",
    );
    let economy: usize = fixture
        .itineraries
        .iter()
        .map(|i| {
            i.fares
                .iter()
                .filter(|f| f.cabin == Some(Cabin::Economy))
                .count()
        })
        .sum();
    assert_eq!(obs.len(), economy);
    let light = obs
        .iter()
        .find(|o| {
            o.flights.as_deref() == Some("LA8146@2026-11-10T17:40")
                && o.award.as_ref().and_then(|a| a.brand.as_deref())
                    == Some("LIGHT")
        })
        .expect("LA8146 LIGHT");
    assert_eq!(light.miles, Some(63_848));
    assert_eq!(light.price, Some(Money::new(6861, Currency::brl())));
    assert_eq!(light.stops, Some(0));
    assert_eq!(light.return_date, None);
    let detail = light.award.as_ref().expect("an award");
    assert_eq!(detail.fare, Some(Money::new(183_882, Currency::brl())));
    assert_eq!(detail.fare_basis.as_deref(), Some("AKTSLZ6E"));
    assert_eq!(
        detail.seats,
        Some(tm_domain::Seats {
            count: 9,
            low: false
        })
    );
    // The family's minimum is in miles.
    let f = store.family(award).await.expect("reads").expect("exists");
    assert_eq!(f.state.last_min_price_minor, Some(63_848));
    // Its value of a mile is measured (0083): R$28.80 per 1,000.
    let rates = store
        .award_rates("latam_pass", Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert_eq!(rates.len(), 1);
    assert!((rates[0].brl_per_1000 - 28.8).abs() < 0.05, "{rates:?}");
}

/// LATAM's round trips wait (0090): the return's price comes only after
/// a click on an outbound fare, which Akamai probably denies. The
/// scheduler skips them with a reason, and a manual check loads
/// nothing.
#[tokio::test]
async fn latam_round_trips_wait_for_a_verified_click() {
    let mut h = harness(Only::default(), |a| {
        a.latam = Box::new(|job| {
            Fetch::ok(
                LatamFound {
                    cells: latam_cells(job),
                    ..LatamFound::default()
                },
                1,
            )
        });
    })
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    let award =
        latam_family(&store, "MIA", pass(), FamilyKind::RoundTrip { days: 7 })
            .await;
    let cash = latam_family(
        &store,
        "MIA",
        latam_airline(),
        FamilyKind::RoundTrip { days: 7 },
    )
    .await;
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    for id in [award, cash] {
        let f = world.families.iter().find(|f| f.id == id).expect("family");
        assert_eq!(
            tm_daemon::candidates::eligibility(f, &cx),
            Err(tm_daemon::candidates::Skip::Unsupported)
        );
        assert!(
            tm_daemon::sites::unsupported(&f.key, &Default::default())
                .is_some_and(|why| why.contains("click")),
        );
    }
    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    // Nor does a check asked for by hand, without `--latam-round-trips`.
    let check = store
        .enqueue(&RequestKind::CheckFamily { family: award })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await;
    let r = store.request(check).await.expect("reads").expect("exists");
    assert_eq!(r.status, RequestStatus::Failed);
    assert!(h.log().latam.is_empty(), "{:?}", h.log().latam);
}

/// Under `--latam-round-trips`, a manual check of one LATAM round-trip
/// family runs, for the live run that verifies the outbound click
/// (0090); the scheduler still skips them.
#[tokio::test]
async fn one_latam_round_trip_runs_by_hand_under_the_flag() {
    let mut config = test_config(Only::default());
    config.jobs.latam_round_trips = true;
    let mut h = harness_with(config, |a| {
        a.latam = Box::new(|job| {
            Fetch::ok(
                LatamFound {
                    cells: latam_cells(job),
                    ..LatamFound::default()
                },
                2,
            )
        });
    })
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    let round = FamilyKind::RoundTrip { days: 7 };
    let award = latam_family(&store, "MIA", pass(), round).await;
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    let f = world
        .families
        .iter()
        .find(|f| f.id == award)
        .expect("family");
    assert_eq!(
        tm_daemon::candidates::eligibility(f, &cx),
        Err(tm_daemon::candidates::Skip::Unsupported)
    );
    h.daemon.tick().await.expect("ticks");
    assert!(
        h.log().latam.iter().all(|j| j.return_date.is_none()),
        "{:?}",
        h.log().latam
    );
    let check = store
        .enqueue(&RequestKind::CheckFamily { family: award })
        .await
        .expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await;
    let r = store.request(check).await.expect("reads").expect("exists");
    assert_eq!(r.status, RequestStatus::Done, "{r:?}");
    let trips: Vec<LatamJob> = h
        .log()
        .latam
        .iter()
        .filter(|j| j.return_date.is_some())
        .cloned()
        .collect();
    assert_eq!(trips.len(), 1, "{trips:?}");
    let trip = &trips[0];
    assert!(trip.award);
    assert_eq!(trip.return_date, Some(trip.departure + Days::new(7)));
    assert_eq!(trip.strip.max_clicks, 0);
}

/// An award check reads one date, one page load (0090): the cheapest
/// cash day within 15% of the leg's cheapest not read in three days,
/// then the next; once those were read, a spot check past the next few
/// days.
#[tokio::test]
async fn award_checks_read_one_low_cash_date_each() {
    let first = today() + Days::new(40);
    let mut h = harness(Only::default(), |a| {
        a.latam = Box::new(|job| {
            Fetch::ok(
                LatamFound {
                    cells: latam_cells(job),
                    ..LatamFound::default()
                },
                1,
            )
        });
    })
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    // Cheapest trips only: no weekly spot check.
    store
        .save_miles_settings(
            "latam_pass",
            &tm_domain::ProgramSettings {
                active: true,
                dates: tm_domain::AwardDates::CheapTrips,
                ..tm_domain::ProgramSettings::default()
            },
        )
        .await
        .expect("saves");
    // Google Flights' outbound: R$1,000 a day, with a dip around
    // first + 5.
    let google = leg(&store, "LIS", Leg::Outbound).await;
    let q = store
        .start_query(google, &QueryReason::Random, Utc::now())
        .await
        .expect("starts");
    let obs: Vec<Observation> = (0..20)
        .map(|i| Observation {
            id: None,
            query: q,
            family: google,
            observed_at: Utc::now(),
            departure: first + Days::new(i),
            return_date: None,
            price: Some(Money::brl_whole(match i {
                5 => 800,
                6 => 850,
                4 => 900,
                _ => 1_000,
            })),
            miles: None,
            carrier: None,
            stops: None,
            duration_minutes: None,
            seller: None,
            flights: None,
            award: None,
        })
        .collect();
    store.insert_observations(&obs).await.expect("inserts");
    store
        .finish_query(q, &QueryOutcome::Ok, 8, Utc::now())
        .await
        .expect("finishes");

    let award = latam_family(&store, "LIS", pass(), OUTBOUND).await;
    let mut read = Vec::new();
    for _ in 0..4 {
        h.daemon
            .run_check(award, QueryReason::Manual)
            .await
            .expect("checks");
        let job = h.log().latam.last().cloned().expect("a job");
        assert!(job.url.contains(&format!("outbound={}T", job.departure)));
        // The strip is off by default: nothing to click.
        assert_eq!(job.strip.max_clicks, 0);
        read.push(job.departure);
    }
    // The low days, cheapest first (R$800, R$850, R$900: all within 15%
    // of R$800); R$1,000 is not low, so the fourth is a spot check, far
    // from the days just read and past the next three days.
    assert_eq!(&read[..3], [5, 6, 4].map(|d| first + Days::new(d)));
    assert!(read[3] > today() + Days::new(3), "{read:?}");
    assert!(!read[..3].contains(&read[3]), "{read:?}");
    // One page load each.
    assert_eq!(h.site_budget("miles:latam_pass").await.loads_today, 4);
    let recent = store.recent_queries(4).await.expect("reads");
    assert!(recent.iter().all(|q| q.page_loads == Some(1)), "{recent:?}");
}

/// With the strip switched on (`--latam-strip`, 0090), an award check
/// reads the cheapest cash day not read lately and clicks the strip days
/// whose cash is low, each a load counted against LATAM's budget, never
/// more than the budget has left (0084, 0085).
#[tokio::test]
async fn strip_clicks_follow_cash_and_the_budget() {
    let first = today() + Days::new(40);
    let mut h = harness_with(
        Config {
            site_budgets: vec![(
                "miles:latam_pass".into(),
                SiteLimits { start: 4, max: 4 },
            )],
            latam_strip: true,
            ..test_config(Only::default())
        },
        |a| {
            a.latam = Box::new(|job| {
                let calendar = std::collections::BTreeMap::new();
                let shown =
                    tm_daemon::sites::latam::dates::strip_days(job.departure);
                let picks = job.strip.clicks(job.departure, &shown, &calendar);
                let mut cells = Vec::new();
                for day in std::iter::once(job.departure).chain(picks.clone()) {
                    let mut j = job.clone();
                    j.departure = day;
                    cells.extend(latam_cells(&j));
                }
                let clicks = picks.len() as u32;
                Fetch::ok(
                    LatamFound {
                        cells,
                        clicks,
                        ..LatamFound::default()
                    },
                    1 + clicks,
                )
            });
        },
    )
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    // Cheapest trips only: no weekly spot check.
    store
        .save_miles_settings(
            "latam_pass",
            &tm_domain::ProgramSettings {
                active: true,
                dates: tm_domain::AwardDates::CheapTrips,
                ..tm_domain::ProgramSettings::default()
            },
        )
        .await
        .expect("saves");
    // Google Flights' outbound: R$1,000 a day, with a dip around
    // first + 5.
    let google = leg(&store, "LIS", Leg::Outbound).await;
    let q = store
        .start_query(google, &QueryReason::Random, Utc::now())
        .await
        .expect("starts");
    let obs: Vec<Observation> = (0..20)
        .map(|i| Observation {
            id: None,
            query: q,
            family: google,
            observed_at: Utc::now(),
            departure: first + Days::new(i),
            return_date: None,
            price: Some(Money::brl_whole(match i {
                5 => 800,
                6 => 850,
                4 => 900,
                _ => 1_000,
            })),
            miles: None,
            carrier: None,
            stops: None,
            duration_minutes: None,
            seller: None,
            flights: None,
            award: None,
        })
        .collect();
    store.insert_observations(&obs).await.expect("inserts");
    store
        .finish_query(q, &QueryOutcome::Ok, 8, Utc::now())
        .await
        .expect("finishes");

    let award = latam_family(&store, "LIS", pass(), OUTBOUND).await;
    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    let job = h.log().latam.last().cloned().expect("a job");
    assert_eq!(job.departure, first + Days::new(5));
    // Four loads a day: the page, then at most three clicks.
    assert_eq!(job.strip.max_clicks, 3);
    let picks = job.strip.clicks(
        job.departure,
        &tm_daemon::sites::latam::dates::strip_days(job.departure),
        &std::collections::BTreeMap::new(),
    );
    assert_eq!(picks, vec![first + Days::new(6), first + Days::new(4)]);
    let budget = h.site_budget("miles:latam_pass").await;
    assert_eq!(budget.loads_today, 3);
    let read: std::collections::BTreeSet<NaiveDate> = store
        .history(award, Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads")
        .into_iter()
        .map(|o| o.departure)
        .collect();
    assert_eq!(
        read,
        [4, 5, 6]
            .map(|d| first + Days::new(d))
            .into_iter()
            .collect()
    );

    // The next check leaves the days just read: the next cheapest.
    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    let job = h.log().latam.last().cloned().expect("a job");
    assert!(
        job.departure < first + Days::new(4)
            || job.departure > first + Days::new(6)
    );
    // One load left today: no click.
    assert_eq!(job.strip.max_clicks, 0);
}

/// A check whose page was read and whose next step (a strip click's
/// search, a round trip's return) was denied keeps what it read (0090):
/// the cells are recorded, and the denial is still a block that backs
/// LATAM's worker off.
#[tokio::test]
async fn a_block_after_the_page_keeps_what_was_read() {
    let quiet = Config {
        budget: Some(1),
        max_budget: Some(1),
        ..test_config(Only::default())
    };
    let mut h = harness_with(quiet, |a| {
        a.latam = Box::new(|job| {
            // The page read, then the second step's search got 403.
            Fetch::partial(
                QueryOutcome::Blocked {
                    detail: "Akamai denied the offers search (403)".into(),
                },
                LatamFound {
                    cells: latam_cells(job),
                    calendar: [(job.departure, 200_141)].into(),
                    session: None,
                    clicks: 1,
                },
                2,
            )
        });
    })
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    let award = latam_family(&store, "LIS", pass(), OUTBOUND).await;
    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    let job = h.log().latam.last().cloned().expect("a job");
    // The cells are recorded under the blocked query.
    let obs = store
        .history(award, Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert_eq!(obs.len(), latam_cells(&job).len());
    assert!(obs.iter().any(|o| o.miles == Some(63_848)));
    let recent = store.recent_queries(1).await.expect("reads");
    assert!(
        matches!(recent[0].outcome, Some(QueryOutcome::Blocked { .. })),
        "{recent:?}"
    );
    assert_eq!(recent[0].page_loads, Some(2));
    assert_eq!(h.site_budget("miles:latam_pass").await.loads_today, 2);
    // The family learned its minimum from them.
    let f = store.family(award).await.expect("reads").expect("exists");
    assert_eq!(f.state.last_min_price_minor, Some(63_848));
    // And the block backs off: recorded, halved, cooling down.
    let blocks = store
        .blocks_since(Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].site, "miles:latam_pass");
    assert_eq!(h.site_budget("miles:latam_pass").await.daily_budget, 50);
    assert_eq!(h.status("miles:latam_pass").await.state, "cooling down");
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    let f = world
        .families
        .iter()
        .find(|f| f.id == award)
        .expect("family");
    assert_eq!(
        tm_daemon::candidates::eligibility(f, &cx),
        Err(tm_daemon::candidates::Skip::CoolingDown)
    );
    // The login is not touched by a block.
    assert_eq!(latam_pass(&store).await.login_state, LoginState::LoggedIn);
}

/// A login wall ends LATAM Pass's session: the program is expired, its
/// award families wait, and the user is told once (0085). The session
/// cookie each check reads is recorded; a missing one ends the session
/// too.
#[tokio::test]
async fn a_login_wall_expires_latam_pass_once() {
    let notices = Arc::new(Recorder::default());
    let walled = Arc::new(AtomicBool::new(false));
    let until = Utc::now() + TimeDelta::days(5);
    let mut h = harness_with(
        Config {
            notifier: Some(notices.clone()),
            ..test_config(Only::default())
        },
        {
            let walled = Arc::clone(&walled);
            move |a| {
                a.latam = Box::new(move |job| {
                    if walled.load(Ordering::SeqCst) {
                        return Fetch::failed(QueryOutcome::LoggedOut, 1);
                    }
                    Fetch::ok(
                        LatamFound {
                            cells: latam_cells(job),
                            session: Some(Some(until)),
                            ..LatamFound::default()
                        },
                        1,
                    )
                });
            }
        },
    )
    .await;
    let store = h.store().clone();
    let cash = latam_family(&store, "LIS", latam_airline(), OUTBOUND).await;
    let award = latam_family(&store, "LIS", pass(), OUTBOUND).await;
    // Not logged in: award families wait; cash runs.
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    let skip = |id| {
        let f = world.families.iter().find(|f| f.id == id).expect("family");
        tm_daemon::candidates::eligibility(f, &cx)
    };
    assert_eq!(skip(award), Err(tm_daemon::candidates::Skip::LoggedOut));
    assert_eq!(skip(cash), Ok(()));

    // A cash check finds a live session cookie: logged in, until when.
    h.daemon
        .run_check(cash, QueryReason::Manual)
        .await
        .expect("checks");
    let program = latam_pass(&store).await;
    assert_eq!(program.login_state, LoginState::LoggedIn);
    let recorded = program.session_until.expect("a session end");
    assert!((recorded - until).num_seconds().abs() <= 1);

    // The award page shows the login: expired, and one notice.
    walled.store(true, Ordering::SeqCst);
    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    assert_eq!(latam_pass(&store).await.login_state, LoginState::Expired);
    // A second wall on the cash site: still one notice.
    h.daemon
        .run_check(cash, QueryReason::Manual)
        .await
        .expect("checks");
    let shown = notices.shown();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title, "LATAM Pass: log in again");
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    let f = world
        .families
        .iter()
        .find(|f| f.id == award)
        .expect("family");
    assert_eq!(
        tm_daemon::candidates::eligibility(f, &cx),
        Err(tm_daemon::candidates::Skip::LoggedOut)
    );
    // Nothing was a block.
    let blocks = store
        .blocks_since(Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert!(blocks.is_empty());
}

/// A check whose session cookie is gone ends a live login the same way
/// (0082, 0085).
#[tokio::test]
async fn a_missing_session_cookie_expires_latam_pass() {
    let notices = Arc::new(Recorder::default());
    let mut h = harness_with(
        Config {
            notifier: Some(notices.clone()),
            ..test_config(Only::default())
        },
        |a| {
            a.latam = Box::new(|job| {
                Fetch::ok(
                    LatamFound {
                        cells: latam_cells(job),
                        session: Some(None),
                        ..LatamFound::default()
                    },
                    1,
                )
            });
        },
    )
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    let cash = latam_family(&store, "LIS", latam_airline(), OUTBOUND).await;
    h.daemon
        .run_check(cash, QueryReason::Manual)
        .await
        .expect("checks");
    let program = latam_pass(&store).await;
    assert_eq!(program.login_state, LoginState::Expired);
    assert_eq!(program.session_until, None);
    assert_eq!(notices.shown().len(), 1);
}

/// Akamai's denial is a block of LATAM's one worker (0039, 0060): its
/// budget halves, it cools down, and both LATAM sites wait.
#[tokio::test]
async fn an_akamai_denial_blocks_both_latam_sites() {
    let quiet = Config {
        budget: Some(1),
        max_budget: Some(1),
        ..test_config(Only::default())
    };
    let mut h = harness_with(quiet, |a| {
        a.latam = Box::new(|_| {
            Fetch::failed(
                QueryOutcome::Blocked {
                    detail: "Akamai denied the offers search (403)".into(),
                },
                1,
            )
        });
    })
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    let cash = latam_family(&store, "LIS", latam_airline(), OUTBOUND).await;
    let award = latam_family(&store, "LIS", pass(), OUTBOUND).await;
    h.daemon
        .run_check(cash, QueryReason::Manual)
        .await
        .expect("checks");
    let blocks = store
        .blocks_since(Utc::now() - TimeDelta::hours(1))
        .await
        .expect("reads");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].site, "miles:latam_pass");
    // Halved from its start of 80, to the floor of 50 (0039, 0090).
    let budget = h.site_budget("miles:latam_pass").await;
    assert_eq!(budget.daily_budget, 50, "{budget:?}");
    assert_eq!(h.status("miles:latam_pass").await.state, "cooling down");
    let world = h.daemon.world().await.expect("loads");
    let cx = h.daemon.context(&world, Utc::now()).await.expect("builds");
    for id in [cash, award] {
        let f = world.families.iter().find(|f| f.id == id).expect("family");
        assert_eq!(
            tm_daemon::candidates::eligibility(f, &cx),
            Err(tm_daemon::candidates::Skip::CoolingDown)
        );
    }
    // A tick while cooling down loads nothing more.
    h.daemon.tick().await.expect("ticks");
    assert_eq!(h.log().latam.len(), 1);
    // The login is not touched by a block.
    assert_eq!(latam_pass(&store).await.login_state, LoginState::LoggedIn);
}

/// One award cell, as the reader writes it.
fn award_cell(date: NaiveDate, miles: i64) -> Cell {
    let mut c =
        Cell::priced(date, None, Some(Money::new(6861, Currency::brl())));
    c.miles = Some(miles);
    c.seller = Some("miles:latam_pass".into());
    c.stops = Some(0);
    c.flights = Some(format!("LA8146@{date}T17:40"));
    c.award = Some(tm_domain::AwardDetail {
        fare: Some(Money::new(
            (miles as f64 * 2.88).round() as i64,
            Currency::brl(),
        )),
        brand: Some("LIGHT".into()),
        fare_basis: Some("AKTSLZ6E".into()),
        seats: Some(tm_domain::Seats {
            count: 2,
            low: true,
        }),
    });
    c
}

/// An award well under its own series' baseline becomes a candidate,
/// is confirmed by reading its date again, and is announced in miles
/// (0083, 0085).
#[tokio::test]
async fn award_deals_are_confirmed_by_a_fresh_read_and_announced() {
    let store_probe = Store::open_in_memory().await.expect("opens");
    store_probe.seed_defaults().await.expect("seeds");
    let award = latam_family(&store_probe, "LIS", pass(), OUTBOUND).await;
    let first = today() + Days::new(50);
    let notices = Arc::new(Recorder::default());
    let mut h = harness_with(
        Config {
            notifier: Some(notices.clone()),
            ..test_config(Only {
                family: Some(award),
                destination: None,
            })
        },
        move |a| {
            // 40% under the usual 100,000 miles on `first`.
            a.latam = Box::new(move |_| {
                Fetch::ok(
                    LatamFound {
                        cells: vec![award_cell(first, 60_000)],
                        ..LatamFound::default()
                    },
                    1,
                )
            });
        },
    )
    .await;
    let store = h.store().clone();
    log_in(&store).await;
    // Five weeks of 100,000 miles.
    let now = Utc::now();
    for k in (1..=36).rev() {
        let at = now - TimeDelta::days(k) - TimeDelta::hours(1);
        let q = store
            .start_query(award, &QueryReason::Random, at)
            .await
            .expect("starts");
        let obs: Vec<Observation> = (0..20)
            .map(|i| {
                let cell = award_cell(first + Days::new(i), 100_000 + k * 100);
                Observation {
                    id: None,
                    query: q,
                    family: award,
                    observed_at: at,
                    departure: cell.departure,
                    return_date: None,
                    price: cell.price,
                    miles: cell.miles,
                    carrier: None,
                    stops: cell.stops,
                    duration_minutes: None,
                    seller: cell.seller,
                    flights: cell.flights,
                    award: cell.award,
                }
            })
            .collect();
        store.insert_observations(&obs).await.expect("inserts");
        store
            .finish_query(q, &QueryOutcome::Ok, 1, at)
            .await
            .expect("finishes");
    }

    h.daemon
        .run_check(award, QueryReason::Manual)
        .await
        .expect("checks");
    let deals = store
        .list_deals(&DealFilter::default())
        .await
        .expect("lists");
    assert_eq!(deals.len(), 1, "{deals:?}");
    let d = &deals[0];
    assert_eq!(d.status, DealStatus::Candidate);
    assert_eq!(
        d.series,
        tm_domain::Series::Award {
            program: "latam_pass".into()
        }
    );
    assert_eq!(d.leg, Some(Leg::Outbound));
    assert_eq!((d.departure, d.return_date), (first, None));
    assert_eq!(d.miles, Some(60_000));
    assert!(
        d.url
            .as_deref()
            .is_some_and(|u| u.contains("redemption=true"))
    );
    assert!(notices.shown().is_empty(), "candidates are not announced");

    // The confirmation reads the same date again, alone.
    h.daemon.run_manual().await;
    let job = h.log().latam.last().cloned().expect("a job");
    assert_eq!(job.departure, first);
    let recent = store.recent_queries(1).await.expect("reads");
    assert_eq!(recent[0].reason, QueryReason::Confirmation);
    let d = store
        .deal(d.id.expect("an id"))
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(d.status, DealStatus::Confirmed);
    let shown = notices.shown();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown[0]
            .title
            .starts_with("SAO → LIS one way: 60,000 mi + R$69"),
        "{:?}",
        shown[0]
    );
    assert!(shown[0].body.contains("LATAM Pass"), "{:?}", shown[0]);
    assert!(shown[0].body.contains("2 seats left"), "{:?}", shown[0]);
    // Announced once.
    h.daemon.run_manual().await;
    assert_eq!(notices.shown().len(), 1);
}

/// The app's "Check award" prices the selected trip's legs, each a LATAM
/// Pass search of its own date, on the shared worker (0083, 0085).
#[tokio::test]
async fn check_award_reads_the_trips_legs() {
    let mut h = harness(Only::default(), |a| {
        a.latam = Box::new(|job| {
            Fetch::ok(
                LatamFound {
                    cells: latam_cells(job),
                    ..LatamFound::default()
                },
                1,
            )
        });
    })
    .await;
    let store = h.store().clone();
    let lis = route(&store, "LIS").await;
    let departure = today() + Days::new(30);
    let return_date = departure + Days::new(8);
    let ask = RequestKind::CheckAward {
        route: lis,
        cabin: Cabin::Economy,
        departure,
        return_date: Some(return_date),
    };
    // Logged out: refused, with why.
    let refused = store.enqueue(&ask).await.expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    let r = store
        .request(refused)
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(r.status, RequestStatus::Failed);
    assert_eq!(r.detail.as_deref(), Some("LATAM Pass is not logged in"));
    assert!(h.log().latam.is_empty());

    log_in(&store).await;
    let request = store.enqueue(&ask).await.expect("enqueues");
    h.daemon.process_requests().await.expect("processes");
    h.daemon.run_manual().await;
    let jobs = h.log().latam.clone();
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    let out = jobs.iter().find(|j| j.origin == "GRU").expect("outbound");
    assert_eq!(
        (out.destination.as_str(), out.departure),
        ("LIS", departure)
    );
    let back = jobs.iter().find(|j| j.origin == "LIS").expect("return");
    assert_eq!(
        (back.destination.as_str(), back.departure),
        ("GRU", return_date)
    );
    assert!(jobs.iter().all(|j| j.award && j.return_date.is_none()));
    let r = store
        .request(request)
        .await
        .expect("reads")
        .expect("exists");
    assert_eq!(r.status, RequestStatus::Done);
    let recent = store.recent_queries(2).await.expect("reads");
    assert!(recent.iter().all(|q| q.reason == QueryReason::Manual));
}

/// The app sets a program's daily loads in its budget row; the worker
/// takes them instead of saving its own copy over them (0083, 0085).
#[tokio::test]
async fn the_worker_keeps_the_budget_the_app_sets() {
    let quiet = Config {
        budget: Some(1),
        max_budget: Some(1),
        ..test_config(Only::default())
    };
    let mut h = harness_with(quiet, |_| {}).await;
    let store = h.store().clone();
    assert!(h.daemon.sites().iter().any(|s| s == "miles:latam_pass"));
    store
        .configure_site_budget(
            "miles:latam_pass",
            SiteLimits { start: 10, max: 20 },
            today(),
            Utc::now(),
        )
        .await
        .expect("configures");
    h.daemon.tick().await.expect("ticks");
    h.daemon.stop().await;
    let row = store
        .site_budget("miles:latam_pass")
        .await
        .expect("reads")
        .expect("a row");
    assert_eq!((row.start_budget, row.max_daily_budget), (10, 20));
    assert!(row.daily_budget <= 20);
}
