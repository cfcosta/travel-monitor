//! One site's worker (DECISIONS.md 0060, 0062): a tokio task that owns
//! the site's browser, budget, pacing and scheduler tick, and runs up to
//! [`Config::parallel`] of the site's checks at once (0072).
//!
//! - **Its own browser.** The worker's [`Sources`] start a Chromium on
//!   the site's own profile directory (`<profile-dir>/<site slug>`,
//!   [`tm_browser::profile::site_profile_dir`]) when the site has work
//!   (a pick, a manual check, a Freebase ID to learn) and close it once
//!   the site has been idle for [`Config::idle_close`]. Points of sale
//!   stay browser contexts inside that process (0033).
//! - **Its own budget and pacing.** A [`Budget`] for the site alone
//!   (0060), and random pauses between its own page loads (0039). One
//!   site's blocks, cooldowns and pace never slow another.
//! - **Its own tick.** `plan_tick` over the site's candidates only, with
//!   the site's budget; the scheduler is unchanged. The picks wait in a
//!   queue, taking turns with manual checks (0071).
//! - **Checks side by side (0072).** A check starts on a free *lane*:
//!   the worker writes its query row, then its fetch runs on a task of
//!   its own while the worker starts others; when the fetch lands, the
//!   worker records it. Each lane rests for a random pause after its
//!   check (0039), so the lanes drift apart. Starting and recording stay
//!   on the worker, one at a time; only the browser work overlaps. The
//!   browser starts or stops only with no fetch in flight.
//! - **Logins pause it alone.** `Pause` closes the site's browser so the
//!   app can open the same profile; `Resume` reads the profile's cookie
//!   jar for the program's session (0064) and lets the site run again.
//!
//! - **LATAM shares one (0084, 0085).** `airline:LA` and
//!   `miles:latam_pass` run on one worker, named for LATAM Pass, with
//!   its profile, browser, budget and pace ([`crate::sites::worker_site`]):
//!   one check at a time, 20–45 s apart. It chooses each check's dates
//!   from the database ([`crate::sites::latam::dates`]), keeps the cash
//!   calendar the pages fetched to rank later dates (never recorded),
//!   follows LATAM Pass's session cookie, and judges and confirms award
//!   deals itself.
//!
//! The store is shared by every worker: each writes its own budget row
//! and status row, and the families, observations, deals and query log
//! they check. Cash deals and their confirmations are Google Flights'
//! (only its checks evaluate trips), so they run on that worker alone;
//! award deals are LATAM's (0083, 0085). Fan-out and
//! Explore hints raise other sites' families from here; a check re-reads
//! its family's state just before writing it, so a raise that lands
//! while it runs is kept.
//!
//! The coordinator ([`crate::daemon`]) sends [`Command`]s. When the
//! worker is *autonomous* (the daemon's loop) it also ticks on its own
//! timer and runs manual checks as they arrive; otherwise (tests,
//! `--once`) it does only what it is told, and every command with a
//! reply runs to completion before the reply.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Context as _;
use chrono::{DateTime, Days, NaiveDate, Utc};
use rand::{Rng, SeedableRng, rngs::StdRng};
use tm_domain::{
    AwardDates,
    Cabin,
    Deal,
    DealId,
    DealStatus,
    ExploreHint,
    FamilyId,
    FamilyKey,
    FamilyKind,
    GOOGLE_FLIGHTS,
    Leg,
    Money,
    Observation,
    PlaceId,
    PlaceKind,
    PointOfSale,
    PosId,
    QueryId,
    QueryOutcome,
    QueryReason,
    RequestId,
    Route,
    RouteId,
    Series,
    Settings,
    Source,
};
use tm_gflights::{PlaceRef, Search, rpc::ExploreDestination, url::search_url};
use tm_scheduler::{
    CheckSummary,
    FxTable,
    Params,
    awards::{AwardKey, award_points, cheapest_by_dates},
    fanout,
    hints::{HintTarget, add_hint, hinted_families},
    plan_tick,
    trips::{build_trips, compare_round_trips},
    update::{min_moved, update_after_check_with},
};
use tm_store::{
    CheckInProgress,
    DealFilter,
    FamilyRecord,
    FamilyStateRecord,
    LoginState,
    SiteStatus,
    Store,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{JoinError, JoinSet},
    time::Instant,
};
use tracing::{debug, info, warn};

use crate::{
    Config,
    budget::{self, Budget},
    candidates::{
        self,
        Context,
        Only,
        from_discount,
        to_discount,
        to_record,
        to_state,
    },
    deals::{self, Group},
    jobs::{self, Job, JobError, JobOptions, LatamDates},
    notify,
    sites::{
        self,
        latam::{
            self,
            dates::{self as latam_dates, Known, Pick, Strip},
        },
    },
    sources::{Cell, ConfirmJob, Fetch, LatamFound, Sources},
};

/// Cells older than this are not paired into trips.
const FRESH_HOURS: i64 = 48;

/// How far back a LATAM check looks for the departures its family read
/// and for LATAM's own cash (0085).
const LATAM_READ_DAYS: i64 = 30;

/// Award deals recorded per series and check: each is confirmed by a
/// fresh read of its date, one search (0085).
const AWARD_CONFIRMS: usize = 1;

/// LATAM's pause between searches, in seconds (0084), unless the
/// command line pauses longer.
const LATAM_PACE: (u64, u64) = (20, 45);

/// The configuration every worker works from, published by the
/// coordinator whenever it reloads (0035).
#[derive(Debug, Clone)]
pub struct Loaded {
    pub settings: Settings,
    pub params: Params,
    pub version: i64,
}

impl Default for Loaded {
    fn default() -> Self {
        let settings = Settings::default();
        Self {
            params: Params::from_settings(&settings),
            settings,
            version: -1,
        }
    }
}

/// Manual checks still to run per request (0046), shared by the
/// coordinator, which counts them, and the workers, which run them: a
/// route's check spans sites.
#[derive(Debug, Clone, Default)]
pub(crate) struct Tracker(Arc<Mutex<HashMap<RequestId, usize>>>);

impl Tracker {
    pub(crate) fn expect(&self, request: RequestId, checks: usize) {
        if let Ok(mut left) = self.0.lock() {
            *left.entry(request).or_insert(0) += checks;
        }
    }

    /// One check of `request` ran; true when it was the last.
    fn done(&self, request: RequestId) -> bool {
        let Ok(mut left) = self.0.lock() else {
            return false;
        };
        match left.get_mut(&request) {
            Some(n) if *n > 1 => {
                *n -= 1;
                false
            }
            Some(_) => {
                left.remove(&request);
                true
            }
            None => false,
        }
    }
}

/// What every worker shares.
#[derive(Clone)]
pub(crate) struct Shared {
    pub store: Store,
    pub config: Config,
    pub loaded: watch::Receiver<Arc<Loaded>>,
    pub shutdown: watch::Receiver<bool>,
    pub tracker: Tracker,
}

/// What the coordinator asks of a worker.
pub(crate) enum Command {
    /// One scheduler tick (tests and `--once`).
    Tick(oneshot::Sender<()>),
    /// Run the queued manual checks (tests and `--once`).
    RunManual(oneshot::Sender<()>),
    /// Learn missing Freebase IDs (Google Flights).
    Resolve(oneshot::Sender<()>),
    /// One check now, outside the queue (tests).
    Check {
        family: FamilyId,
        reason: QueryReason,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    /// Manual checks of a request (0046); the request is completed by
    /// whichever worker runs its last check.
    Queue(Vec<FamilyId>, Option<RequestId>),
    /// Manual checks of given dates: the app's "Check award" (0083,
    /// 0085), one LATAM search per leg.
    QueueDated(Vec<(FamilyId, AwardKey)>, Option<RequestId>),
    /// Re-check a deal (Google Flights).
    Confirm(DealId, RequestId),
    /// A login window is about to open this site's profile (0024).
    Pause {
        program: String,
        request: Option<RequestId>,
    },
    /// The login window closed (0064).
    Resume {
        program: Option<String>,
        request: Option<RequestId>,
    },
    /// Answered once every command before it was handled.
    Sync(oneshot::Sender<()>),
}

/// What a check produced, for the bookkeeping after it.
enum Found {
    Cells(Vec<Cell>),
    Explore(Vec<ExploreDestination>),
    Latam(LatamFound),
}

/// A check whose fetch is in flight (0072): what recording it needs.
struct Flight {
    family: FamilyRecord,
    route: Route,
    pos: PointOfSale,
    query: QueryId,
    reason: QueryReason,
    /// The manual request it serves (0046).
    request: Option<RequestId>,
    /// The award deal this check confirms, by reading its dates again
    /// (0085).
    confirms: Option<DealId>,
}

/// A LATAM cash calendar as a page fetched it, and when (0085).
type Calendar = (DateTime<Utc>, BTreeMap<NaiveDate, i64>);

/// A LATAM check of given dates instead of the dates the worker would
/// choose (0085): the app's "Check award", or an award candidate's
/// confirmation.
#[derive(Debug, Clone, Copy)]
struct Dated {
    at: AwardKey,
    confirms: Option<DealId>,
}

/// A fetch that came back.
struct Landed {
    flight: Flight,
    fetch: Fetch<()>,
    found: Found,
}

/// Which queues a worker draws its next check from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Both, in turn (the daemon's loop).
    All,
    /// A tick's picks, each with a manual check before it while any
    /// wait (0071).
    Tick,
    /// Manual checks only.
    Manual,
}

enum Manual {
    Check(FamilyId, Option<RequestId>),
    /// A LATAM check of given dates (0085).
    Dated(FamilyId, AwardKey, Option<RequestId>),
    Confirm(DealId, RequestId),
    /// An award candidate's confirmation: its dates read again (0085).
    ConfirmAward(DealId),
}

/// Which queued manual check goes next: the first one for a request
/// other than `last`, else the first one (0071).
fn next_manual_index(
    queue: &VecDeque<Manual>,
    last: Option<RequestId>,
) -> usize {
    queue.iter().position(|m| m.request() != last).unwrap_or(0)
}

impl Manual {
    /// The app request this check serves, if any.
    fn request(&self) -> Option<RequestId> {
        match self {
            Manual::Check(_, request) | Manual::Dated(_, _, request) => {
                *request
            }
            Manual::Confirm(_, request) => Some(*request),
            Manual::ConfirmAward(_) => None,
        }
    }
}

/// The configuration a tick works from.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub families: Vec<FamilyRecord>,
    pub routes: HashMap<RouteId, Route>,
    pub sweeps: HashSet<RouteId>,
    pub pos: HashMap<PosId, PointOfSale>,
    /// Families whose last check that loaded found no flights (0067).
    pub no_flights: HashSet<FamilyId>,
    /// Miles programs without a live login (0085).
    pub logged_out: HashSet<String>,
}

impl World {
    fn family_by_key(&self, key: &FamilyKey) -> Option<&FamilyRecord> {
        self.families.iter().find(|f| &f.key == key)
    }
}

