//! The loop (DECISIONS.md 0028, 0029, 0035, 0039, 0046): reload the
//! configuration, take the app's requests, ask the scheduler for this
//! tick's picks, run each through its source, record what it saw,
//! update the scheduler's state, find and confirm deals, and keep the
//! heartbeat.
//!
//! Everything runs on one task, one check at a time: the browser is
//! shared, and Google sees one visitor reading at a human pace
//! (random pauses between page loads, 0039).

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    time::Duration,
};

use anyhow::Context as _;
use chrono::{DateTime, Days, NaiveDate, Utc};
use rand::{Rng, SeedableRng, rngs::StdRng};
use tm_domain::{
    Cabin,
    Deal,
    DealId,
    DealStatus,
    ExploreHint,
    FamilyId,
    FamilyKey,
    FamilyKind,
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
    Settings,
    Source,
};
use tm_gflights::{PlaceRef, Search, rpc::ExploreDestination, url::search_url};
use tm_scheduler::{
    CheckSummary,
    FxTable,
    Params,
    fanout,
    hints::{HintTarget, add_hint, hinted_families},
    plan_tick,
    trips::{build_trips, compare_round_trips},
    update::{min_moved, update_after_check_with},
};
use tm_store::{
    DealFilter,
    FamilyRecord,
    Heartbeat,
    LoginState,
    RequestKind,
    RequestRecord,
    Store,
};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::{
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
    jobs::{self, Job, JobError, JobOptions},
    sources::{Cell, ConfirmJob, Fetch, Sources},
};

/// How the daemon runs, from the command line.
#[derive(Debug, Clone)]
pub struct Config {
    pub tick: Duration,
    /// Start up, run one tick, stop.
    pub once: bool,
    pub only: Only,
    /// Seconds of random pause between page loads.
    pub pace: (u64, u64),
    pub jobs: JobOptions,
    pub budget: Option<u32>,
    pub max_budget: Option<u32>,
    /// How often the requests table is polled between ticks.
    pub request_poll: Duration,
    /// Seed for the scheduler's randomness (tests); `None` seeds from
    /// the OS.
    pub seed: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(60),
            once: false,
            only: Only::default(),
            pace: (4, 15),
            jobs: JobOptions::default(),
            budget: None,
            max_budget: None,
            request_poll: Duration::from_secs(2),
            seed: None,
        }
    }
}

/// Cells older than this are not paired into trips.
const FRESH_HOURS: i64 = 48;

/// The configuration a tick works from.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub families: Vec<FamilyRecord>,
    pub routes: HashMap<RouteId, Route>,
    pub sweeps: HashSet<RouteId>,
    pub pos: HashMap<PosId, PointOfSale>,
    pub logged_in: HashSet<String>,
}

impl World {
    fn family_by_key(&self, key: &FamilyKey) -> Option<&FamilyRecord> {
        self.families.iter().find(|f| &f.key == key)
    }
}

/// What a check produced, for the bookkeeping after it.
enum Found {
    Cells(Vec<Cell>),
    Explore(Vec<ExploreDestination>),
}

pub struct Daemon<S: Sources> {
    store: Store,
    sources: S,
    config: Config,
    shutdown: watch::Receiver<bool>,
    settings: Settings,
    params: Params,
    config_version: i64,
    budget: Option<Budget>,
    rng: StdRng,
    /// The miles program being logged into while the browser is paused
    /// (0024).
    paused: Option<String>,
    /// Requests that need the browser, held while it is paused.
    deferred: VecDeque<RequestRecord>,
    /// Manual checks (0046), run before the scheduler's picks.
    manual: VecDeque<(FamilyId, Option<RequestId>)>,
    /// Manual checks still to run per request.
    manual_left: HashMap<RequestId, usize>,
    /// Places whose Freebase ID could not be learned under this
    /// configuration version.
    resolve_tried: HashSet<PlaceId>,
    state: String,
    next_check: Option<String>,
}

