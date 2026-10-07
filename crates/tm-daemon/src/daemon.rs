//! The coordinator (DECISIONS.md 0028, 0035, 0046, 0060, 0062): reload
//! the configuration, take the app's requests, and keep one worker per
//! site ([`crate::worker`]) running that site's checks.
//!
//! Each site (`Source::site()`: Google Flights, TAP, each miles program)
//! has its own worker task with its own Chromium profile and process
//! (LATAM's cash site shares LATAM Pass's, [`crate::sites::worker_site`],
//! 0084),
//! its own budget, its own pacing and its own scheduler tick, so
//! different sites check in parallel, and each runs up to
//! [`Config::parallel`] checks at once (0072). A worker starts when the site has eligible families (or a
//! login request names it) and lives until the daemon stops; its browser
//! starts when the site has work and closes when the site is idle.
//!
//! Requests are routed by site: a route's manual check goes to every
//! site the route has families on, a deal confirmation to Google
//! Flights, a "Check award" to the miles programs' workers (0085), and
//! a miles login (`OpenLogin`/`CloseLogin`) to that program's site
//! alone, which pauses while every other site keeps running.
//!
//! The coordinator writes the global heartbeat (process alive, totals);
//! each worker writes its own row of `site_status` (what it checks now)
//! and of `site_budgets`.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use chrono::Utc;
use rand::{Rng, SeedableRng, rngs::StdRng};
use tm_domain::{
    DealStatus,
    FamilyId,
    FamilyKey,
    FamilyKind,
    GOOGLE_FLIGHTS,
    Leg,
    QueryReason,
    RequestId,
    SiteLimits,
    Source,
    has_award_reader,
    program_name,
};
use tm_scheduler::{Params, awards::AwardKey};
use tm_store::{
    Heartbeat,
    LoginState,
    RequestKind,
    RequestRecord,
    SiteStatus,
    Store,
};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{error, info, warn};

use crate::{
    budget::{self, Budget},
    candidates::{self, Context, Only},
    jobs::JobOptions,
    notify::Notifier,
    sites::worker_site,
    sources::Sources,
    worker::{self, Command, Loaded, Shared, Tracker, Worker, World},
};

/// How the daemon runs, from the command line.
#[derive(Debug, Clone)]
pub struct Config {
    pub tick: Duration,
    /// Start up, run one tick, stop.
    pub once: bool,
    pub only: Only,
    /// Seconds of random pause between one site's page loads.
    pub pace: (u64, u64),
    /// How many checks each site runs at once (0072).
    pub parallel: usize,
    pub jobs: JobOptions,
    /// Google Flights' starting daily budget (`--budget`).
    pub budget: Option<u32>,
    /// Google Flights' maximum (`--max-budget`).
    pub max_budget: Option<u32>,
    /// Other sites' limits (`--site-budget airline:TP=300:1000`), 0060.
    pub site_budgets: Vec<(String, SiteLimits)>,
    /// How often the requests table is polled between ticks.
    pub request_poll: Duration,
    /// Seed for the scheduler's randomness (tests); `None` seeds from
    /// the OS.
    pub seed: Option<u64>,
    /// The base of the sites' Chromium profiles: each site's is
    /// `<profile_dir>/<site slug>` (0062), whose cookie jar says whether
    /// a miles login took (0064). `None` records no login state.
    pub profile_dir: Option<PathBuf>,
    /// A site's browser closes after this long without work (0062).
    pub idle_close: Duration,
    /// Where newly confirmed deals are announced (0080); `None` sends
    /// nothing and leaves them unseen.
    pub notifier: Option<Arc<dyn Notifier>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(5),
            once: false,
            only: Only::default(),
            pace: (4, 10),
            parallel: 3,
            jobs: JobOptions::default(),
            budget: None,
            max_budget: None,
            site_budgets: Vec::new(),
            request_poll: Duration::from_secs(2),
            seed: None,
            profile_dir: None,
            idle_close: Duration::from_secs(5 * 60),
            notifier: None,
        }
    }
}