/// Everything a tick needs from the configuration.
pub async fn world(store: &Store) -> anyhow::Result<World> {
    let families = store.list_families(true).await?;
    let mut routes: HashMap<RouteId, Route> = store
        .list_routes(false)
        .await?
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
    let mut sweeps = HashSet::new();
    for r in store.sweep_routes(false).await? {
        sweeps.insert(r.id);
        routes.insert(r.id, r);
    }
    let pos = store
        .list_points_of_sale(false)
        .await?
        .into_iter()
        .map(|p| (p.id, p))
        .collect();
    let no_flights = store.no_flights_families().await?.into_iter().collect();
    let logged_out = store
        .list_miles_programs()
        .await?
        .into_iter()
        .filter(|p| p.login_state != LoginState::LoggedIn)
        .map(|p| p.program)
        .collect();
    Ok(World {
        families,
        routes,
        sweeps,
        pos,
        no_flights,
        logged_out,
    })
}

/// The candidate filters for `world` at `now`, with the sites cooling
/// down and paused.
pub fn context(
    world: &World,
    settings: &Settings,
    only: &Only,
    cooling: HashSet<String>,
    paused: HashSet<String>,
) -> Context {
    let unresolved = world
        .routes
        .values()
        .filter(|r| {
            r.origin.google_id().is_none()
                || r.destination.google_id().is_none()
        })
        .map(|r| r.id)
        .collect();
    let destinations = world
        .routes
        .values()
        .map(|r| {
            let code = (!world.sweeps.contains(&r.id))
                .then(|| r.destination.code.clone());
            (r.id, code)
        })
        .collect();
    Context {
        unresolved,
        cooling,
        destinations,
        paused,
        only: only.clone(),
        home_pos: world.pos.values().find(|p| p.is_home).map(|p| p.id),
        passengers: settings.passengers,
        no_flights: world.no_flights.clone(),
        logged_out: world.logged_out.clone(),
    }
}

/// One site's worker. See the module documentation.
pub(crate) struct Worker<S: Sources> {
    site: String,
    /// Whether the site has a reader (0050): one without only pauses for
    /// logins and never spends.
    reader: bool,
    shared: Shared,
    /// Shared with the fetches in flight.
    sources: Arc<S>,
    loaded: Arc<Loaded>,
    budget: Budget,
    rng: StdRng,
    rx: mpsc::UnboundedReceiver<Command>,
    autonomous: bool,
    /// Commands that arrived during a check and wait for it to end.
    pending: VecDeque<Command>,
    /// The program being logged into while the site is paused (0024).
    paused: Option<String>,
    /// Manual checks (0046), interleaved one-for-one with the
    /// scheduler's picks (0071).
    manual: VecDeque<Manual>,
    /// The last tick's picks still to start.
    scheduled: VecDeque<(FamilyId, QueryReason)>,
    /// Whether a manual check goes before the next pick (0071).
    manual_turn: bool,
    /// The request the last manual check served, so the next one comes
    /// from another pending request when there is one (0071).
    last_manual: Option<RequestId>,
    /// Places whose Freebase ID could not be learned under
    /// `resolve_version` of the configuration.
    resolve_tried: HashSet<PlaceId>,
    resolve_version: i64,
    /// The browser did not start.
    unavailable: bool,
    stopped: bool,
    /// When the site looks for work again, for the app's countdown.
    next_check_at: Option<DateTime<Utc>>,
    /// The checks running now (0057, 0072).
    checks: Vec<CheckInProgress>,
    /// The fetches in flight, at most [`Config::parallel`] (0072).
    flights: JoinSet<Landed>,
    /// Each fetch's query and request, to close them should its task
    /// panic, and the loads its family is expected to cost: a tick plans
    /// only what the bank holds beyond the checks still in flight.
    flight_ids: HashMap<tokio::task::Id, (QueryId, Option<RequestId>, u32)>,
    /// Lanes resting after a check, until these instants (0039).
    resting: Vec<Instant>,
    /// When the browser last did something, to close it when idle.
    last_work: Instant,
    /// LATAM's cash calendar per route and leg, as the pages fetched it,
    /// and when: it ranks later checks' dates (0085), never recorded.
    calendars: HashMap<(RouteId, Leg), Calendar>,
    /// When each award family last had its weekly spot check (0085).
    /// Lost on a restart, which brings one spot check forward.
    last_spot: HashMap<FamilyId, DateTime<Utc>>,
}

impl<S: Sources + 'static> Worker<S> {
    /// Starts `site`'s worker task. `budget` is the site's, restored.
    pub(crate) fn spawn(
        site: String,
        shared: Shared,
        sources: S,
        budget: Budget,
        seed: u64,
        autonomous: bool,
    ) -> (mpsc::UnboundedSender<Command>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let loaded = Arc::clone(&shared.loaded.borrow());
        let worker = Self {
            reader: candidates::has_reader(&site),
            site,
            shared,
            sources: Arc::new(sources),
            loaded,
            budget,
            rng: StdRng::seed_from_u64(seed),
            rx,
            autonomous,
            pending: VecDeque::new(),
            paused: None,
            manual: VecDeque::new(),
            scheduled: VecDeque::new(),
            manual_turn: true,
            last_manual: None,
            resolve_tried: HashSet::new(),
            resolve_version: -1,
            unavailable: false,
            stopped: false,
            next_check_at: None,
            checks: Vec::new(),
            flights: JoinSet::new(),
            flight_ids: HashMap::new(),
            resting: Vec::new(),
            last_work: Instant::now(),
            calendars: HashMap::new(),
            last_spot: HashMap::new(),
        };
        (tx, tokio::spawn(worker.run()))
    }
}