impl<S: Sources> Daemon<S> {
    pub fn new(
        store: Store,
        sources: S,
        config: Config,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let rng = match config.seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::from_os_rng(),
        };
        Self {
            store,
            sources,
            config,
            shutdown,
            settings: Settings::default(),
            params: Params::default(),
            config_version: -1,
            budget: None,
            rng,
            paused: None,
            deferred: VecDeque::new(),
            manual: VecDeque::new(),
            manual_left: HashMap::new(),
            resolve_tried: HashSet::new(),
            state: "starting".into(),
            next_check: None,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn sources(&self) -> &S {
        &self.sources
    }

    pub fn sources_mut(&mut self) -> &mut S {
        &mut self.sources
    }

    pub fn is_paused(&self) -> bool {
        self.paused.is_some()
    }

    fn stopping(&self) -> bool {
        *self.shutdown.borrow()
    }

    fn budget(&mut self) -> anyhow::Result<&mut Budget> {
        self.budget
            .as_mut()
            .context("the budget is not loaded (startup did not run)")
    }

    // -----------------------------------------------------------------
    // Startup and the loop
    // -----------------------------------------------------------------

    /// Opens for business: seeds an empty database, applies the budget
    /// flags, requeues requests a previous run left claimed, syncs
    /// routes and families, starts the browser and learns missing
    /// Freebase IDs.
    pub async fn startup(&mut self) -> anyhow::Result<()> {
        if self.store.seed_defaults().await? {
            info!("seeded the first-run configuration");
        }
        self.settings = budget::apply_flags(
            &self.store,
            self.config.budget,
            self.config.max_budget,
        )
        .await?;
        let requeued = self.store.requeue_claimed().await?;
        if requeued > 0 {
            info!(requeued, "requeued requests a previous run left claimed");
        }
        self.reload(true).await?;
        let now = Utc::now();
        let budget =
            Budget::restore(&self.store, &self.params, now, budget::today())
                .await?;
        budget.save(&self.store).await?;
        info!(
            daily = budget.state.daily_budget,
            used = budget.loads_today,
            cooling = ?budget.cooling(now),
            "budget restored"
        );
        self.budget = Some(budget);
        self.ensure_browser().await;
        if let Err(e) = self.resolve_places().await {
            warn!("resolving Freebase IDs failed: {e:#}");
        }
        self.state = "running".into();
        self.heartbeat().await;
        Ok(())
    }

    /// Runs until SIGINT/SIGTERM (or one tick with `once`), then closes
    /// the browser and writes a last heartbeat.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        self.startup().await?;
        if self.config.once {
            if let Err(e) = self.process_requests().await {
                warn!("requests: {e:#}");
            }
            if let Err(e) = self.run_manual().await {
                warn!("manual checks: {e:#}");
            }
            if let Err(e) = self.tick().await {
                error!("tick failed: {e:#}");
            }
            self.stop().await;
            return Ok(());
        }
        let mut next_tick = tokio::time::Instant::now();
        let mut last_beat = tokio::time::Instant::now();
        while !self.stopping() {
            if let Err(e) = self.process_requests().await {
                warn!("requests: {e:#}");
            }
            if let Err(e) = self.run_manual().await {
                warn!("manual checks: {e:#}");
            }
            if self.stopping() {
                break;
            }
            if tokio::time::Instant::now() >= next_tick {
                if let Err(e) = self.tick().await {
                    error!("tick failed: {e:#}");
                }
                next_tick = tokio::time::Instant::now() + self.config.tick;
                self.next_check = Some(format!(
                    "next tick at {}",
                    (chrono::Local::now()
                        + chrono::TimeDelta::from_std(self.config.tick)
                            .unwrap_or_default())
                    .format("%H:%M:%S")
                ));
                self.heartbeat().await;
                last_beat = tokio::time::Instant::now();
            } else if last_beat.elapsed() >= Duration::from_secs(30) {
                self.heartbeat().await;
                last_beat = tokio::time::Instant::now();
            }
            let mut shutdown = self.shutdown.clone();
            tokio::select! {
                () = tokio::time::sleep(self.config.request_poll) => {}
                _ = shutdown.changed() => {}
            }
        }
        self.stop().await;
        Ok(())
    }

    /// Closes the browser and records the daemon as stopped.
    pub async fn stop(&mut self) {
        info!("stopping");
        self.sources.stop().await;
        self.state = "stopped".into();
        self.next_check = None;
        if let Some(budget) = &self.budget
            && let Err(e) = budget.save(&self.store).await
        {
            warn!("saving the budget failed: {e:#}");
        }
        self.heartbeat().await;
    }