/// Makes a site's [`Sources`]: the real daemon gives each site its own
/// Chromium profile ([`crate::sources::browser::site_config`]).
pub type SourceFactory<S> = Arc<dyn Fn(&str) -> S + Send + Sync>;

/// A worker task and the way to talk to it.
struct Handle {
    tx: mpsc::UnboundedSender<Command>,
    join: tokio::task::JoinHandle<()>,
}

pub struct Daemon<S: Sources> {
    store: Store,
    config: Config,
    shutdown: watch::Receiver<bool>,
    factory: SourceFactory<S>,
    loaded: Arc<Loaded>,
    publish: watch::Sender<Arc<Loaded>>,
    workers: BTreeMap<String, Handle>,
    /// Sites paused for a login, and the program (0024, 0062).
    paused: BTreeMap<String, String>,
    tracker: Tracker,
    /// Workers tick on their own timers (the daemon's loop); otherwise
    /// they act only when told (tests and `--once`).
    autonomous: bool,
    rng: StdRng,
    state: String,
}

impl<S: Sources + 'static> Daemon<S> {
    /// A daemon over `store`; `sources` makes each site's sources.
    pub fn new(
        store: Store,
        sources: impl Fn(&str) -> S + Send + Sync + 'static,
        config: Config,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let rng = match config.seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::from_os_rng(),
        };
        let loaded = Arc::new(Loaded::default());
        let (publish, _) = watch::channel(Arc::clone(&loaded));
        Self {
            store,
            config,
            shutdown,
            factory: Arc::new(sources),
            loaded,
            publish,
            workers: BTreeMap::new(),
            paused: BTreeMap::new(),
            tracker: Tracker::default(),
            autonomous: false,
            rng,
            state: "starting".into(),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The sites that have a worker.
    pub fn sites(&self) -> Vec<String> {
        self.workers.keys().cloned().collect()
    }

    /// Whether `site` is paused for a login.
    pub fn is_paused(&self, site: &str) -> bool {
        self.paused.contains_key(site)
    }

    /// The sites paused for a login.
    pub fn paused_sites(&self) -> Vec<String> {
        self.paused.keys().cloned().collect()
    }

    fn stopping(&self) -> bool {
        *self.shutdown.borrow()
    }

    // -----------------------------------------------------------------
    // Startup and the loop
    // -----------------------------------------------------------------

    /// Opens for business: seeds an empty database, applies the budget
    /// flags to the sites' rows, requeues requests a previous run left
    /// claimed, syncs routes and families, starts a worker per site with
    /// work, and has Google Flights learn missing Freebase IDs.
    pub async fn startup(&mut self) -> anyhow::Result<()> {
        if self.store.seed_defaults().await? {
            info!("seeded the first-run configuration");
        }
        let settings = budget::apply_flags(
            &self.store,
            self.config.budget,
            self.config.max_budget,
        )
        .await?;
        budget::configure_sites(
            &self.store,
            &settings,
            &self.config.site_budgets,
            Utc::now(),
            budget::today(),
        )
        .await?;
        for b in self.store.site_budgets().await? {
            info!(
                site = %b.site,
                daily = b.daily_budget,
                max = b.max_daily_budget,
                used = b.loads_on(budget::today()),
                "budget"
            );
        }
        let requeued = self.store.requeue_claimed().await?;
        if requeued > 0 {
            info!(requeued, "requeued requests a previous run left claimed");
        }
        self.store.clear_site_statuses().await?;
        self.reload(true).await?;
        self.ensure_workers().await?;
        if !self.autonomous && self.workers.contains_key(GOOGLE_FLIGHTS) {
            self.ask(GOOGLE_FLIGHTS, Command::Resolve).await;
        }
        self.state = "running".into();
        self.heartbeat().await;
        Ok(())
    }

    /// Runs until SIGINT/SIGTERM (or one tick with `once`), then stops
    /// every worker and writes a last heartbeat.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        self.autonomous = !self.config.once;
        self.startup().await?;
        if self.config.once {
            if let Err(e) = self.process_requests().await {
                warn!("requests: {e:#}");
            }
            self.run_manual().await;
            if let Err(e) = self.tick().await {
                error!("tick failed: {e:#}");
            }
            self.stop().await;
            return Ok(());
        }
        let mut next_tick = tokio::time::Instant::now() + self.config.tick;
        let mut last_beat = tokio::time::Instant::now();
        while !self.stopping() {
            if let Err(e) = self.process_requests().await {
                warn!("requests: {e:#}");
            }
            self.reap().await;
            if tokio::time::Instant::now() >= next_tick {
                if let Err(e) = self.reload(false).await {
                    warn!("reloading the configuration failed: {e:#}");
                }
                if let Err(e) = self.ensure_workers().await {
                    warn!("starting site workers failed: {e:#}");
                }
                next_tick = tokio::time::Instant::now() + self.config.tick;
            }
            if last_beat.elapsed() >= Duration::from_secs(30) {
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

    /// Stops every worker (each closes its browser and saves its budget)
    /// and records the daemon as stopped.
    pub async fn stop(&mut self) {
        info!(sites = self.workers.len(), "stopping");
        let workers = std::mem::take(&mut self.workers);
        let mut joins = Vec::new();
        for (site, handle) in workers {
            // Dropping the sender ends the worker's command loop.
            drop(handle.tx);
            joins.push((site, handle.join));
        }
        for (site, join) in joins {
            if let Err(e) = join.await {
                error!(%site, "the site worker failed: {e}");
                self.write_stopped(&site).await;
            }
        }
        self.state = "stopped".into();
        self.heartbeat().await;
    }

    /// Notices workers that ended on their own (a panic): their site is
    /// marked stopped and gets a new worker on the next tick.
    async fn reap(&mut self) {
        let done: Vec<String> = self
            .workers
            .iter()
            .filter(|(_, h)| h.join.is_finished())
            .map(|(site, _)| site.clone())
            .collect();
        for site in done {
            if let Some(handle) = self.workers.remove(&site) {
                if let Err(e) = handle.join.await {
                    error!(%site, "the site worker failed: {e}");
                }
                self.write_stopped(&site).await;
            }
        }
    }

    async fn write_stopped(&self, site: &str) {
        let status = SiteStatus {
            site: site.to_owned(),
            updated_at: Utc::now(),
            state: "stopped".into(),
            next_check_at: None,
            checks: Vec::new(),
        };
        if let Err(e) = self.store.write_site_status(&status).await {
            warn!(%site, "status failed: {e}");
        }
    }

    /// The global heartbeat: the process is alive, with today's loads and
    /// budgets summed over the sites (each site's own are in
    /// `site_budgets`).
    async fn heartbeat(&self) {
        let today = budget::today();
        let (loads_today, budget) = match self.store.site_budgets().await {
            Ok(rows) => rows.iter().fold((0u32, 0u32), |(l, b), r| {
                (
                    l.saturating_add(r.loads_on(today)),
                    b.saturating_add(r.daily_budget),
                )
            }),
            Err(e) => {
                warn!("reading the budgets failed: {e}");
                (0, 0)
            }
        };
        let beat = Heartbeat {
            pid: std::process::id(),
            updated_at: Utc::now(),
            state: self.state.clone(),
            next_check: None,
            next_check_at: None,
            loads_today,
            budget,
        };
        if let Err(e) = self.store.write_heartbeat(&beat).await {
            warn!("heartbeat failed: {e}");
        }
    }

    /// Re-reads the settings and re-syncs routes and families when the
    /// configuration changed (0035), or when `force`d, and hands the new
    /// configuration to the workers.
    pub async fn reload(&mut self, force: bool) -> anyhow::Result<bool> {
        let version = self.store.config_version().await?;
        if !force && version == self.loaded.version {
            return Ok(false);
        }
        let settings = self.store.settings().await?;
        let params = Params::from_settings(&settings);
        let routes = self.store.sync_routes().await?;
        let sync = self.store.sync_families(&settings).await?;
        let version = self.store.config_version().await?;
        self.loaded = Arc::new(Loaded {
            settings,
            params,
            version,
        });
        self.publish.send_replace(Arc::clone(&self.loaded));
        info!(
            routes,
            families = sync.active,
            created = sync.created,
            version,
            "configuration loaded"
        );
        Ok(true)
    }

    // -----------------------------------------------------------------
    // Workers
    // -----------------------------------------------------------------

    /// Starts a worker for every site with eligible families.
    async fn ensure_workers(&mut self) -> anyhow::Result<()> {
        let world = self.world().await?;
        let cx = worker::context(
            &world,
            &self.loaded.settings,
            &self.config.only,
            HashSet::new(),
            HashSet::new(),
        );
        let sites: BTreeSet<String> = world
            .families
            .iter()
            .filter(|f| candidates::eligibility(f, &cx).is_ok())
            .map(|f| worker_site(&f.site).to_owned())
            .collect();
        for site in sites {
            self.worker(&site).await?;
        }
        Ok(())
    }

    /// `site`'s worker, started if it has none.
    async fn worker(
        &mut self,
        site: &str,
    ) -> anyhow::Result<&mpsc::UnboundedSender<Command>> {
        if !self.workers.contains_key(site) {
            let budget = Budget::restore(
                &self.store,
                site,
                &self.loaded.params,
                Utc::now(),
                budget::today(),
            )
            .await?;
            let shared = Shared {
                store: self.store.clone(),
                config: self.config.clone(),
                loaded: self.publish.subscribe(),
                shutdown: self.shutdown.clone(),
                tracker: self.tracker.clone(),
            };
            let seed = self.rng.random();
            let (tx, join) = Worker::spawn(
                site.to_owned(),
                shared,
                (self.factory)(site),
                budget,
                seed,
                self.autonomous,
            );
            if let Some(program) = self.paused.get(site) {
                let _ = tx.send(Command::Pause {
                    program: program.clone(),
                    request: None,
                });
            }
            self.workers.insert(site.to_owned(), Handle { tx, join });
        }
        self.workers
            .get(site)
            .map(|h| &h.tx)
            .ok_or_else(|| anyhow::anyhow!("no worker for {site}"))
    }

    /// Sends `command` to `site`'s worker, starting it if needed.
    async fn send(
        &mut self,
        site: &str,
        command: Command,
    ) -> anyhow::Result<()> {
        let tx = self.worker(site).await?;
        tx.send(command)
            .map_err(|_| anyhow::anyhow!("the {site} worker has stopped"))
    }

    /// Sends a command with a reply to an existing worker and waits.
    async fn ask<T>(
        &self,
        site: &str,
        make: impl FnOnce(oneshot::Sender<T>) -> Command,
    ) -> Option<T> {
        let handle = self.workers.get(site)?;
        let (tx, rx) = oneshot::channel();
        handle.tx.send(make(tx)).ok()?;
        rx.await.ok()
    }

    /// Sends a command with a reply to every worker, then waits for all:
    /// the workers run it at the same time.
    async fn ask_all(&self, make: impl Fn(oneshot::Sender<()>) -> Command) {
        let mut replies = Vec::new();
        for handle in self.workers.values() {
            let (tx, rx) = oneshot::channel();
            if handle.tx.send(make(tx)).is_ok() {
                replies.push(rx);
            }
        }
        for rx in replies {
            let _ = rx.await;
        }
    }

    /// One scheduler tick on every site at once (0028, 0062); the daemon's
    /// loop leaves ticks to the workers' own timers.
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        self.reload(false).await?;
        self.ensure_workers().await?;
        self.ask_all(Command::Tick).await;
        self.heartbeat().await;
        Ok(())
    }

    /// Runs every site's queued manual checks, the sites in parallel.
    pub async fn run_manual(&mut self) {
        self.ask_all(Command::RunManual).await;
    }

    /// Runs one check of `family` now on its site's worker, and records
    /// everything it saw.
    pub async fn run_check(
        &mut self,
        family: FamilyId,
        reason: QueryReason,
    ) -> anyhow::Result<()> {
        let record = self
            .store
            .family(family)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no family {family}"))?;
        let site = worker_site(&record.site).to_owned();
        self.worker(&site).await?;
        self.ask(&site, |reply| Command::Check {
            family,
            reason,
            reply,
        })
        .await
        .unwrap_or_else(|| Err(anyhow::anyhow!("the {site} worker stopped")))
    }

    /// Everything a tick needs from the configuration.
    pub async fn world(&self) -> anyhow::Result<World> {
        worker::world(&self.store).await
    }

    /// The candidate filters for `world` at `now`: the sites in a block
    /// cooldown (from the block log) and those paused for a login.
    pub async fn context(
        &self,
        world: &World,
        now: chrono::DateTime<Utc>,
    ) -> anyhow::Result<Context> {
        let cooling =
            budget::cooling_sites(&self.store, &self.loaded.params, now)
                .await?;
        Ok(worker::context(
            world,
            &self.loaded.settings,
            &self.config.only,
            cooling,
            self.paused.keys().cloned().collect(),
        ))
    }

    // -----------------------------------------------------------------
    // Requests (0046)
    // -----------------------------------------------------------------

    /// Takes every pending request and hands it to its site's worker.
    /// Outside the daemon's loop, waits until the workers took them.
    pub async fn process_requests(&mut self) -> anyhow::Result<()> {
        let mut any = false;
        while let Some(request) = self.store.claim_next().await? {
            info!(id = %request.id, kind = request.kind.name(), "request");
            self.handle_request(request).await?;
            any = true;
        }
        if any && !self.autonomous {
            self.ask_all(Command::Sync).await;
        }
        Ok(())
    }

    async fn handle_request(
        &mut self,
        request: RequestRecord,
    ) -> anyhow::Result<()> {
        let id = request.id;
        match request.kind {
            RequestKind::OpenLogin { program } => {
                let site = miles_site(&program);
                info!(%site, "pausing the site for a login");
                self.paused.insert(site.clone(), program.clone());
                self.send(
                    &site,
                    Command::Pause {
                        program,
                        request: Some(id),
                    },
                )
                .await?;
            }
            RequestKind::CloseLogin {
                program: Some(program),
            } => {
                let site = miles_site(&program);
                self.paused.remove(&site);
                self.send(
                    &site,
                    Command::Resume {
                        program: Some(program),
                        request: Some(id),
                    },
                )
                .await?;
            }
            RequestKind::CloseLogin { program: None } => {
                // An app before 0062: resume every paused site.
                let paused = std::mem::take(&mut self.paused);
                let mut request = Some(id);
                for (site, program) in paused {
                    self.send(
                        &site,
                        Command::Resume {
                            program: Some(program),
                            request: request.take(),
                        },
                    )
                    .await?;
                }
                if let Some(id) = request {
                    self.store.complete(id, true, Some("resumed")).await?;
                }
            }
            RequestKind::ReloadConfig => {
                self.reload(true).await?;
                self.ensure_workers().await?;
                self.store.complete(id, true, None).await?;
            }
            RequestKind::CheckRoute { route } => {
                let world = self.world().await?;
                let cx = self.manual_context(&world).await?;
                let ids = candidates::of_route(&world.families, route, &cx);
                self.queue_manual(id, &world, ids).await?;
            }
            RequestKind::CheckFamily { family } => {
                let world = self.world().await?;
                let mut cx = self.manual_context(&world).await?;
                // Manual checks ignore the debugging restriction.
                cx.only = Only::default();
                let ids: Vec<FamilyId> = world
                    .families
                    .iter()
                    .filter(|f| f.id == family)
                    .filter(|f| candidates::eligibility(f, &cx).is_ok())
                    .map(|f| f.id)
                    .collect();
                self.queue_manual(id, &world, ids).await?;
            }
            RequestKind::CheckAward {
                route,
                cabin,
                departure,
                return_date,
            } => {
                let dates = [
                    (Leg::Outbound, Some(departure)),
                    (Leg::Return, return_date),
                ];
                self.check_award(id, route, cabin, dates).await?;
            }
            RequestKind::ConfirmDeal { deal } => match self
                .store
                .deal(deal)
                .await?
            {
                Some(d)
                    if d.status == DealStatus::Candidate
                        || d.status == DealStatus::Confirmed =>
                {
                    self.send(GOOGLE_FLIGHTS, Command::Confirm(deal, id))
                        .await?;
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

    /// "Check award" (0083, 0085): the selected trip's legs, each a
    /// LATAM search of its own date, on every active miles program with
    /// a reader and a live login. They run on the program's worker in
    /// turn with its schedule, as "Check now" does (0071).
    async fn check_award(
        &mut self,
        request: RequestId,
        route: tm_domain::RouteId,
        cabin: tm_domain::Cabin,
        dates: [(Leg, Option<chrono::NaiveDate>); 2],
    ) -> anyhow::Result<()> {
        let world = self.world().await?;
        let Some(home) = self.store.home_point_of_sale().await? else {
            self.store
                .complete(request, false, Some("no home point of sale"))
                .await?;
            return Ok(());
        };
        let mut why = "no active miles program can price awards".to_owned();
        let mut by_site: BTreeMap<String, Vec<(FamilyId, AwardKey)>> =
            BTreeMap::new();
        for program in self.store.list_miles_programs().await? {
            if !program.active || !has_award_reader(&program.program) {
                continue;
            }
            if program.login_state != LoginState::LoggedIn {
                why = format!(
                    "{} is not logged in",
                    program_name(&program.program)
                );
                continue;
            }
            for (leg, date) in dates {
                let Some(date) = date else { continue };
                let key = FamilyKey {
                    route,
                    cabin,
                    source: Source::Miles {
                        program: program.program.clone(),
                    },
                    pos: home.id,
                    kind: FamilyKind::Leg { leg },
                };
                if let Some(f) =
                    world.families.iter().find(|f| f.key == key && f.active)
                {
                    by_site
                        .entry(worker_site(&f.site).to_owned())
                        .or_default()
                        .push((f.id, (date, None)));
                }
            }
        }
        if by_site.is_empty() {
            self.store.complete(request, false, Some(&why)).await?;
            return Ok(());
        }
        self.tracker
            .expect(request, by_site.values().map(Vec::len).sum());
        for (site, checks) in by_site {
            self.send(&site, Command::QueueDated(checks, Some(request)))
                .await?;
        }
        Ok(())
    }

    /// The filters of a manual check: as a tick's, except that a paused
    /// site's checks wait for it to resume instead of being dropped.
    async fn manual_context(&self, world: &World) -> anyhow::Result<Context> {
        let mut cx = self.context(world, Utc::now()).await?;
        cx.paused.clear();
        Ok(cx)
    }

    /// Hands a request's checks to their sites' workers.
    async fn queue_manual(
        &mut self,
        request: RequestId,
        world: &World,
        ids: Vec<FamilyId>,
    ) -> anyhow::Result<()> {
        if ids.is_empty() {
            self.store
                .complete(request, false, Some("nothing can be checked now"))
                .await?;
            return Ok(());
        }
        let mut by_site: BTreeMap<String, Vec<FamilyId>> = BTreeMap::new();
        for id in ids {
            if let Some(f) = world.families.iter().find(|f| f.id == id) {
                by_site
                    .entry(worker_site(&f.site).to_owned())
                    .or_default()
                    .push(id);
            }
        }
        self.tracker
            .expect(request, by_site.values().map(Vec::len).sum());
        for (site, ids) in by_site {
            self.send(&site, Command::Queue(ids, Some(request))).await?;
        }
        Ok(())
    }
}

/// The site of a miles program (`miles:smiles`).
fn miles_site(program: &str) -> String {
    Source::Miles {
        program: program.to_owned(),
    }
    .site()
}