impl<S: Sources + 'static> Worker<S> {
    fn store(&self) -> &Store {
        &self.shared.store
    }

    fn config(&self) -> &Config {
        &self.shared.config
    }

    fn settings(&self) -> &Settings {
        &self.loaded.settings
    }

    fn params(&self) -> &Params {
        &self.loaded.params
    }

    fn stopping(&self) -> bool {
        *self.shared.shutdown.borrow()
    }

    fn is_google(&self) -> bool {
        self.site == GOOGLE_FLIGHTS
    }

    /// LATAM's worker, for `airline:LA` and `miles:latam_pass` (0084).
    fn is_latam(&self) -> bool {
        sites::is_latam(&self.site)
    }

    /// How many checks run at once: LATAM runs one, every check a search
    /// under the user's own account (0084); other sites `--parallel`.
    fn lanes(&self) -> usize {
        if self.is_latam() {
            1
        } else {
            self.config().parallel.max(1)
        }
    }

    /// The random pause after a check, in seconds: LATAM's is a page
    /// load's 20–45 s (0084), unless pauses are off.
    fn pace_range(&self) -> (u64, u64) {
        let (min, max) = self.config().pace;
        if self.is_latam() && max > 0 {
            (min.max(LATAM_PACE.0), max.max(LATAM_PACE.1))
        } else {
            (min, max)
        }
    }

    /// This site's Chromium profile (0062), whose cookie jar says
    /// whether a miles login took (0064).
    fn profile_dir(&self) -> Option<PathBuf> {
        self.config()
            .profile_dir
            .as_deref()
            .map(|base| tm_browser::profile::site_profile_dir(base, &self.site))
    }

    // -----------------------------------------------------------------
    // The task
    // -----------------------------------------------------------------

    async fn run(mut self) {
        info!(site = %self.site, reader = self.reader, "site worker started");
        self.status().await;
        // LATAM Pass's session, as the profile holds it (0085).
        if self.is_latam() && self.reader {
            self.read_profile_session().await;
        }
        // Deals confirmed while no notification could be shown.
        if (self.is_google() || self.is_latam())
            && let Err(e) = self.notify_deals().await
        {
            warn!("cannot announce deals: {e:#}");
        }
        let mut next_tick = Instant::now();
        let mut shutdown = self.shared.shutdown.clone();
        // A dropped sender cannot signal any more: stop watching it.
        let mut watch_shutdown = true;
        loop {
            while let Some(command) = self.next_command() {
                self.handle(command).await;
            }
            if self.stopping() {
                break;
            }
            // Whether queued work waits on something other than a lane.
            let mut stalled = false;
            if self.autonomous {
                // A new tick once the last one's picks have all started.
                if Instant::now() >= next_tick && self.scheduled.is_empty() {
                    if let Err(e) = self.plan().await {
                        warn!(site = %self.site, "tick failed: {e:#}");
                    }
                    next_tick = Instant::now() + self.config().tick;
                    self.next_check_at = Some(
                        Utc::now()
                            + chrono::TimeDelta::from_std(self.config().tick)
                                .unwrap_or_default(),
                    );
                    self.status().await;
                }
                stalled = self.dispatch(Mode::All).await;
                self.close_when_idle().await;
                if !self.pending.is_empty() || self.stopping() {
                    continue;
                }
            }
            let wake = if self.autonomous {
                let mut wake = next_tick;
                if self.sources.is_running() && self.flights.is_empty() {
                    let idle = self.last_work + self.config().idle_close;
                    wake = wake.min(idle.max(Instant::now()));
                }
                if !stalled
                    && self.has_work(Mode::All)
                    && let Some(rest) = self.next_rest()
                {
                    wake = wake.min(rest);
                }
                wake
            } else {
                Instant::now() + Duration::from_secs(86_400)
            };
            tokio::select! {
                command = self.rx.recv() => match command {
                    Some(command) => self.pending.push_back(command),
                    None => break,
                },
                Some(joined) = self.flights.join_next_with_id() => {
                    // Logged by `landed`.
                    let _ = self.landed(joined).await;
                }
                () = tokio::time::sleep_until(wake) => {}
                changed = shutdown.changed(), if watch_shutdown => {
                    watch_shutdown = changed.is_ok();
                }
            }
        }
        self.finish().await;
    }

    /// A command held during a check, else one waiting in the channel.
    fn next_command(&mut self) -> Option<Command> {
        self.pending.pop_front().or_else(|| self.rx.try_recv().ok())
    }

    /// Handles one command.
    async fn handle(&mut self, command: Command) {
        match command {
            Command::Tick(reply) => {
                match self.plan().await {
                    Ok(()) => self.drain(Mode::Tick).await,
                    Err(e) => warn!(site = %self.site, "tick failed: {e:#}"),
                }
                // A tick ends with its picks, started or not.
                self.scheduled.clear();
                let _ = reply.send(());
            }
            Command::RunManual(reply) => {
                if !self.manual.is_empty() && self.paused.is_none() {
                    self.refresh_loaded().await;
                    self.drain(Mode::Manual).await;
                }
                let _ = reply.send(());
            }
            Command::Resolve(reply) => {
                self.refresh_loaded().await;
                if let Err(e) = self.resolve_places().await {
                    warn!("resolving Freebase IDs failed: {e:#}");
                }
                let _ = reply.send(());
            }
            Command::Check {
                family,
                reason,
                reply,
            } => {
                self.refresh_loaded().await;
                let result = self.run_check(family, reason).await;
                let _ = reply.send(result);
            }
            other => self.handle_light(other).await,
        }
    }

    /// Commands that take no browser time: handled even between the
    /// checks of a tick. The rest wait in `pending`.
    async fn handle_light(&mut self, command: Command) {
        let result = match command {
            Command::Queue(families, request) => {
                self.manual.extend(
                    families.into_iter().map(|f| Manual::Check(f, request)),
                );
                Ok(())
            }
            Command::QueueDated(checks, request) => {
                self.manual.extend(
                    checks
                        .into_iter()
                        .map(|(f, at)| Manual::Dated(f, at, request)),
                );
                Ok(())
            }
            Command::Confirm(deal, request) => {
                self.manual.push_back(Manual::Confirm(deal, request));
                Ok(())
            }
            Command::Pause { program, request } => {
                self.pause(program, request).await
            }
            Command::Resume { program, request } => {
                self.resume(program, request).await
            }
            Command::Sync(reply) => {
                let _ = reply.send(());
                Ok(())
            }
            heavy => {
                self.pending.push_back(heavy);
                Ok(())
            }
        };
        if let Err(e) = result {
            warn!(site = %self.site, "request failed: {e:#}");
        }
    }

    /// Handles the light commands that arrived, between two checks.
    async fn poll(&mut self) {
        let mut heavy = VecDeque::new();
        while let Ok(command) = self.rx.try_recv() {
            match command {
                c @ (Command::Tick(_)
                | Command::RunManual(_)
                | Command::Resolve(_)
                | Command::Check { .. }) => heavy.push_back(c),
                light => self.handle_light(light).await,
            }
        }
        self.pending.extend(heavy);
    }

    /// Lets every fetch in flight land, closes the browser, saves the
    /// budget and says the site stopped.
    async fn finish(&mut self) {
        self.land_all().await;
        self.sources.stop().await;
        self.stopped = true;
        self.checks.clear();
        self.next_check_at = None;
        if self.reader
            && let Err(e) = self.budget.save(self.store()).await
        {
            warn!(site = %self.site, "saving the budget failed: {e:#}");
        }
        self.status().await;
        info!(site = %self.site, "site worker stopped");
    }

    /// Closes a browser that has had no work for `idle_close` (0062).
    async fn close_when_idle(&mut self) {
        if self.flights.is_empty()
            && self.sources.is_running()
            && self.last_work.elapsed() >= self.config().idle_close
        {
            info!(site = %self.site, "no work: closing the browser");
            self.sources.stop().await;
            self.status().await;
        }
    }

    // -----------------------------------------------------------------
    // Status (0057, 0062)
    // -----------------------------------------------------------------

    fn state(&self) -> String {
        if self.stopped {
            "stopped".to_owned()
        } else if let Some(program) = &self.paused {
            format!("paused for login: {program}")
        } else if !self.checks.is_empty() {
            "checking".to_owned()
        } else if self.reader && self.budget.is_cooling(Utc::now()) {
            "cooling down".to_owned()
        } else if self.unavailable {
            "browser unavailable".to_owned()
        } else {
            "idle".to_owned()
        }
    }

    async fn status(&mut self) {
        let status = SiteStatus {
            site: self.site.clone(),
            updated_at: Utc::now(),
            state: self.state(),
            next_check_at: self.next_check_at,
            checks: self.checks.clone(),
        };
        if let Err(e) = self.store().write_site_status(&status).await {
            warn!(site = %self.site, "status failed: {e}");
        }
    }

    async fn begin_check(
        &mut self,
        family: FamilyId,
        query: QueryId,
        reason: &QueryReason,
        started_at: DateTime<Utc>,
    ) {
        self.checks.push(CheckInProgress {
            family,
            query: Some(query),
            reason: reason.clone(),
            started_at,
        });
        self.next_check_at = None;
        self.status().await;
    }

    async fn end_check(&mut self, query: QueryId) {
        let before = self.checks.len();
        self.checks.retain(|c| c.query != Some(query));
        if self.checks.len() != before {
            self.status().await;
        }
    }

    // -----------------------------------------------------------------
    // Logins (0024, 0064)
    // -----------------------------------------------------------------

    async fn pause(
        &mut self,
        program: String,
        request: Option<RequestId>,
    ) -> anyhow::Result<()> {
        info!(site = %self.site, %program, "pausing the site for a login");
        // Nothing new starts; the checks in flight land first.
        self.paused = Some(program);
        self.land_all().await;
        self.sources.stop().await;
        self.status().await;
        if let Some(id) = request {
            self.store()
                .complete(id, true, Some("the site's browser is closed"))
                .await?;
        }
        Ok(())
    }

    async fn resume(
        &mut self,
        program: Option<String>,
        request: Option<RequestId>,
    ) -> anyhow::Result<()> {
        let program = program.or_else(|| self.paused.clone());
        self.paused = None;
        if let (Some(program), Some(dir)) = (program, self.profile_dir()) {
            // The window has closed and our Chromium is not up, so the
            // cookie jar is free to read.
            let domains = tm_domain::miles_session_domains(&program);
            let now = Utc::now();
            match tm_browser::profile::login_cookies(&dir, domains, now).await {
                Ok(cookies) => {
                    let state = if cookies.is_empty() {
                        LoginState::LoggedOut
                    } else {
                        LoginState::LoggedIn
                    };
                    info!(%program, profile = %dir.display(), ?state, "login window closed");
                    self.store().set_miles_login(&program, state, now).await?;
                    let until = cookies.iter().filter_map(|c| c.expires).max();
                    self.store()
                        .set_miles_session_until(&program, until)
                        .await?;
                }
                Err(e) => warn!(%program, "login not checked: {e}"),
            }
        }
        self.status().await;
        if let Some(id) = request {
            self.store().complete(id, true, Some("resumed")).await?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Configuration and budget
    // -----------------------------------------------------------------

    /// Takes the configuration the coordinator last published. Google
    /// Flights' limits follow the settings (0060).
    async fn refresh_loaded(&mut self) {
        let loaded = Arc::clone(&self.shared.loaded.borrow());
        if loaded.version == self.loaded.version {
            return;
        }
        self.reread_budget().await;
        self.loaded = loaded;
        if self.is_google() {
            let limits = self.loaded.settings.google_flights_limits();
            if self.budget.configure(limits, Utc::now())
                && let Err(e) = self.budget.save(self.store()).await
            {
                warn!("saving the budget failed: {e:#}");
            }
        }
    }

    /// Takes the limits the app wrote into this site's budget row
    /// (`configure_site_budget`, a program's daily award loads, 0083)
    /// instead of keeping, and later saving over them, the copy in
    /// memory. Google Flights' limits are the settings'.
    async fn reread_budget(&mut self) {
        if self.is_google() || !self.reader {
            return;
        }
        match self.store().site_budget(&self.site).await {
            Ok(Some(row)) => {
                if self.budget.adopt(&row) {
                    info!(
                        site = %self.site,
                        start = row.start_budget,
                        max = row.max_daily_budget,
                        "budget limits changed"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!(site = %self.site, "reading the budget failed: {e}")
            }
        }
    }

    async fn spend(&mut self, loads: u32) -> anyhow::Result<()> {
        let store = self.store().clone();
        self.budget
            .spend(&store, loads, Utc::now(), budget::today())
            .await
    }

    async fn block(&mut self, detail: &str) -> anyhow::Result<()> {
        warn!(site = %self.site, %detail, "blocked: backing off");
        let store = self.store().clone();
        let params = self.loaded.params.clone();
        self.budget
            .on_block(&store, detail, Utc::now(), &params)
            .await?;
        self.status().await;
        Ok(())
    }

    /// Starts the browser unless it runs or the site is paused. True when
    /// it runs.
    async fn ensure_browser(&mut self) -> bool {
        if self.paused.is_some() {
            return false;
        }
        if self.sources.is_running() {
            return true;
        }
        let pos = match self.store().list_points_of_sale(true).await {
            Ok(pos) => pos,
            Err(e) => {
                warn!("cannot list points of sale: {e}");
                return false;
            }
        };
        match self.sources.start(&pos).await {
            Ok(()) => {
                self.unavailable = false;
                self.last_work = Instant::now();
                true
            }
            Err(e) => {
                warn!(site = %self.site, "the browser did not start: {e:#}");
                self.unavailable = true;
                self.status().await;
                false
            }
        }
    }

    /// A random pause between page loads, cut short by a shutdown.
    async fn pace(&mut self) {
        let (min, max) = self.pace_range();
        if max == 0 {
            return;
        }
        let secs = self.rng.random_range(min..=max);
        self.next_check_at = Some(
            Utc::now()
                + chrono::TimeDelta::from_std(Duration::from_secs(secs))
                    .unwrap_or_default(),
        );
        self.status().await;
        let mut shutdown = self.shared.shutdown.clone();
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(secs)) => {}
            _ = shutdown.changed() => {}
        }
    }

    // -----------------------------------------------------------------
    // Lanes (0072)
    // -----------------------------------------------------------------

    /// Rests the lane a check just freed for a random pause (0039).
    fn rest(&mut self) {
        let (min, max) = self.pace_range();
        if max == 0 {
            return;
        }
        let secs = self.rng.random_range(min..=max);
        self.resting
            .push(Instant::now() + Duration::from_secs(secs));
    }

    /// When the next resting lane is free again.
    fn next_rest(&mut self) -> Option<Instant> {
        let now = Instant::now();
        self.resting.retain(|&at| at > now);
        self.resting.iter().min().copied()
    }

    /// Lanes neither checking nor resting.
    fn free_lanes(&mut self) -> usize {
        self.next_rest();
        self.lanes()
            .saturating_sub(self.flights.len() + self.resting.len())
    }

    /// Whether a check waits in the queues `mode` draws from.
    fn has_work(&self, mode: Mode) -> bool {
        if self.paused.is_some() || self.stopping() {
            return false;
        }
        match mode {
            Mode::All => !self.manual.is_empty() || !self.scheduled.is_empty(),
            Mode::Tick => !self.scheduled.is_empty(),
            Mode::Manual => !self.manual.is_empty(),
        }
    }

    /// Starts queued checks on the free lanes: a manual check, then a
    /// pick, and so on (0071). True when work is left that cannot start
    /// for another reason than a busy lane (no browser).
    async fn dispatch(&mut self, mode: Mode) -> bool {
        loop {
            if self.reader && self.budget.is_cooling(Utc::now()) {
                // A block ends the tick (0039).
                self.scheduled.clear();
            }
            if !self.has_work(mode) || self.free_lanes() == 0 {
                return false;
            }
            let manual = !self.manual.is_empty();
            let pick = !self.scheduled.is_empty() && mode != Mode::Manual;
            let take_manual = manual && (self.manual_turn || !pick);
            if take_manual {
                self.manual_turn = false;
                let Some(item) = self.next_manual() else {
                    return false;
                };
                // Reader-less sites check nothing, so need no browser.
                if self.reader && !self.ensure_browser().await {
                    self.manual.push_front(item);
                    return true;
                }
                self.refresh_loaded().await;
                self.last_manual = item.request();
                match item {
                    Manual::Check(family, request) => {
                        self.launch(family, QueryReason::Manual, request, None)
                            .await;
                    }
                    Manual::Dated(family, at, request) => {
                        let dated = Dated { at, confirms: None };
                        self.launch(
                            family,
                            QueryReason::Manual,
                            request,
                            Some(dated),
                        )
                        .await;
                    }
                    Manual::Confirm(id, request) => {
                        if let Err(e) = self.confirm_request(id, request).await
                        {
                            warn!(deal = %id, "confirmation failed: {e:#}");
                        }
                        self.rest();
                    }
                    Manual::ConfirmAward(id) => {
                        if let Err(e) = self.confirm_award(id).await {
                            warn!(deal = %id, "award confirmation failed: {e:#}");
                        }
                    }
                }
            } else if pick {
                self.manual_turn = true;
                if !self.ensure_browser().await {
                    self.scheduled.clear();
                    return true;
                }
                let Some((family, reason)) = self.scheduled.pop_front() else {
                    return false;
                };
                self.launch(family, reason, None, None).await;
            } else {
                return false;
            }
        }
    }

    /// Starts one check; a manual request is told at once when it does
    /// not start.
    async fn launch(
        &mut self,
        family: FamilyId,
        reason: QueryReason,
        request: Option<RequestId>,
        dated: Option<Dated>,
    ) {
        match self.launch_check(family, reason, request, dated).await {
            Ok(Some(_)) => {}
            Ok(None) => self.finish_request(request).await,
            Err(e) => {
                warn!(%family, "check failed: {e:#}");
                self.finish_request(request).await;
            }
        }
    }

    /// Counts one check of a manual request done, completing the request
    /// with its last (0046).
    async fn finish_request(&mut self, request: Option<RequestId>) {
        if let Some(request) = request
            && self.shared.tracker.done(request)
            && let Err(e) = self.store().complete(request, true, None).await
        {
            warn!(%request, "completing the request failed: {e}");
        }
    }

    /// Starts and lands checks from `mode`'s queues until none is left
    /// or none can start, then lets the last ones land (tests and
    /// `--once`).
    async fn drain(&mut self, mode: Mode) {
        loop {
            self.poll().await;
            let stalled = self.dispatch(mode).await;
            let rest = (!stalled && self.has_work(mode))
                .then(|| self.next_rest())
                .flatten();
            if self.flights.is_empty() {
                match rest {
                    Some(at) => {
                        let mut shutdown = self.shared.shutdown.clone();
                        tokio::select! {
                            () = tokio::time::sleep_until(at) => {}
                            _ = shutdown.changed() => {}
                        }
                    }
                    None => break,
                }
                continue;
            }
            let wake = rest.unwrap_or_else(|| {
                Instant::now() + Duration::from_secs(86_400)
            });
            tokio::select! {
                Some(joined) = self.flights.join_next_with_id() => {
                    // Logged by `landed`.
                    let _ = self.landed(joined).await;
                }
                () = tokio::time::sleep_until(wake) => {}
            }
        }
    }

    /// Lets every fetch in flight land, recording each.
    async fn land_all(&mut self) {
        while let Some(joined) = self.flights.join_next_with_id().await {
            // Logged by `landed`.
            let _ = self.landed(joined).await;
        }
    }

    // -----------------------------------------------------------------
    // The tick (0028) and manual checks (0046)
    // -----------------------------------------------------------------

    /// One scheduler tick over this site's candidates: its picks join
    /// the queue. A family in flight is not picked again.
    async fn plan(&mut self) -> anyhow::Result<()> {
        self.refresh_loaded().await;
        self.reread_budget().await;
        let now = Utc::now();
        let params = self.loaded.params.clone();
        if self.reader && self.budget.advance(now, budget::today(), &params) {
            self.budget.save(self.store()).await?;
        }
        if !self.reader || self.paused.is_some() {
            return Ok(());
        }
        if self.is_google()
            && let Err(e) = self.resolve_places().await
        {
            warn!("resolving Freebase IDs failed: {e:#}");
        }
        let world = world(self.store()).await?;
        let cooling: HashSet<String> = if self.budget.is_cooling(now) {
            [self.site.clone()].into()
        } else {
            HashSet::new()
        };
        let cx = context(
            &world,
            self.settings(),
            &self.config().only,
            cooling,
            HashSet::new(),
        );
        let site = self.site.clone();
        let flying: HashSet<FamilyId> =
            self.checks.iter().map(|c| c.family).collect();
        let (candidates, skipped) = candidates::build(
            world.families.iter().filter(|f| {
                sites::worker_site(&f.site) == site && !flying.contains(&f.id)
            }),
            &cx,
        );
        let ticks_left = if self.config().once {
            1
        } else {
            budget::ticks_left_today(self.config().tick)
        };
        let mut rng = StdRng::from_rng(&mut self.rng);
        let mut tick_budget =
            self.budget.tick(now, ticks_left, &params, &mut rng);
        // A check's loads leave the bank when it lands: until then its
        // expected cost is held back, or a tick planned while checks ran
        // would spend the same loads again and leave the site idle once
        // they landed (0079).
        let reserved: u32 = self.flight_ids.values().map(|f| f.2).sum();
        tick_budget.loads_available =
            tick_budget.loads_available.saturating_sub(reserved);
        let site_params = self.budget.params(&params);
        let picks =
            plan_tick(&candidates, now, &tick_budget, &site_params, &mut rng);
        info!(
            site = %self.site,
            candidates = candidates.len(),
            ?skipped,
            loads_available = tick_budget.loads_available,
            reserved,
            loads_today = self.budget.loads_today,
            budget = self.budget.daily(now, &params),
            picks = picks.len(),
            "tick planned"
        );
        self.scheduled
            .extend(picks.into_iter().map(|p| (p.family, p.reason)));
        self.manual_turn = true;
        Ok(())
    }

    /// The next manual check: the first one queued for a request other
    /// than the last one served, else the first one (0071).
    fn next_manual(&mut self) -> Option<Manual> {
        let i = next_manual_index(&self.manual, self.last_manual);
        self.manual.remove(i)
    }

    async fn confirm_request(
        &mut self,
        id: DealId,
        request: RequestId,
    ) -> anyhow::Result<()> {
        match self.store().deal(id).await? {
            Some(d)
                if d.status == DealStatus::Candidate
                    || d.status == DealStatus::Confirmed =>
            {
                let ok = self.confirm(id, &d, None).await.is_ok();
                self.store().complete(request, ok, None).await?;
            }
            Some(_) => {
                self.store()
                    .complete(
                        request,
                        false,
                        Some("the deal is no longer open"),
                    )
                    .await?;
            }
            None => {
                self.store()
                    .complete(request, false, Some("no such deal"))
                    .await?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Freebase IDs (0041)
    // -----------------------------------------------------------------

    /// Learns the Freebase ID of every city on a selected route that
    /// lacks one, once per configuration version (Google Flights).
    async fn resolve_places(&mut self) -> anyhow::Result<()> {
        if !self.is_google() || self.paused.is_some() {
            return Ok(());
        }
        if self.resolve_version != self.loaded.version {
            self.resolve_tried.clear();
            self.resolve_version = self.loaded.version;
        }
        let routes = self.store().list_routes(true).await?;
        let only = self.config().only.clone();
        let mut places = Vec::new();
        for route in &routes {
            if let Some(code) = &only.destination
                && !route.destination.code.eq_ignore_ascii_case(code)
            {
                continue;
            }
            if let Some(id) = only.family {
                match self.store().family(id).await? {
                    Some(f) if f.key.route == route.id => {}
                    _ => continue,
                }
            }
            for place in [&route.origin, &route.destination] {
                if matches!(place.kind, PlaceKind::City { freebase_id: None })
                    && !self.resolve_tried.contains(&place.id)
                    && !places
                        .iter()
                        .any(|p: &tm_domain::Place| p.id == place.id)
                {
                    places.push(place.clone());
                }
            }
        }
        if places.is_empty() {
            return Ok(());
        }
        let Some(home) = self.store().home_point_of_sale().await? else {
            return Ok(());
        };
        for place in places {
            let now = Utc::now();
            if self.stopping()
                || self.budget.is_cooling(now)
                || self.budget.is_spent(now, self.params())
                || !self.ensure_browser().await
            {
                break;
            }
            self.resolve_tried.insert(place.id);
            let job = jobs::resolve_job(
                &place,
                self.settings(),
                &home,
                &self.config().jobs,
            );
            info!(code = %place.code, url = %job.url, "learning a Freebase ID");
            let fetch = self.sources.resolve(&job).await;
            self.last_work = Instant::now();
            self.spend(fetch.page_loads).await?;
            match (&fetch.outcome, &fetch.value) {
                (QueryOutcome::Ok, Some(id)) => {
                    info!(code = %place.code, %id, "Freebase ID learned");
                    self.store().set_freebase_id(place.id, id).await?;
                }
                (QueryOutcome::Blocked { detail }, _) => {
                    self.block(detail).await?;
                    break;
                }
                (outcome, _) => {
                    warn!(code = %place.code, ?outcome, "no Freebase ID");
                }
            }
            self.pace().await;
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Checks
    // -----------------------------------------------------------------

    /// The departure a TAP search without a calendar prices (0053): the
    /// cheapest one Google Flights showed lately for the same route,
    /// cabin and leg under the home point of sale.
    async fn cheapest_departure(
        &mut self,
        family: &FamilyRecord,
    ) -> Option<NaiveDate> {
        let home = self.store().home_point_of_sale().await.ok()??;
        let leg = match family.key.kind {
            FamilyKind::Leg { leg } => leg,
            _ => Leg::Outbound,
        };
        let key = FamilyKey {
            route: family.key.route,
            cabin: family.key.cabin,
            source: Source::GoogleFlights,
            pos: home.id,
            kind: FamilyKind::Leg { leg },
        };
        let id = self.store().family_id(&key).await.ok()??;
        let tomorrow = budget::today() + Days::new(1);
        self.store()
            .latest_cells(id)
            .await
            .ok()?
            .into_iter()
            .filter(|c| c.departure >= tomorrow)
            .filter_map(|c| c.price.map(|p| (p.minor, c.departure)))
            .min()
            .map(|(_, d)| d)
    }

    /// Runs one check of `family` to the end, with every other check in
    /// flight (tests).
    pub(crate) async fn run_check(
        &mut self,
        family_id: FamilyId,
        reason: QueryReason,
    ) -> anyhow::Result<()> {
        let Some(query) =
            self.launch_check(family_id, reason, None, None).await?
        else {
            return Ok(());
        };
        let mut result = Ok(());
        while let Some(joined) = self.flights.join_next_with_id().await {
            let (landed, r) = self.landed(joined).await;
            if landed == Some(query) {
                result = r;
            }
        }
        result
    }

    /// Starts one check of `family`: writes its query row, says so in
    /// the site's status (0057) and sends its fetch off on a task of its
    /// own (0072). `None` when there was nothing to load.
    async fn launch_check(
        &mut self,
        family_id: FamilyId,
        reason: QueryReason,
        request: Option<RequestId>,
        dated: Option<Dated>,
    ) -> anyhow::Result<Option<QueryId>> {
        // Rows by id, not the whole world: starting a check stays cheap.
        let Some(family) = self.store().family(family_id).await? else {
            anyhow::bail!("no family {family_id}");
        };
        let route = self
            .store()
            .route(family.key.route)
            .await?
            .context("the family's route is not configured")?;
        let pos = self
            .store()
            .point_of_sale(family.key.pos)
            .await?
            .context("the family's point of sale is not configured")?;
        let today = budget::today();
        let latam = if sites::is_latam(&family.site) {
            self.latam_dates(&family, dated.map(|d| d.at)).await?
        } else {
            None
        };
        let options = JobOptions {
            cheapest_departure: match family.key.source {
                Source::GoogleFlights => None,
                _ => self.cheapest_departure(&family).await,
            },
            now: Some(Utc::now()),
            latam,
            ..self.config().jobs.clone()
        };
        let job = match jobs::build(
            &family.key,
            &route,
            &pos,
            self.settings(),
            today,
            &options,
        ) {
            Ok(job) => job,
            Err(e @ JobError::Unresolved(_)) => {
                debug!(family = %family.id, "{e}");
                return Ok(None);
            }
            Err(e) => {
                warn!(family = %family.id, "cannot check: {e}");
                self.mark_checked(family.id, Utc::now()).await?;
                return Ok(None);
            }
        };
        if !self.ensure_browser().await {
            anyhow::bail!("the {} browser is not running", self.site);
        }
        let started = Utc::now();
        let query = self
            .store()
            .start_query(family.id, &reason, started)
            .await?;
        self.begin_check(family.id, query, &reason, started).await;
        info!(
            family = %family.id,
            route = %format!("{}-{}", route.origin.code, route.destination.code),
            cabin = ?family.key.cabin,
            site = %family.site,
            pos = %pos.country,
            kind = ?family.key.kind,
            reason = reason.label(),
            in_flight = self.flights.len() + 1,
            "checking"
        );
        let flight = Flight {
            family,
            route,
            pos,
            query,
            reason,
            request,
            confirms: dated.and_then(|d| d.confirms),
        };
        let expected = flight_cost(&flight.family);
        let sources = Arc::clone(&self.sources);
        let task = self.flights.spawn(async move {
            let (fetch, found) = match &job {
                Job::Calendar(j) => {
                    split(sources.calendar(j).await, Found::Cells)
                }
                Job::Tap(j) => split(sources.tap(j).await, Found::Cells),
                Job::Latam(j) => split(sources.latam(j).await, Found::Latam),
                Job::Explore(j) => {
                    split(sources.explore(j).await, Found::Explore)
                }
            };
            Landed {
                flight,
                fetch,
                found,
            }
        });
        self.flight_ids
            .insert(task.id(), (query, request, expected));
        Ok(Some(query))
    }

    /// Records a fetch that came back (or whose task panicked), rests
    /// its lane and counts it for its request. Returns its query and how
    /// recording went, which is already logged.
    async fn landed(
        &mut self,
        joined: Result<(tokio::task::Id, Landed), JoinError>,
    ) -> (Option<QueryId>, anyhow::Result<()>) {
        self.last_work = Instant::now();
        self.rest();
        let (query, request, result) = match joined {
            Ok((id, landed)) => {
                self.flight_ids.remove(&id);
                let (query, request) =
                    (landed.flight.query, landed.flight.request);
                let family = landed.flight.family.id;
                let result = self.land(landed).await;
                if let Err(e) = &result {
                    warn!(%family, "check failed: {e:#}");
                }
                (Some(query), request, result)
            }
            Err(e) => {
                warn!(site = %self.site, "a check's task failed: {e}");
                let Some((query, request, _)) = self.flight_ids.remove(&e.id())
                else {
                    return (None, Err(e.into()));
                };
                let outcome = QueryOutcome::Error {
                    detail: format!("the check's task failed: {e}"),
                };
                if let Err(e) = self
                    .store()
                    .finish_query(query, &outcome, 0, Utc::now())
                    .await
                {
                    warn!(%query, "closing the query failed: {e}");
                }
                (Some(query), request, Err(e.into()))
            }
        };
        if let Some(query) = query {
            self.end_check(query).await;
        }
        self.finish_request(request).await;
        if self.checks.is_empty()
            && self.has_work(Mode::All)
            && let Some(at) = self.next_rest()
        {
            let wait = at.saturating_duration_since(Instant::now());
            self.next_check_at = Some(
                Utc::now()
                    + chrono::TimeDelta::from_std(wait).unwrap_or_default(),
            );
            self.status().await;
        }
        (query, result)
    }

    /// Records everything a check saw.
    async fn land(&mut self, landed: Landed) -> anyhow::Result<()> {
        let Landed {
            flight:
                Flight {
                    family,
                    route,
                    pos,
                    query,
                    reason,
                    request: _,
                    confirms,
                },
            fetch,
            found,
        } = landed;
        let finished = Utc::now();
        self.store()
            .finish_query(query, &fetch.outcome, fetch.page_loads, finished)
            .await?;
        self.spend(fetch.page_loads).await?;
        self.end_check(query).await;
        info!(
            family = %family.id,
            site = %family.site,
            outcome = ?fetch.outcome,
            loads = fetch.page_loads,
            "checked"
        );
        match &fetch.outcome {
            QueryOutcome::Ok => {}
            QueryOutcome::Blocked { detail } => {
                self.block(detail).await?;
                self.mark_checked(family.id, finished).await?;
                return Ok(());
            }
            QueryOutcome::LoggedOut => {
                if self.is_latam() {
                    // Any LATAM page on the login wall: LATAM Pass's
                    // session ended (0085).
                    self.session_ended(finished).await?;
                } else if let Source::Miles { program } = &family.key.source {
                    warn!(%program, "logged out: the app will ask for a login");
                    self.store()
                        .set_miles_login(program, LoginState::Expired, finished)
                        .await?;
                }
                self.mark_checked(family.id, finished).await?;
                return Ok(());
            }
            // Nothing to sell (0067): not a failure, and no observations.
            // The family waits a week (`candidates::intervals`).
            QueryOutcome::NoFlights => {
                info!(family = %family.id, "no flights: next check in a week");
                self.mark_checked(family.id, finished).await?;
                if let Some(deal) = confirms {
                    info!(deal = %deal, "the award's date has no flights: rejected");
                    self.store()
                        .set_deal_status(deal, DealStatus::Rejected, finished)
                        .await?;
                }
                return Ok(());
            }
            QueryOutcome::Unexpected { .. } | QueryOutcome::Error { .. } => {
                self.mark_checked(family.id, finished).await?;
                return Ok(());
            }
        }
        // The configuration as it is now: other checks landed meanwhile.
        let world = world(self.store()).await?;
        match found {
            Found::Explore(destinations) => {
                self.after_explore(
                    &family,
                    &route,
                    &pos,
                    query,
                    destinations,
                    &world,
                    finished,
                    fetch.page_loads,
                    reason,
                )
                .await
            }
            Found::Cells(cells) => {
                self.after_cells(
                    &family,
                    &route,
                    &pos,
                    query,
                    cells,
                    &world,
                    finished,
                    fetch.page_loads,
                    reason,
                )
                .await
            }
            Found::Latam(found) => {
                let LatamFound {
                    cells,
                    calendar,
                    session,
                    clicks,
                } = found;
                debug!(family = %family.id, clicks, "LATAM strip clicks");
                self.after_cells(
                    &family,
                    &route,
                    &pos,
                    query,
                    cells,
                    &world,
                    finished,
                    fetch.page_loads,
                    reason,
                )
                .await?;
                if let FamilyKind::Leg { leg } = family.key.kind
                    && !calendar.is_empty()
                {
                    self.calendars
                        .insert((route.id, leg), (finished, calendar));
                }
                if let Some(until) = session {
                    self.observe_session(until, finished).await?;
                }
                if let Some(deal) = confirms {
                    self.judge_award(deal, family.id, finished).await?;
                } else {
                    self.evaluate_awards(&family, &route, &pos, query).await?;
                }
                Ok(())
            }
        }
    }

    /// A family's state as stored now: other sites' workers may have
    /// raised it (fan-out, hints) while this check ran.
    async fn current_state(
        &mut self,
        id: FamilyId,
    ) -> anyhow::Result<Option<FamilyStateRecord>> {
        Ok(self.store().family(id).await?.map(|f| f.state))
    }

    /// A check that taught nothing about prices still counts as a visit,
    /// so a broken family is not retried every tick.
    async fn mark_checked(
        &mut self,
        family: FamilyId,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let Some(mut state) = self.current_state(family).await? else {
            return Ok(());
        };
        state.last_check = Some(at);
        self.store().save_family_state(family, &state).await?;
        Ok(())
    }

    /// Folds a successful check into the family's state; returns whether
    /// its window minimum dropped materially.
    async fn learn(
        &mut self,
        family: &FamilyRecord,
        min_price_minor: Option<i64>,
        page_loads: u32,
        reason: QueryReason,
        at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let base = self
            .current_state(family.id)
            .await?
            .unwrap_or_else(|| family.state.clone());
        let mut state = to_state(&base);
        let moved = min_moved(
            state.last_min_price_minor,
            min_price_minor,
            self.params().material_change,
        );
        let summary = CheckSummary {
            min_price_minor,
            any_material_change: false,
            drop: false,
            page_loads,
            reason,
        };
        update_after_check_with(&mut state, &summary, at, self.params());
        self.store()
            .save_family_state(family.id, &to_record(&state, &base))
            .await?;
        Ok(moved.dropped)
    }

    /// Adds one pending hint to the families related to one that
    /// dropped (0028: probe, then fan out), on every site.
    async fn fan_out(
        &mut self,
        dropped: &FamilyRecord,
        world: &World,
    ) -> anyhow::Result<()> {
        let related = fanout::related(
            &dropped.key,
            world.families.iter().map(|f| &f.key),
        );
        for (key, relation) in related {
            let Some(id) = world.family_by_key(&key).map(|f| f.id) else {
                continue;
            };
            if let Some(f) = self.store().family(id).await? {
                let mut state = to_state(&f.state);
                fanout::boost(&mut state);
                self.store()
                    .save_family_state(id, &to_record(&state, &f.state))
                    .await?;
                debug!(family = %id, ?relation, "fan-out boost");
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn after_cells(
        &mut self,
        family: &FamilyRecord,
        route: &Route,
        pos: &PointOfSale,
        query: QueryId,
        cells: Vec<Cell>,
        world: &World,
        at: DateTime<Utc>,
        page_loads: u32,
        reason: QueryReason,
    ) -> anyhow::Result<()> {
        let observations: Vec<Observation> = cells
            .into_iter()
            .map(|c| Observation {
                id: None,
                query,
                family: family.id,
                observed_at: at,
                departure: c.departure,
                return_date: c.return_date,
                price: c.price,
                miles: c.miles,
                carrier: c.carrier,
                stops: c.stops,
                duration_minutes: c.duration_minutes,
                seller: c.seller,
                flights: c.flights,
                award: c.award,
            })
            .collect();
        if !observations.is_empty() {
            self.store().insert_observations(&observations).await?;
        }
        let miles = matches!(family.key.source, Source::Miles { .. });
        let min = observations
            .iter()
            .filter_map(|o| {
                if miles {
                    o.miles.or(o.price.as_ref().map(|p| p.minor))
                } else {
                    o.price.as_ref().map(|p| p.minor)
                }
            })
            .min();
        let dropped = self.learn(family, min, page_loads, reason, at).await?;
        if dropped {
            info!(family = %family.id, "material drop: fanning out");
            self.fan_out(family, world).await?;
        }
        if family.key.source == Source::GoogleFlights
            && family.key.kind != FamilyKind::Explore
        {
            self.evaluate_deals(route, family.key.cabin, pos, family.key.kind)
                .await?;
        }
        Ok(())
    }

    /// The minimum a hint is compared with for a route and cabin: the
    /// round trip's last minimum, else the sum of the legs' (0041).
    fn reference_min(
        world: &World,
        route: RouteId,
        cabin: Cabin,
    ) -> Option<i64> {
        let of = |pred: &dyn Fn(&FamilyKind) -> bool| {
            world
                .families
                .iter()
                .filter(|f| {
                    f.key.route == route
                        && f.key.cabin == cabin
                        && f.key.source == Source::GoogleFlights
                        && pred(&f.key.kind)
                })
                .filter_map(|f| f.state.last_min_price_minor)
                .min()
        };
        of(&|k| matches!(k, FamilyKind::RoundTrip { .. })).or_else(|| {
            Some(
                of(&|k| *k == FamilyKind::Leg { leg: Leg::Outbound })?
                    + of(&|k| *k == FamilyKind::Leg { leg: Leg::Return })?,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn after_explore(
        &mut self,
        family: &FamilyRecord,
        sweep: &Route,
        pos: &PointOfSale,
        query: QueryId,
        destinations: Vec<ExploreDestination>,
        world: &World,
        at: DateTime<Utc>,
        page_loads: u32,
        reason: QueryReason,
    ) -> anyhow::Result<()> {
        let hints: Vec<ExploreHint> = destinations
            .iter()
            .filter_map(|d| {
                Some(ExploreHint {
                    query,
                    origin: sweep.origin.id,
                    cabin: family.key.cabin,
                    pos: pos.id,
                    observed_at: at,
                    destination_freebase_id: d.freebase_id.clone(),
                    destination_airport: d.airport.clone().unwrap_or_default(),
                    price: d.money(&pos.currency)?,
                    departure: d.departure,
                    return_date: d.return_date,
                    stops: d.stops,
                    duration_minutes: d.duration_minutes,
                    carrier: d
                        .carrier
                        .clone()
                        .or_else(|| d.carrier_code.clone()),
                })
            })
            .collect();
        if !hints.is_empty() {
            self.store().insert_explore_hints(&hints).await?;
        }
        let targets: Vec<HintTarget> = world
            .families
            .iter()
            .filter(|f| {
                f.key.kind != FamilyKind::Explore
                    && f.key.cabin == family.key.cabin
            })
            .filter_map(|f| {
                let route = world.routes.get(&f.key.route)?;
                (route.origin.id == sweep.origin.id
                    && !world.sweeps.contains(&route.id))
                .then(|| HintTarget {
                    family: f.id,
                    origin: route.origin.id,
                    cabin: f.key.cabin,
                    destination_code: route.destination.code.clone(),
                    destination_airports: Vec::new(),
                    destination_freebase_id: match &route.destination.kind {
                        PlaceKind::City { freebase_id } => freebase_id.clone(),
                        PlaceKind::Airport => None,
                    },
                    reference_min_minor: Self::reference_min(
                        world,
                        route.id,
                        f.key.cabin,
                    ),
                })
            })
            .collect();
        let hinted = hinted_families(&hints, &targets, self.params());
        info!(
            destinations = destinations.len(),
            hints = hints.len(),
            hinted = hinted.len(),
            "Explore recorded"
        );
        for id in hinted {
            if let Some(f) = self.store().family(id).await? {
                let mut state = to_state(&f.state);
                add_hint(&mut state);
                self.store()
                    .save_family_state(id, &to_record(&state, &f.state))
                    .await?;
            }
        }
        let min = hints.iter().map(|h| h.price.minor).min();
        self.learn(family, min, page_loads, reason, at).await?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // LATAM (0084, 0085)
    // -----------------------------------------------------------------

    /// The home point of sale's family of `key`'s route, cabin and kind
    /// on `source`.
    async fn sibling(
        &self,
        family: &FamilyRecord,
        source: Source,
        kind: FamilyKind,
    ) -> anyhow::Result<Option<FamilyId>> {
        let Some(home) = self.store().home_point_of_sale().await? else {
            return Ok(None);
        };
        Ok(self
            .store()
            .family_id(&FamilyKey {
                route: family.key.route,
                cabin: family.key.cabin,
                source,
                pos: home.id,
                kind,
            })
            .await?)
    }

    /// One leg's cash per departure for choosing LATAM dates: Google
    /// Flights' latest graph, LATAM's own cash read in the last
    /// [`LATAM_READ_DAYS`] (it wins: it is the airline's live price), and
    /// the cached calendar a LATAM page fetched in the last two days
    /// where neither has the day. Minor units.
    async fn leg_cash(
        &self,
        family: &FamilyRecord,
        leg: Leg,
        now: DateTime<Utc>,
    ) -> anyhow::Result<BTreeMap<NaiveDate, i64>> {
        let kind = FamilyKind::Leg { leg };
        let mut cash: BTreeMap<NaiveDate, i64> = BTreeMap::new();
        if let Some((at, calendar)) =
            self.calendars.get(&(family.key.route, leg))
            && now - *at <= chrono::TimeDelta::hours(FRESH_HOURS)
        {
            cash.extend(calendar.iter().map(|(d, p)| (*d, *p)));
        }
        if let Some(id) =
            self.sibling(family, Source::GoogleFlights, kind).await?
        {
            cash.extend(deals::leg_prices(
                &self.store().latest_cells(id).await?,
            ));
        }
        let airline = Source::Airline {
            code: latam::CODE.to_owned(),
        };
        if let Some(id) = self.sibling(family, airline, kind).await? {
            let since = now - chrono::TimeDelta::days(LATAM_READ_DAYS);
            let mut own: BTreeMap<NaiveDate, (DateTime<Utc>, i64)> =
                BTreeMap::new();
            for o in self.store().history(id, since).await? {
                if let (Some(p), None) = (&o.price, o.return_date) {
                    own.insert(o.departure, (o.observed_at, p.minor));
                }
            }
            cash.extend(own.into_iter().map(|(d, (_, p))| (d, p)));
        }
        Ok(cash)
    }

    /// The departures `family` read in the last [`LATAM_READ_DAYS`], and
    /// when it last read each.
    async fn read_dates(
        &self,
        family: FamilyId,
        now: DateTime<Utc>,
    ) -> anyhow::Result<BTreeMap<NaiveDate, DateTime<Utc>>> {
        let since = now - chrono::TimeDelta::days(LATAM_READ_DAYS);
        let mut read = BTreeMap::new();
        for o in self.store().history(family, since).await? {
            read.entry(o.departure)
                .and_modify(|at: &mut DateTime<Utc>| {
                    *at = (*at).max(o.observed_at)
                })
                .or_insert(o.observed_at);
        }
        Ok(read)
    }

    /// Which dates a LATAM check of `family` reads (0085): `at` when
    /// given, else the anchor and strip [`latam_dates`] chooses from
    /// the cash in the database, what the family read lately and, for
    /// awards, the program's setting and its weekly spot check. Strip
    /// clicks are cut to what today's budget has left.
    async fn latam_dates(
        &mut self,
        family: &FamilyRecord,
        at: Option<AwardKey>,
    ) -> anyhow::Result<Option<LatamDates>> {
        if let Some((departure, return_date)) = at {
            return Ok(Some(LatamDates {
                departure,
                return_date,
                strip: Strip::none(),
            }));
        }
        let now = Utc::now();
        let today = budget::today();
        let first = jobs::first_departure(today);
        let last = jobs::horizon_end(today, self.settings());
        let cash = match family.key.kind {
            FamilyKind::Leg { leg } => self.leg_cash(family, leg, now).await?,
            FamilyKind::RoundTrip { days } => {
                // The round trip's cash: Google Flights' real round
                // trips, else the two legs' sum.
                let mut cash: BTreeMap<NaiveDate, i64> = BTreeMap::new();
                let out = self.leg_cash(family, Leg::Outbound, now).await?;
                let back = self.leg_cash(family, Leg::Return, now).await?;
                for (d, p) in &out {
                    if let Some(r) =
                        back.get(&(*d + Days::new(u64::from(days))))
                    {
                        cash.insert(*d, p + r);
                    }
                }
                let kind = FamilyKind::RoundTrip { days };
                if let Some(id) =
                    self.sibling(family, Source::GoogleFlights, kind).await?
                {
                    for ((d, _), p) in deals::round_trip_prices(
                        &self.store().latest_cells(id).await?,
                    ) {
                        cash.insert(d, p);
                    }
                }
                cash
            }
            FamilyKind::Explore => return Ok(None),
        };
        let known = Known {
            first,
            last,
            cash,
            read: self.read_dates(family.id, now).await?,
        };
        let pick = match &family.key.source {
            Source::Miles { program } => {
                let dates = self
                    .store()
                    .list_miles_programs()
                    .await?
                    .into_iter()
                    .find(|p| &p.program == program)
                    .map_or(AwardDates::default(), |p| p.dates);
                let spot_due = self.last_spot.get(&family.id).is_none_or(|t| {
                    now - *t >= chrono::TimeDelta::days(latam_dates::SPOT_DAYS)
                });
                Pick::of(dates, spot_due)
            }
            _ => Pick::Cheap,
        };
        let Some((departure, used)) = latam_dates::anchor(pick, &known, now)
        else {
            return Ok(None);
        };
        let award = matches!(family.key.source, Source::Miles { .. });
        if award && used == Pick::Spot {
            self.last_spot.insert(family.id, now);
        }
        let return_date = match family.key.kind {
            FamilyKind::RoundTrip { days } => {
                Some(departure + Days::new(u64::from(days)))
            }
            _ => None,
        };
        // Strip clicks are for awards, which have no calendar; cash has
        // one and reads one date a check (0085). Each click is a load.
        let left = self
            .budget
            .daily(now, &self.loaded.params)
            .saturating_sub(self.budget.loads_today)
            .saturating_sub(1);
        let strip = if award && return_date.is_none() {
            Strip::plan(used, &known, now, left)
        } else {
            Strip::none()
        };
        info!(
            family = %family.id,
            %departure,
            pick = ?used,
            clicks = strip.max_clicks,
            "LATAM dates chosen"
        );
        Ok(Some(LatamDates {
            departure,
            return_date,
            strip,
        }))
    }

    /// LATAM Pass's login state as stored.
    async fn latam_login(&self) -> anyhow::Result<Option<LoginState>> {
        Ok(self
            .store()
            .list_miles_programs()
            .await?
            .into_iter()
            .find(|p| p.program == latam::PROGRAM)
            .map(|p| p.login_state))
    }

    /// Reads LATAM Pass's session from the profile's cookie jar while no
    /// Chromium runs on it (the worker's start, 0064, 0085).
    async fn read_profile_session(&mut self) {
        let Some(dir) = self.profile_dir() else {
            return;
        };
        if self.sources.is_running() {
            return;
        }
        let now = Utc::now();
        let domains = tm_domain::miles_session_domains(latam::PROGRAM);
        match tm_browser::profile::login_cookies(&dir, domains, now).await {
            Ok(cookies) => {
                let until = cookies.iter().filter_map(|c| c.expires).max();
                info!(profile = %dir.display(), ?until, "LATAM Pass session in the profile");
                if let Err(e) = self.observe_session(until, now).await {
                    warn!("recording LATAM Pass's session failed: {e:#}");
                }
            }
            Err(e) => {
                warn!(profile = %dir.display(), "LATAM Pass's session not read: {e}")
            }
        }
    }

    /// Records LATAM Pass's session as its cookie shows it (0082): when
    /// it ends, and whether there is one. A profile with a live session
    /// counts as logged in unless an earlier check found it expired
    /// (only the login window clears that, 0064); one without has
    /// ended.
    async fn observe_session(
        &mut self,
        until: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.store()
            .set_miles_session_until(latam::PROGRAM, until)
            .await?;
        match (until, self.latam_login().await?) {
            (Some(_), Some(LoginState::LoggedOut)) => {
                info!("LATAM Pass's profile holds a session: logged in");
                self.store()
                    .set_miles_login(latam::PROGRAM, LoginState::LoggedIn, at)
                    .await?;
            }
            (None, _) => self.session_ended(at).await?,
            _ => {}
        }
        Ok(())
    }

    /// LATAM Pass's session ended: its award families wait for a login
    /// (`candidates::Skip::LoggedOut`), and the user is told once per
    /// expiry, since checks run with the app closed (0083, 0085).
    async fn session_ended(&mut self, at: DateTime<Utc>) -> anyhow::Result<()> {
        if self.latam_login().await? != Some(LoginState::LoggedIn) {
            return Ok(());
        }
        warn!("LATAM Pass's session ended: award checks wait for a login");
        self.store()
            .set_miles_login(latam::PROGRAM, LoginState::Expired, at)
            .await?;
        if let Some(notifier) = self.config().notifier.clone() {
            let notice = notify::login_notice(latam::PROGRAM);
            match tokio::task::spawn_blocking(move || notifier.show(&notice))
                .await
            {
                Ok(Ok(())) => info!("login notice shown"),
                Ok(Err(e)) => warn!("the login notice was not shown: {e:#}"),
                Err(e) => warn!("the login notice task failed: {e}"),
            }
        }
        Ok(())
    }

    /// The Google Flights cash price of an award's leg (or round trip)
    /// and dates, under the home point of sale, for scale (0083).
    async fn award_cash(
        &self,
        family: &FamilyRecord,
        (departure, return_date): AwardKey,
    ) -> anyhow::Result<Option<Money>> {
        let Some(id) = self
            .sibling(family, Source::GoogleFlights, family.key.kind)
            .await?
        else {
            return Ok(None);
        };
        Ok(self
            .store()
            .latest_cells(id)
            .await?
            .into_iter()
            .filter(|c| {
                c.departure == departure && c.return_date == return_date
            })
            .filter_map(|c| c.price)
            .min_by_key(|p| p.minor))
    }

    /// The award series' deals after a LATAM Pass check (0083, 0085):
    /// confirmed deals whose miles rose are expired; the check's awards
    /// that beat the series' own baseline become candidates, each
    /// confirmed by reading its dates again.
    async fn evaluate_awards(
        &mut self,
        family: &FamilyRecord,
        route: &Route,
        pos: &PointOfSale,
        query: QueryId,
    ) -> anyhow::Result<()> {
        let Source::Miles { program } = &family.key.source else {
            return Ok(());
        };
        let leg = match family.key.kind {
            FamilyKind::Leg { leg } => Some(leg),
            FamilyKind::RoundTrip { .. } => None,
            FamilyKind::Explore => return Ok(()),
        };
        let store = self.store().clone();
        let now = Utc::now();
        let today = budget::today();
        let params = self.loaded.params.clone();
        let cells: Vec<Observation> = store
            .latest_cells(family.id)
            .await?
            .into_iter()
            .filter(|c| c.query == query)
            .collect();
        let current = cheapest_by_dates(&cells);
        if current.is_empty() {
            return Ok(());
        }
        let history = store
            .history(family.id, now - params.baseline_window)
            .await?;
        let points = award_points(&history, family.id, &[query].into());
        let first = today + Days::new(1);
        let last = jobs::horizon_end(today, self.settings());
        let found =
            deals::find_awards(&current, &points, now, &params, first, last);
        let series = Series::Award {
            program: program.clone(),
        };
        let cabin = family.key.cabin;
        let known = store
            .list_deals(&DealFilter {
                limit: Some(1000),
                ..DealFilter::default()
            })
            .await?;
        for d in known.iter().filter(|d| {
            d.status == DealStatus::Confirmed
                && d.series == series
                && d.route == route.id
                && d.cabin == cabin
                && d.leg == leg
        }) {
            if deals::award_expired(d, &current, today)
                && let Some(id) = d.id
            {
                info!(deal = %id, "award deal expired");
                store.set_deal_status(id, DealStatus::Expired, now).await?;
            }
        }
        let picked = deals::select_awards(
            found,
            &known,
            &series,
            (route.id, cabin, leg),
            now,
            AWARD_CONFIRMS,
        );
        for f in picked {
            let dates = (f.award.departure, f.award.return_date);
            let cash = self.award_cash(family, dates).await?;
            let mut deal = deals::award_candidate(
                &f, program, leg, route.id, cabin, pos, cash, now,
            );
            deal.url =
                award_url(route, leg, dates, &self.settings().passengers);
            let id = store.insert_deal(&deal).await?;
            info!(
                deal = %id,
                route = %route.destination.code,
                ?leg,
                departure = %dates.0,
                miles = ?deal.miles,
                change = f.change,
                "award candidate: reading its date again"
            );
            self.manual.push_front(Manual::ConfirmAward(id));
        }
        Ok(())
    }

    /// Starts an award candidate's confirmation: its family's dates read
    /// again, one search (0085).
    async fn confirm_award(&mut self, id: DealId) -> anyhow::Result<()> {
        let Some(deal) = self.store().deal(id).await? else {
            return Ok(());
        };
        if deal.status != DealStatus::Candidate {
            return Ok(());
        }
        let Series::Award { program } = &deal.series else {
            return Ok(());
        };
        let kind = match (deal.leg, deal.return_date) {
            (Some(leg), _) => FamilyKind::Leg { leg },
            (None, Some(r)) => FamilyKind::RoundTrip {
                days: u16::try_from((r - deal.departure).num_days())
                    .unwrap_or(0),
            },
            (None, None) => return Ok(()),
        };
        let Some(family) = self
            .store()
            .family_id(&FamilyKey {
                route: deal.route,
                cabin: deal.cabin,
                source: Source::Miles {
                    program: program.clone(),
                },
                pos: deal.pos,
                kind,
            })
            .await?
        else {
            return Ok(());
        };
        let dated = Dated {
            at: (deal.departure, deal.return_date),
            confirms: Some(id),
        };
        self.launch(family, QueryReason::Confirmation, None, Some(dated))
            .await;
        Ok(())
    }

    /// Judges an award candidate against the check that read its dates
    /// again: it holds when the cheapest award of those dates still costs
    /// at most [`deals::HOLD_TOLERANCE`] times its miles. A held deal is
    /// announced (0080).
    async fn judge_award(
        &mut self,
        id: DealId,
        family: FamilyId,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let store = self.store().clone();
        let Some(deal) = store.deal(id).await? else {
            return Ok(());
        };
        if deal.status != DealStatus::Candidate {
            return Ok(());
        }
        let cells = store.latest_cells(family).await?;
        let current = cheapest_by_dates(&cells);
        let fresh = current
            .get(&(deal.departure, deal.return_date))
            .and_then(|o| o.miles);
        let held = match (fresh, deal.miles) {
            (Some(now), Some(was)) => {
                now as f64 <= was as f64 * deals::HOLD_TOLERANCE
            }
            _ => false,
        };
        let status = if held {
            DealStatus::Confirmed
        } else {
            DealStatus::Rejected
        };
        store.set_deal_status(id, status, at).await?;
        info!(deal = %id, ?status, candidate = ?deal.miles, read_again = ?fresh, "award confirmation");
        if held && let Err(e) = self.notify_deals().await {
            warn!("cannot announce deals: {e:#}");
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Deals (0030, 0031, 0042, 0043)
    // -----------------------------------------------------------------

    /// Re-prices the trips of one route × cabin × point of sale after a
    /// check of one of its Google Flights families, learns the
    /// round-trip discount, expires confirmed deals that are gone, and
    /// confirms new candidates.
    async fn evaluate_deals(
        &mut self,
        route: &Route,
        cabin: Cabin,
        pos: &PointOfSale,
        touched: FamilyKind,
    ) -> anyhow::Result<()> {
        let now = Utc::now();
        let today = budget::today();
        let key = |kind| FamilyKey {
            route: route.id,
            cabin,
            source: Source::GoogleFlights,
            pos: pos.id,
            kind,
        };
        let days = self.settings().trips.round_trip_days;
        let store = self.store().clone();
        let (Some(outbound), Some(ret)) = (
            store
                .family_id(&key(FamilyKind::Leg { leg: Leg::Outbound }))
                .await?,
            store
                .family_id(&key(FamilyKind::Leg { leg: Leg::Return }))
                .await?,
        ) else {
            return Ok(());
        };
        let round_trip = store
            .family_id(&key(FamilyKind::RoundTrip { days }))
            .await?;
        let fresh = |cells: Vec<Observation>| -> Vec<Observation> {
            cells
                .into_iter()
                .filter(|c| {
                    now - c.observed_at <= chrono::TimeDelta::hours(FRESH_HOURS)
                })
                .collect()
        };
        let out_cells = fresh(store.latest_cells(outbound).await?);
        let ret_cells = fresh(store.latest_cells(ret).await?);
        let rt_cells = match round_trip {
            Some(id) => fresh(store.latest_cells(id).await?),
            None => Vec::new(),
        };
        let out = deals::leg_prices(&out_cells);
        let back = deals::leg_prices(&ret_cells);
        let rt = deals::round_trip_prices(&rt_cells);
        let mut discount = to_discount(
            &store.route_trip_state(route.id).await?.unwrap_or_default(),
        );
        let params = self.loaded.params.clone();
        let settings = self.loaded.settings.clone();
        if matches!(touched, FamilyKind::RoundTrip { .. }) && !rt.is_empty() {
            let pairs = compare_round_trips(&rt, &out, &back);
            if !pairs.is_empty() {
                let was = discount.additive;
                discount.observe_pairs(&pairs, &params);
                store
                    .save_route_trip_state(route.id, &from_discount(&discount))
                    .await?;
                info!(
                    route = %route.destination.code,
                    pairs = pairs.len(),
                    discount = discount.ewma,
                    additive = discount.additive,
                    "round-trip discount learned"
                );
                if discount.additive && !was {
                    info!(route = %route.destination.code, "round trips match their legs: tracking stops");
                }
            }
        }
        let range = settings.trips;
        let estimated = build_trips(&out, &back, range, discount.discount());
        let real = deals::real_trips(&rt);
        let trips: Vec<(tm_scheduler::Trip, bool)> = estimated
            .iter()
            .map(|t| (*t, true))
            .chain(real.iter().map(|t| (*t, false)))
            .collect();
        if trips.is_empty() {
            return Ok(());
        }
        let history = store
            .history_for_route(route.id, cabin, now - params.baseline_window)
            .await?;
        let group = Group {
            outbound,
            ret,
            round_trip: round_trip.map(|id| (id, days)),
        };
        let exclude: HashSet<QueryId> = out_cells
            .iter()
            .chain(&ret_cells)
            .chain(&rt_cells)
            .map(|c| c.query)
            .collect();
        let points = deals::history_points(
            &history,
            &group,
            range,
            discount.discount(),
            &exclude,
        );
        let first = today + Days::new(1);
        let last = jobs::horizon_end(today, &settings);
        let found = deals::find(&trips, &points, now, &params, first, last);
        let known = store
            .list_deals(&DealFilter {
                limit: Some(1000),
                ..DealFilter::default()
            })
            .await?;
        let mut current: BTreeMap<(NaiveDate, NaiveDate), i64> =
            BTreeMap::new();
        for (t, _) in &trips {
            current
                .entry((t.departure, t.return_date))
                .and_modify(|p| *p = (*p).min(t.price_minor))
                .or_insert(t.price_minor);
        }
        for d in known.iter().filter(|d| {
            d.status == DealStatus::Confirmed
                && d.route == route.id
                && d.cabin == cabin
                && d.pos == pos.id
        }) {
            if deals::expired(d, &current, today)
                && let Some(id) = d.id
            {
                info!(deal = %id, "deal expired");
                store.set_deal_status(id, DealStatus::Expired, now).await?;
            }
        }
        let picked = deals::select(
            found,
            &known,
            (route.id, cabin, pos.id),
            now,
            deals::CONFIRMS_PER_GROUP,
        );
        for f in picked {
            let url = route_search(
                route,
                cabin,
                f.trip.departure,
                f.trip.return_date,
                &settings,
            )
            .map(|s| {
                jobs::rebase(
                    &search_url(&s, &jobs::locale(&settings, pos)),
                    None,
                )
            });
            let deal =
                deals::candidate(&f, route.id, cabin, pos, now, &params, url);
            let id = store.insert_deal(&deal).await?;
            info!(
                deal = %id,
                route = %route.destination.code,
                departure = %f.trip.departure,
                return_date = %f.trip.return_date,
                price = %deal.price,
                change = f.change,
                reason = ?f.reason,
                estimated = f.estimated,
                "deal candidate: confirming"
            );
            let legs = f.estimated.then_some(f.trip.legs_sum_minor);
            if let Err(e) = self.confirm(id, &deal, legs).await {
                warn!(deal = %id, "confirmation failed: {e:#}");
            }
        }
        Ok(())
    }

    /// The second query of a candidate (0031): the round trip for its
    /// exact dates under its point of sale, then that point of sale's
    /// booking page (0034). Logged as a `Confirmation` query of the
    /// route's round-trip family (or its outbound leg).
    async fn confirm(
        &mut self,
        id: DealId,
        deal: &Deal,
        legs_sum: Option<i64>,
    ) -> anyhow::Result<()> {
        let before = self.checks.len();
        let result = self.confirm_deal(id, deal, legs_sum).await;
        // The confirmation's own entry, should it have ended early.
        if self.checks.len() > before {
            self.checks.truncate(before);
            self.status().await;
        }
        if let Err(e) = self.notify_deals().await {
            warn!("cannot announce deals: {e:#}");
        }
        result
    }

    /// Shows a desktop notification for each confirmed deal not shown
    /// yet, oldest first, and marks it seen (0080). One that cannot be
    /// shown stays unseen and is tried after the next confirmation. Each
    /// worker announces its own: Google Flights the cash deals, LATAM
    /// the award deals (0085), so no deal is announced twice.
    async fn notify_deals(&self) -> anyhow::Result<()> {
        let Some(notifier) = self.config().notifier.clone() else {
            return Ok(());
        };
        let store = self.store();
        let values: HashMap<String, Money> = store
            .list_miles_programs()
            .await?
            .into_iter()
            .filter_map(|p| Some((p.program, p.value_per_1000?)))
            .collect();
        for deal in store.unseen_confirmed().await? {
            if deal.series.is_award() != self.is_latam() {
                continue;
            }
            let (Some(id), Some(route)) =
                (deal.id, store.route(deal.route).await?)
            else {
                continue;
            };
            let notice = match deal.series.program() {
                Some(program) => {
                    notify::award_notice(&deal, &route, values.get(program))
                }
                None => notify::notice(&deal, &route),
            };
            let notifier = Arc::clone(&notifier);
            tokio::task::spawn_blocking(move || notifier.show(&notice))
                .await
                .context("the notification task failed")??;
            store.mark_deal_seen(id).await?;
            info!(deal = %id, "deal announced");
        }
        Ok(())
    }

    async fn confirm_deal(
        &mut self,
        id: DealId,
        deal: &Deal,
        legs_sum: Option<i64>,
    ) -> anyhow::Result<()> {
        // Award deals (0083) are not confirmed on Google Flights.
        let (tm_domain::Series::Cash, Some(return_date)) =
            (&deal.series, deal.return_date)
        else {
            return Ok(());
        };
        if !self.is_google()
            || self.budget.is_cooling(Utc::now())
            || !self.ensure_browser().await
        {
            info!(deal = %id, "cannot confirm now: left as a candidate");
            return Ok(());
        }
        let store = self.store().clone();
        let settings = self.loaded.settings.clone();
        let params = self.loaded.params.clone();
        let route = store.route(deal.route).await?.context("no route")?;
        let pos = store
            .point_of_sale(deal.pos)
            .await?
            .context("no point of sale")?;
        let Some(search) = route_search(
            &route,
            deal.cabin,
            deal.departure,
            return_date,
            &settings,
        ) else {
            return Ok(());
        };
        let days = settings.trips.round_trip_days;
        let key = |kind| FamilyKey {
            route: route.id,
            cabin: deal.cabin,
            source: Source::GoogleFlights,
            pos: pos.id,
            kind,
        };
        let rt_family = store
            .family_id(&key(FamilyKind::RoundTrip { days }))
            .await?;
        let log_family = match rt_family {
            Some(id) => id,
            None => store
                .family_id(&key(FamilyKind::Leg { leg: Leg::Outbound }))
                .await?
                .context("no family to log the confirmation on")?,
        };
        let now = Utc::now();
        let query = store
            .start_query(log_family, &QueryReason::Confirmation, now)
            .await?;
        self.begin_check(log_family, query, &QueryReason::Confirmation, now)
            .await;
        let job = ConfirmJob {
            pos: pos.id,
            search,
            locale: jobs::locale(&settings, &pos),
            origin_override: self.config().jobs.origin_override.clone(),
        };
        let fetch = self.sources.confirm(&job).await;
        self.last_work = Instant::now();
        let finished = Utc::now();
        store
            .finish_query(query, &fetch.outcome, fetch.page_loads, finished)
            .await?;
        self.spend(fetch.page_loads).await?;
        self.end_check(query).await;
        match &fetch.outcome {
            QueryOutcome::Ok => {}
            QueryOutcome::Blocked { detail } => {
                self.block(detail).await?;
                return Ok(());
            }
            other => {
                warn!(deal = %id, outcome = ?other, "confirmation did not load: left as a candidate");
                return Ok(());
            }
        }
        let Some(confirmation) = fetch.value else {
            return Ok(());
        };
        let fx = FxTable::new(
            store
                .latest_exchange_rates()
                .await?
                .into_iter()
                .map(|r| (r.currency.0, r.brl_per_unit)),
        );
        let Some(verdict) =
            deals::judge(deal.price.minor, &confirmation, &pos, &fx, &params)
        else {
            info!(deal = %id, "no price on the results page: rejected");
            store
                .set_deal_status(id, DealStatus::Rejected, finished)
                .await?;
            return Ok(());
        };
        for (currency, rate) in &verdict.rates {
            store.set_exchange_rate(currency, *rate, finished).await?;
        }
        store
            .update_deal_quote(
                id,
                &verdict.price,
                &verdict.effective,
                confirmation.carrier.as_deref(),
                verdict.seller.as_deref(),
                Some(
                    confirmation
                        .booking_url
                        .as_deref()
                        .unwrap_or(&confirmation.search_url),
                ),
            )
            .await?;
        if !verdict.quotes.is_empty() {
            store.attach_seller_quotes(id, &verdict.quotes).await?;
        }
        let status = if verdict.held {
            DealStatus::Confirmed
        } else {
            DealStatus::Rejected
        };
        store.set_deal_status(id, status, finished).await?;
        info!(
            deal = %id,
            ?status,
            candidate = %deal.price,
            confirmed = %verdict.price,
            effective = %verdict.effective,
            seller = ?verdict.seller,
            sellers = verdict.quotes.len(),
            "deal confirmation"
        );
        // A confirmation is a real round trip next to its legs: it
        // teaches the discount, and resumes round-trip tracking when it
        // shows a gap (0042).
        if let Some(legs) = legs_sum {
            let mut discount = to_discount(
                &store.route_trip_state(route.id).await?.unwrap_or_default(),
            );
            discount.observe(verdict.price.minor, legs, &params);
            store
                .save_route_trip_state(route.id, &from_discount(&discount))
                .await?;
        }
        let length = (return_date - deal.departure).num_days();
        if rt_family == Some(log_family) && length == i64::from(days) {
            store
                .insert_observations(&[Observation {
                    id: None,
                    query,
                    family: log_family,
                    observed_at: finished,
                    departure: deal.departure,
                    return_date: Some(return_date),
                    price: Some(Money::brl_whole(verdict.price.whole())),
                    miles: None,
                    carrier: confirmation.carrier.clone(),
                    stops: None,
                    duration_minutes: None,
                    seller: verdict.seller.clone(),
                    flights: None,
                    award: None,
                }])
                .await?;
        }
        Ok(())
    }
}

/// The loads a check of `family` is expected to cost, held back from the
/// bank while it runs (0079).
fn flight_cost(family: &FamilyRecord) -> u32 {
    let cost = family.state.cost_loads;
    if cost.is_finite() && cost > 0.0 {
        cost.ceil() as u32
    } else {
        1
    }
}

/// The round-trip search for a route's exact dates.
fn route_search(
    route: &Route,
    cabin: Cabin,
    departure: NaiveDate,
    return_date: NaiveDate,
    settings: &Settings,
) -> Option<Search> {
    Some(Search::round_trip(
        PlaceRef::from_place(&route.origin)?,
        PlaceRef::from_place(&route.destination)?,
        departure,
        return_date,
        cabin,
        settings.passengers,
    ))
}

/// The LATAM Pass results page of an award deal's leg (or round trip)
/// and dates, for the app's link.
fn award_url(
    route: &Route,
    leg: Option<Leg>,
    (departure, return_date): AwardKey,
    passengers: &tm_domain::Passengers,
) -> Option<String> {
    let (from, to) = match leg {
        Some(Leg::Return) => (&route.destination, &route.origin),
        _ => (&route.origin, &route.destination),
    };
    latam::Search {
        origin: latam::airport(&from.code).to_owned(),
        destination: latam::airport(&to.code).to_owned(),
        outbound: departure,
        inbound: return_date,
        passengers: *passengers,
        cabin: Cabin::Economy,
        redemption: true,
    }
    .url(latam::BASE)
}

/// A fetch's bookkeeping, and its value as a [`Found`].
fn split<T>(f: Fetch<T>, found: impl FnOnce(T) -> Found) -> (Fetch<()>, Found) {
    let Fetch {
        outcome,
        value,
        page_loads,
    } = f;
    (
        Fetch {
            outcome,
            value: (),
            page_loads,
        },
        found(value),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn manual_checks_rotate_across_requests() {
        use std::collections::VecDeque;

        use tm_domain::{FamilyId, RequestId};

        use super::{Manual, next_manual_index};

        // Request 1 queued three checks, then request 2 queued two.
        let mut q: VecDeque<Manual> = [(1, 1), (2, 1), (3, 1), (4, 2), (5, 2)]
            .into_iter()
            .map(|(f, r)| Manual::Check(FamilyId(f), Some(RequestId(r))))
            .collect();
        let mut last = None;
        let mut order = Vec::new();
        while !q.is_empty() {
            let i = next_manual_index(&q, last);
            let m = q.remove(i).expect("item");
            last = m.request();
            if let Manual::Check(f, _) = m {
                order.push(f.0);
            }
        }
        assert_eq!(order, vec![1, 4, 2, 5, 3]);
    }
}