    async fn heartbeat(&mut self) {
        let now = Utc::now();
        let (loads_today, budget) = match &self.budget {
            Some(b) => (b.loads_today, b.daily(now, &self.params)),
            None => (0, self.settings.daily_budget),
        };
        let state = match &self.paused {
            Some(program) if self.state != "stopped" => {
                format!("paused for login: {program}")
            }
            _ => self.state.clone(),
        };
        let beat = Heartbeat {
            pid: std::process::id(),
            updated_at: now,
            state,
            next_check: self.next_check.clone(),
            loads_today,
            budget,
        };
        if let Err(e) = self.store.write_heartbeat(&beat).await {
            warn!("heartbeat failed: {e}");
        }
    }

    /// Re-reads the settings and re-syncs routes and families when the
    /// configuration changed (0035), or when `force`d.
    pub async fn reload(&mut self, force: bool) -> anyhow::Result<bool> {
        let version = self.store.config_version().await?;
        if !force && version == self.config_version {
            return Ok(false);
        }
        self.settings = self.store.settings().await?;
        self.params = Params::from_settings(&self.settings);
        let routes = self.store.sync_routes().await?;
        let sync = self.store.sync_families(&self.settings).await?;
        self.config_version = self.store.config_version().await?;
        self.resolve_tried.clear();
        info!(
            routes,
            families = sync.active,
            created = sync.created,
            version = self.config_version,
            "configuration loaded"
        );
        Ok(true)
    }

    /// Starts the browser unless it runs or is paused for a login.
    async fn ensure_browser(&mut self) {
        if self.paused.is_some() || self.sources.is_running() {
            return;
        }
        let pos = match self.store.list_points_of_sale(true).await {
            Ok(pos) => pos,
            Err(e) => {
                warn!("cannot list points of sale: {e}");
                return;
            }
        };
        match self.sources.start(&pos).await {
            Ok(()) => {
                if self.state == "browser unavailable" {
                    self.state = "running".into();
                }
            }
            Err(e) => {
                error!("the browser did not start: {e:#}");
                self.state = "browser unavailable".into();
            }
        }
    }

    /// Everything a tick needs from the configuration.
    pub async fn world(&self) -> anyhow::Result<World> {
        let families = self.store.list_families(true).await?;
        let mut routes: HashMap<RouteId, Route> = self
            .store
            .list_routes(false)
            .await?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let mut sweeps = HashSet::new();
        for r in self.store.sweep_routes(false).await? {
            sweeps.insert(r.id);
            routes.insert(r.id, r);
        }
        let pos = self
            .store
            .list_points_of_sale(false)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
        let logged_in = self
            .store
            .list_miles_programs()
            .await?
            .into_iter()
            .filter(|m| m.active && m.login_state == LoginState::LoggedIn)
            .map(|m| m.program)
            .collect();
        Ok(World {
            families,
            routes,
            sweeps,
            pos,
            logged_in,
        })
    }

    /// The candidate filters for `world` at `now`.
    pub fn context(&self, world: &World, now: DateTime<Utc>) -> Context {
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
            has_jev: self.sources.has_jev(),
            logged_in: world.logged_in.clone(),
            unresolved,
            cooling: self
                .budget
                .as_ref()
                .map(|b| b.cooling(now))
                .unwrap_or_default(),
            destinations,
            paused: self.paused.is_some(),
            only: self.config.only.clone(),
            home_pos: world.pos.values().find(|p| p.is_home).map(|p| p.id),
            // No airline or miles site has been probed yet (0050).
            verified_sites: HashSet::new(),
        }
    }

    /// One scheduler tick (0028).
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        let now = Utc::now();
        let today = budget::today();
        self.reload(false).await?;
        let params = self.params.clone();
        if self.budget()?.advance(now, today, &params) {
            let budget = self.budget()?.clone();
            budget.save(&self.store).await?;
        }
        if self.paused.is_some() {
            debug!("paused for a login: no checks");
            return Ok(());
        }
        self.ensure_browser().await;
        if !self.sources.is_running() {
            return Ok(());
        }
        self.state = "running".into();
        if let Err(e) = self.resolve_places().await {
            warn!("resolving Freebase IDs failed: {e:#}");
        }
        let world = self.world().await?;
        let cx = self.context(&world, now);
        let (candidates, skipped) = candidates::build(&world.families, &cx);
        let ticks_left = if self.config.once {
            1
        } else {
            budget::ticks_left_today(self.config.tick)
        };
        let mut rng = StdRng::from_rng(&mut self.rng);
        let tick_budget =
            self.budget()?.tick(now, ticks_left, &params, &mut rng);
        let picks =
            plan_tick(&candidates, now, &tick_budget, &params, &mut rng);
        info!(
            candidates = candidates.len(),
            ?skipped,
            loads_available = tick_budget.loads_available,
            picks = picks.len(),
            "tick planned"
        );
        for (i, pick) in picks.iter().enumerate() {
            if self.stopping() {
                break;
            }
            if self.login_requested().await {
                info!("a login is waiting: ending the tick early");
                break;
            }
            if self
                .budget
                .as_ref()
                .is_some_and(|b| b.cooling(Utc::now()).contains(&pick.site))
            {
                continue;
            }
            if i > 0 {
                self.pace().await;
            }
            if let Err(e) = self
                .run_check(pick.family, pick.reason.clone(), &world)
                .await
            {
                warn!(family = %pick.family, "check failed: {e:#}");
            }
        }
        Ok(())
    }

    /// A random pause between page loads, cut short by a shutdown.
    async fn pace(&mut self) {
        let (min, max) = self.config.pace;
        if max == 0 {
            return;
        }
        let secs = self.rng.random_range(min..=max);
        let mut shutdown = self.shutdown.clone();
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(secs)) => {}
            _ = shutdown.changed() => {}
        }
    }

    async fn login_requested(&self) -> bool {
        self.store.pending_requests().await.is_ok_and(|rs| {
            rs.iter()
                .any(|r| matches!(r.kind, RequestKind::OpenLogin { .. }))
        })
    }

    // -----------------------------------------------------------------
    // Requests (0046)
    // -----------------------------------------------------------------

    /// Takes every pending request.
    pub async fn process_requests(&mut self) -> anyhow::Result<()> {
        while let Some(request) = self.store.claim_next().await? {
            info!(id = %request.id, kind = request.kind.name(), "request");
            self.handle_request(request).await?;
        }
        if self.paused.is_none() && !self.deferred.is_empty() {
            let deferred: Vec<RequestRecord> =
                self.deferred.drain(..).collect();
            for request in deferred {
                self.handle_request(request).await?;
            }
        }
        Ok(())
    }

    async fn handle_request(
        &mut self,
        request: RequestRecord,
    ) -> anyhow::Result<()> {
        let id = request.id;
        match &request.kind {
            RequestKind::OpenLogin { program } => {
                info!(%program, "pausing the browser for a login");
                self.sources.stop().await;
                self.paused = Some(program.clone());
                self.heartbeat().await;
                self.store
                    .complete(id, true, Some("the daemon's browser is closed"))
                    .await?;
            }
            RequestKind::CloseLogin => {
                if let Some(program) = self.paused.take() {
                    info!(%program, "login window closed: resuming");
                    self.store
                        .set_miles_login(
                            &program,
                            LoginState::LoggedIn,
                            Utc::now(),
                        )
                        .await?;
                }
                self.ensure_browser().await;
                let ok = self.sources.is_running();
                self.store
                    .complete(
                        id,
                        ok,
                        Some(if ok {
                            "resumed"
                        } else {
                            "the browser did not start"
                        }),
                    )
                    .await?;
                self.heartbeat().await;
            }
            RequestKind::ReloadConfig => {
                self.reload(true).await?;
                self.store.complete(id, true, None).await?;
            }
            RequestKind::CheckRoute { .. }
            | RequestKind::CheckFamily { .. }
                if self.paused.is_some() =>
            {
                self.deferred.push_back(request);
            }
            RequestKind::ConfirmDeal { .. } if self.paused.is_some() => {
                self.deferred.push_back(request);
            }
            RequestKind::CheckRoute { route } => {
                let world = self.world().await?;
                let cx = self.context(&world, Utc::now());
                let ids = candidates::of_route(&world.families, *route, &cx);
                self.queue_manual(id, ids).await?;
            }
            RequestKind::CheckFamily { family } => {
                let world = self.world().await?;
                let cx = self.context(&world, Utc::now());
                let ids: Vec<FamilyId> = world
                    .families
                    .iter()
                    .filter(|f| f.id == *family)
                    .filter(|f| {
                        // Manual checks ignore the debugging restriction.
                        let mut cx = cx.clone();
                        cx.only = Only::default();
                        candidates::eligibility(f, &cx).is_ok()
                    })
                    .map(|f| f.id)
                    .collect();
                self.queue_manual(id, ids).await?;
            }
            RequestKind::ConfirmDeal { deal } => match self
                .store
                .deal(*deal)
                .await?
            {
                Some(d)
                    if d.status == DealStatus::Candidate
                        || d.status == DealStatus::Confirmed =>
                {
                    self.ensure_browser().await;
                    let ok = self.confirm(*deal, &d, None).await.is_ok();
                    self.store.complete(id, ok, None).await?;
                }
                Some(_) => {
                    self.store
                        .complete(id, false, Some("the deal is no longer open"))
                        .await?;
                }
                None => {
                    self.store
                        .complete(id, false, Some("no such deal"))
                        .await?;
                }
            },
        }
        Ok(())
    }

    async fn queue_manual(
        &mut self,
        request: RequestId,
        ids: Vec<FamilyId>,
    ) -> anyhow::Result<()> {
        if ids.is_empty() {
            self.store
                .complete(request, false, Some("nothing can be checked now"))
                .await?;
            return Ok(());
        }
        self.manual_left.insert(request, ids.len());
        self.manual
            .extend(ids.into_iter().map(|f| (f, Some(request))));
        Ok(())
    }

    /// Runs the queued manual checks, first in line (0046).
    pub async fn run_manual(&mut self) -> anyhow::Result<()> {
        if self.manual.is_empty() || self.paused.is_some() {
            return Ok(());
        }
        self.ensure_browser().await;
        if !self.sources.is_running() {
            return Ok(());
        }
        let world = self.world().await?;
        let mut first = true;
        while let Some((family, request)) = self.manual.pop_front() {
            if self.stopping() || self.paused.is_some() {
                self.manual.push_front((family, request));
                break;
            }
            if !first {
                self.pace().await;
            }
            first = false;
            let result =
                self.run_check(family, QueryReason::Manual, &world).await;
            if let Err(e) = &result {
                warn!(%family, "manual check failed: {e:#}");
            }
            if let Some(request) = request
                && let Some(left) = self.manual_left.get_mut(&request)
            {
                *left -= 1;
                if *left == 0 {
                    self.manual_left.remove(&request);
                    self.store.complete(request, true, None).await?;
                }
            }
            if self.login_requested().await {
                break;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Freebase IDs (0041)
    // -----------------------------------------------------------------

    /// Learns the Freebase ID of every city on a selected route that
    /// lacks one, once per configuration version.
    async fn resolve_places(&mut self) -> anyhow::Result<()> {
        if !self.sources.is_running() || self.paused.is_some() {
            return Ok(());
        }
        let routes = self.store.list_routes(true).await?;
        let only = &self.config.only;
        let mut places = Vec::new();
        for route in &routes {
            if let Some(code) = &only.destination
                && !route.destination.code.eq_ignore_ascii_case(code)
            {
                continue;
            }
            if only.family.is_some() {
                let Some(id) = only.family else { continue };
                match self.store.family(id).await? {
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
        let Some(home) = self.store.home_point_of_sale().await? else {
            return Ok(());
        };
        for place in places {
            let now = Utc::now();
            let params = self.params.clone();
            let budget = self.budget()?;
            if budget.cooling(now).contains("google_flights")
                || budget.loads_today >= budget.daily(now, &params)
            {
                break;
            }
            self.resolve_tried.insert(place.id);
            let job = jobs::resolve_job(
                &place,
                &self.settings,
                &home,
                &self.config.jobs,
            );
            info!(code = %place.code, url = %job.url, "learning a Freebase ID");
            let fetch = self.sources.resolve(&job).await;
            self.spend(fetch.page_loads).await?;
            match (&fetch.outcome, &fetch.value) {
                (QueryOutcome::Ok, Some(id)) => {
                    info!(code = %place.code, %id, "Freebase ID learned");
                    self.store.set_freebase_id(place.id, id).await?;
                }
                (QueryOutcome::Blocked { detail }, _) => {
                    self.block("google_flights", detail).await?;
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

    async fn spend(&mut self, loads: u32) -> anyhow::Result<()> {
        let store = self.store.clone();
        let today = budget::today();
        self.budget()?.spend(&store, loads, Utc::now(), today).await
    }

    async fn block(&mut self, site: &str, detail: &str) -> anyhow::Result<()> {
        warn!(%site, %detail, "blocked: backing off");
        let store = self.store.clone();
        let params = self.params.clone();
        self.budget()?
            .on_block(&store, site, detail, Utc::now(), &params)
            .await
    }

    /// The departure an airline or miles search uses: the cheapest one
    /// Google Flights showed lately for the same route, cabin and leg
    /// under the home point of sale.
    async fn agent_departure(
        &self,
        family: &FamilyRecord,
        world: &World,
    ) -> Option<NaiveDate> {
        let home = world.pos.values().find(|p| p.is_home)?;
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
        let id = world.family_by_key(&key)?.id;
        let tomorrow = budget::today() + Days::new(1);
        self.store
            .latest_cells(id)
            .await
            .ok()?
            .into_iter()
            .filter(|c| c.departure >= tomorrow)
            .filter_map(|c| c.price.map(|p| (p.minor, c.departure)))
            .min()
            .map(|(_, d)| d)
    }

    /// Runs one check of `family` and records everything it saw.
    pub async fn run_check(
        &mut self,
        family_id: FamilyId,
        reason: QueryReason,
        world: &World,
    ) -> anyhow::Result<()> {
        let Some(family) = self.store.family(family_id).await? else {
            anyhow::bail!("no family {family_id}");
        };
        let route = world
            .routes
            .get(&family.key.route)
            .cloned()
            .context("the family's route is not configured")?;
        let pos = world
            .pos
            .get(&family.key.pos)
            .cloned()
            .context("the family's point of sale is not configured")?;
        let today = budget::today();
        let options = JobOptions {
            agent_departure: match family.key.source {
                Source::GoogleFlights => None,
                _ => self.agent_departure(&family, world).await,
            },
            ..self.config.jobs.clone()
        };
        let job = match jobs::build(
            &family.key,
            &route,
            &pos,
            &self.settings,
            today,
            &options,
        ) {
            Ok(job) => job,
            Err(e @ JobError::Unresolved(_)) => {
                debug!(family = %family.id, "{e}");
                return Ok(());
            }
            Err(e) => {
                warn!(family = %family.id, "cannot check: {e}");
                self.mark_checked(&family, Utc::now()).await?;
                return Ok(());
            }
        };
        let started = Utc::now();
        let query = self.store.start_query(family.id, &reason, started).await?;
        info!(
            family = %family.id,
            route = %format!("{}-{}", route.origin.code, route.destination.code),
            cabin = ?family.key.cabin,
            site = %family.site,
            pos = %pos.country,
            kind = ?family.key.kind,
            reason = reason.label(),
            "checking"
        );
        let (fetch, found) = match &job {
            Job::Calendar(j) => {
                split(self.sources.calendar(j).await, Found::Cells)
            }
            Job::Agent(j) => {
                split(self.sources.agent_search(j).await, Found::Cells)
            }
            Job::Explore(j) => {
                split(self.sources.explore(j).await, Found::Explore)
            }
        };
        let finished = Utc::now();
        self.store
            .finish_query(
                query,
                &fetch.outcome,
                fetch.page_loads,
                fetch.jev_calls,
                finished,
            )
            .await?;
        self.spend(fetch.page_loads).await?;
        info!(
            family = %family.id,
            outcome = ?fetch.outcome,
            loads = fetch.page_loads,
            jev = fetch.jev_calls,
            "checked"
        );
        match &fetch.outcome {
            QueryOutcome::Ok => {}
            QueryOutcome::Blocked { detail } => {
                self.block(&family.site, detail).await?;
                self.mark_checked(&family, finished).await?;
                return Ok(());
            }
            QueryOutcome::LoggedOut => {
                if let Source::Miles { program } = &family.key.source {
                    warn!(%program, "logged out: the app will ask for a login");
                    self.store
                        .set_miles_login(program, LoginState::Expired, finished)
                        .await?;
                }
                self.mark_checked(&family, finished).await?;
                return Ok(());
            }
            QueryOutcome::Unexpected { .. } | QueryOutcome::Error { .. } => {
                self.mark_checked(&family, finished).await?;
                return Ok(());
            }
        }
        match found {
            Found::Explore(destinations) => {
                self.after_explore(
                    &family,
                    &route,
                    &pos,
                    query,
                    destinations,
                    world,
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
                    world,
                    finished,
                    fetch.page_loads,
                    reason,
                )
                .await
            }
        }
    }

    /// A check that taught nothing about prices still counts as a visit,
    /// so a broken family is not retried every tick.
    async fn mark_checked(
        &self,
        family: &FamilyRecord,
        at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let mut state = family.state.clone();
        state.last_check = Some(at);
        self.store.save_family_state(family.id, &state).await?;
        Ok(())
    }

    /// Folds a successful check into the family's state; returns whether
    /// its window minimum dropped materially.
    async fn learn(
        &self,
        family: &FamilyRecord,
        min_price_minor: Option<i64>,
        page_loads: u32,
        reason: QueryReason,
        at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let mut state = to_state(&family.state);
        let moved = min_moved(
            state.last_min_price_minor,
            min_price_minor,
            self.params.material_change,
        );
        let summary = CheckSummary {
            min_price_minor,
            any_material_change: false,
            drop: false,
            page_loads,
            reason,
        };
        update_after_check_with(&mut state, &summary, at, &self.params);
        self.store
            .save_family_state(family.id, &to_record(&state, &family.state))
            .await?;
        Ok(moved.dropped)
    }

    /// Adds one pending hint to the families related to one that
    /// dropped (0028: probe, then fan out).
    async fn fan_out(
        &self,
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
            if let Some(f) = self.store.family(id).await? {
                let mut state = to_state(&f.state);
                fanout::boost(&mut state);
                self.store
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
            })
            .collect();
        if !observations.is_empty() {
            self.store.insert_observations(&observations).await?;
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
            self.store.insert_explore_hints(&hints).await?;
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
        let hinted = hinted_families(&hints, &targets, &self.params);
        info!(
            destinations = destinations.len(),
            hints = hints.len(),
            hinted = hinted.len(),
            "Explore recorded"
        );
        for id in hinted {
            if let Some(f) = self.store.family(id).await? {
                let mut state = to_state(&f.state);
                add_hint(&mut state);
                self.store
                    .save_family_state(id, &to_record(&state, &f.state))
                    .await?;
            }
        }
        let min = hints.iter().map(|h| h.price.minor).min();
        self.learn(family, min, page_loads, reason, at).await?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Deals (0030, 0031, 0042, 0043)
    // -----------------------------------------------------------------

    /// Re-prices the trips of one route × cabin × point of sale after a
    /// check of one of its Google Flights families, learns the
    /// round-trip discount, expires confirmed deals that are gone, and
    /// confirms new candidates.
    pub async fn evaluate_deals(
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
        let days = self.settings.trips.round_trip_days;
        let (Some(outbound), Some(ret)) = (
            self.store
                .family_id(&key(FamilyKind::Leg { leg: Leg::Outbound }))
                .await?,
            self.store
                .family_id(&key(FamilyKind::Leg { leg: Leg::Return }))
                .await?,
        ) else {
            return Ok(());
        };
        let round_trip = self
            .store
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
        let out_cells = fresh(self.store.latest_cells(outbound).await?);
        let ret_cells = fresh(self.store.latest_cells(ret).await?);
        let rt_cells = match round_trip {
            Some(id) => fresh(self.store.latest_cells(id).await?),
            None => Vec::new(),
        };
        let out = deals::leg_prices(&out_cells);
        let back = deals::leg_prices(&ret_cells);
        let rt = deals::round_trip_prices(&rt_cells);
        let mut discount = to_discount(
            &self
                .store
                .route_trip_state(route.id)
                .await?
                .unwrap_or_default(),
        );
        if matches!(touched, FamilyKind::RoundTrip { .. }) && !rt.is_empty() {
            let pairs = compare_round_trips(&rt, &out, &back);
            if !pairs.is_empty() {
                let was = discount.additive;
                discount.observe_pairs(&pairs, &self.params);
                self.store
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
        let range = self.settings.trips;
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
        let history = self
            .store
            .history_for_route(
                route.id,
                cabin,
                now - self.params.baseline_window,
            )
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
        let last = jobs::horizon_end(today, &self.settings);
        let found =
            deals::find(&trips, &points, now, &self.params, first, last);
        let known = self
            .store
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
                self.store
                    .set_deal_status(id, DealStatus::Expired, now)
                    .await?;
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
                &self.settings,
            )
            .map(|s| {
                jobs::rebase(
                    &search_url(&s, &jobs::locale(&self.settings, pos)),
                    None,
                )
            });
            let deal = deals::candidate(
                &f,
                route.id,
                cabin,
                pos,
                now,
                &self.params,
                url,
            );
            let id = self.store.insert_deal(&deal).await?;
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
    pub async fn confirm(
        &mut self,
        id: DealId,
        deal: &Deal,
        legs_sum: Option<i64>,
    ) -> anyhow::Result<()> {
        if self
            .budget
            .as_ref()
            .is_some_and(|b| b.cooling(Utc::now()).contains("google_flights"))
            || !self.sources.is_running()
        {
            info!(deal = %id, "cannot confirm now: left as a candidate");
            return Ok(());
        }
        let route = self.store.route(deal.route).await?.context("no route")?;
        let pos = self
            .store
            .point_of_sale(deal.pos)
            .await?
            .context("no point of sale")?;
        let Some(search) = route_search(
            &route,
            deal.cabin,
            deal.departure,
            deal.return_date,
            &self.settings,
        ) else {
            return Ok(());
        };
        let days = self.settings.trips.round_trip_days;
        let key = |kind| FamilyKey {
            route: route.id,
            cabin: deal.cabin,
            source: Source::GoogleFlights,
            pos: pos.id,
            kind,
        };
        let rt_family = self
            .store
            .family_id(&key(FamilyKind::RoundTrip { days }))
            .await?;
        let log_family = match rt_family {
            Some(id) => id,
            None => self
                .store
                .family_id(&key(FamilyKind::Leg { leg: Leg::Outbound }))
                .await?
                .context("no family to log the confirmation on")?,
        };
        let now = Utc::now();
        let query = self
            .store
            .start_query(log_family, &QueryReason::Confirmation, now)
            .await?;
        let job = ConfirmJob {
            pos: pos.id,
            search,
            locale: jobs::locale(&self.settings, &pos),
            origin_override: self.config.jobs.origin_override.clone(),
        };
        let fetch = self.sources.confirm(&job).await;
        let finished = Utc::now();
        self.store
            .finish_query(
                query,
                &fetch.outcome,
                fetch.page_loads,
                fetch.jev_calls,
                finished,
            )
            .await?;
        self.spend(fetch.page_loads).await?;
        match &fetch.outcome {
            QueryOutcome::Ok => {}
            QueryOutcome::Blocked { detail } => {
                self.block("google_flights", detail).await?;
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
            self.store
                .latest_exchange_rates()
                .await?
                .into_iter()
                .map(|r| (r.currency.0, r.brl_per_unit)),
        );
        let Some(verdict) = deals::judge(
            deal.price.minor,
            &confirmation,
            &pos,
            &fx,
            &self.params,
        ) else {
            info!(deal = %id, "no price on the results page: rejected");
            self.store
                .set_deal_status(id, DealStatus::Rejected, finished)
                .await?;
            return Ok(());
        };
        for (currency, rate) in &verdict.rates {
            self.store
                .set_exchange_rate(currency, *rate, finished)
                .await?;
        }
        self.store
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
            self.store.attach_seller_quotes(id, &verdict.quotes).await?;
        }
        let status = if verdict.held {
            DealStatus::Confirmed
        } else {
            DealStatus::Rejected
        };
        self.store.set_deal_status(id, status, finished).await?;
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
                &self
                    .store
                    .route_trip_state(route.id)
                    .await?
                    .unwrap_or_default(),
            );
            discount.observe(verdict.price.minor, legs, &self.params);
            self.store
                .save_route_trip_state(route.id, &from_discount(&discount))
                .await?;
        }
        let length = (deal.return_date - deal.departure).num_days();
        if rt_family == Some(log_family) && length == i64::from(days) {
            self.store
                .insert_observations(&[Observation {
                    id: None,
                    query,
                    family: log_family,
                    observed_at: finished,
                    departure: deal.departure,
                    return_date: Some(deal.return_date),
                    price: Some(Money::brl_whole(verdict.price.whole())),
                    miles: None,
                    carrier: confirmation.carrier.clone(),
                    stops: None,
                    duration_minutes: None,
                    seller: verdict.seller.clone(),
                }])
                .await?;
        }
        Ok(())
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

/// A fetch's bookkeeping, and its value as a [`Found`].
fn split<T>(f: Fetch<T>, found: impl FnOnce(T) -> Found) -> (Fetch<()>, Found) {
    let Fetch {
        outcome,
        value,
        page_loads,
        jev_calls,
    } = f;
    (
        Fetch {
            outcome,
            value: (),
            page_loads,
            jev_calls,
        },
        found(value),
    )
}
