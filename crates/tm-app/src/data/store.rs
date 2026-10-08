//! The daemon's SQLite file, read through `tm-store` (DECISIONS.md
//! 0020, 0040).
//!
//! sqlx runs on tokio, so every call is spawned on the app's tokio
//! runtime and only its result is awaited by the interface. The store
//! is opened on first use: a missing or locked file shows as an error
//! in the window, retried with every refresh. Leg and history reads are
//! cached per family and only re-read when the family was checked
//! again; those that are stale are read together, one statement for
//! all the latest cells and one for all the histories.
//!
//! A query that only selects another row (or departure, or trip
//! length) while the watcher below has noticed no write since the last
//! snapshot reuses that snapshot's rows and prices only the detail.
//!
//! Changes (0057): a task on the same runtime keeps one connection of
//! its own ([`tm_store::ChangeWatch`], not from the pool) and reads
//! `PRAGMA data_version` every [`POLL`]; when another connection
//! committed, it tells the window, which refreshes (debounced).

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{
        Arc,
        Mutex,
        OnceLock,
        Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration as StdDuration,
};

use anyhow::Context as _;
use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use tm_domain::{
    Cabin,
    CountryCode,
    Deal,
    DealStatus,
    FamilyId,
    FamilyKind,
    Leg,
    Money,
    Observation,
    Place,
    PlaceId,
    PointOfSale,
    PosId,
    ProgramSettings,
    QueryOutcome,
    QueryReason,
    RequestId,
    Route,
    Series,
    Settings,
    SiteLimits,
    Source,
    effective_brl,
    has_award_reader,
};
use tm_scheduler::finds::{ExploreFind, FindParams, explore_finds};
use tm_store::{
    ChangeWatch,
    DealFilter,
    FamilyRecord,
    LatestCell,
    LoginState,
    PriceCell,
    QueryLogEntry,
    RequestKind,
    RequestStatus,
    Store,
};
use tokio::{runtime::Handle, sync::OnceCell};

use super::{
    ActivityRow,
    Backend,
    BlockRow,
    ChangeSender,
    Changes,
    CurrentCheck,
    DaemonStatus,
    ExploreSummary,
    FeedItem,
    Login,
    MilesRow,
    NewPlace,
    NewPos,
    Request,
    Role,
    RouteDetail,
    RouteRow,
    RowKey,
    RowStatus,
    SellerRow,
    SharedBackend,
    SiteRow,
    Snapshot,
    SnapshotQuery,
    Ticket,
    TicketState,
    Tone,
    Trip,
    awards::{AwardInputs, AwardMarks, route_awards, summary},
    detail::{Baselines, DetailInputs, assemble},
    found,
    history::{self, History},
};
use crate::{format, grid::TripInputs};

/// A cached read and the last check it reflects.
type Stamped<T> = (Option<DateTime<Utc>>, Arc<T>);

/// The latest Explore hint and the configuration version.
type FindsStamp = (Option<DateTime<Utc>>, i64);

/// Cached reads, keyed by when the family (or route) was last checked.
#[derive(Default)]
struct Cache {
    /// The latest cells of the families rows are priced from.
    latest: HashMap<FamilyId, Stamped<Vec<LatestCell>>>,
    /// The latest cells of award families, whole (0083).
    cells: HashMap<FamilyId, Stamped<Vec<Observation>>>,
    history: HashMap<RowKey, Stamped<History>>,
    /// An award family's last 60 days (0083).
    award_history: HashMap<FamilyId, Stamped<Vec<Observation>>>,
    /// Explore finds (0087), as of the latest hint and the configuration
    /// version: they change when a sweep lands or the watchlist does.
    finds: Option<(FindsStamp, Arc<Vec<ExploreFind>>)>,
}

struct Inner {
    path: PathBuf,
    store: OnceCell<Store>,
    cache: Mutex<Cache>,
    last: Mutex<Option<Arc<Last>>>,
    /// Held while a snapshot is read: one at a time.
    turn: tokio::sync::Mutex<()>,
    /// Snapshots that read latest cells from the database.
    cold_reads: AtomicUsize,
    /// Snapshots being read now, and the most there ever were at once.
    reading: AtomicUsize,
    most_at_once: AtomicUsize,
}

pub struct StoreBackend {
    inner: Arc<Inner>,
    tokio: Handle,
    /// Started by the first [`Backend::changes`].
    changes: OnceLock<ChangeSender>,
}

/// How slow a statement is before sqlx warns: the first snapshot's
/// large reads are expected, and each snapshot logs its own time.
const SLOW_STATEMENT: StdDuration = StdDuration::from_secs(5);

/// How often the watcher reads `data_version`.
pub const POLL: StdDuration = StdDuration::from_millis(250);

/// How long the watcher waits to retry a database it cannot open.
const REOPEN: StdDuration = StdDuration::from_secs(2);

/// The backend for the database at `path`; nothing is opened yet.
pub fn backend(path: PathBuf, tokio: Handle) -> SharedBackend {
    Arc::new(StoreBackend::new(path, tokio))
}

impl StoreBackend {
    fn new(path: PathBuf, tokio: Handle) -> Self {
        Self {
            inner: Arc::new(Inner {
                path,
                store: OnceCell::new(),
                cache: Mutex::new(Cache::default()),
                last: Mutex::new(None),
                turn: tokio::sync::Mutex::new(()),
                cold_reads: AtomicUsize::new(0),
                reading: AtomicUsize::new(0),
                most_at_once: AtomicUsize::new(0),
            }),
            tokio,
            changes: OnceLock::new(),
        }
    }
}

/// Watches the database for commits by any other connection (the
/// daemon's, or the app's own pool) until the backend is dropped. The
/// file may not exist yet: the first snapshot creates it. A connection
/// that fails is reopened, and a reopen counts as a change, since
/// writes may have been missed meanwhile.
async fn watch_changes(inner: Weak<Inner>, path: PathBuf, tx: ChangeSender) {
    let mut watch: Option<ChangeWatch> = None;
    while inner.strong_count() > 0 {
        let Some(w) = watch.as_mut() else {
            match ChangeWatch::open(&path).await {
                Ok(mut w) => {
                    if let Err(error) = w.changed().await {
                        tracing::debug!("cannot watch the database: {error}");
                        tokio::time::sleep(REOPEN).await;
                        continue;
                    }
                    watch = Some(w);
                    tx.notify();
                }
                Err(error) => {
                    tracing::debug!("cannot watch the database: {error}");
                    tokio::time::sleep(REOPEN).await;
                }
            }
            continue;
        };
        match w.changed().await {
            Ok(true) => tx.notify(),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!("watching the database failed: {error}");
                watch = None;
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

impl StoreBackend {
    /// Runs `f` with the store on tokio and awaits its result.
    async fn run<T, F>(
        &self,
        f: impl FnOnce(Arc<Inner>) -> F + Send + 'static,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let inner = self.inner.clone();
        self.tokio
            .spawn(async move { f(inner).await })
            .await
            .context("the database task stopped")?
    }
}

impl Inner {
    async fn store(&self) -> anyhow::Result<&Store> {
        self.store
            .get_or_try_init(|| async {
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir).with_context(|| {
                        format!("cannot create {}", dir.display())
                    })?;
                }
                let store = Store::open_with(&self.path, SLOW_STATEMENT)
                    .await
                    .with_context(|| {
                        format!("cannot open {}", self.path.display())
                    })?;
                // A fresh file gets SAO and the watchlist, as the daemon
                // would (0035).
                store.seed_defaults().await?;
                anyhow::Ok(store)
            })
            .await
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The last snapshot, when `query` can be answered from its rows:
    /// no change notice since it was read (`changes`), the same origin
    /// and day, not long ago, and a different query (asking the same
    /// again, as the fallback refresh does, reads everything).
    fn reusable(
        &self,
        query: &SnapshotQuery,
        changes: u64,
    ) -> Option<Arc<Last>> {
        let last = self
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        let same_origin = query.origin == last.query.origin
            || query.origin == last.snapshot.origin;
        (last.changes == changes
            && same_origin
            && *query != last.query
            && last.at.elapsed() < REUSE_FOR
            && Utc::now().date_naive() == last.pricing.today)
            .then_some(last)
    }

    fn keep(&self, last: Last) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Arc::new(last));
    }

    async fn cells(
        &self,
        store: &Store,
        family: &FamilyRecord,
    ) -> anyhow::Result<Arc<Vec<Observation>>> {
        let stamp = family.state.last_check;
        if let Some((at, cells)) = self.lock().cells.get(&family.id)
            && *at == stamp
        {
            return Ok(cells.clone());
        }
        let cells = Arc::new(store.latest_cells(family.id).await?);
        self.lock().cells.insert(family.id, (stamp, cells.clone()));
        Ok(cells)
    }

    async fn award_history(
        &self,
        store: &Store,
        family: &FamilyRecord,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Arc<Vec<Observation>>> {
        let stamp = family.state.last_check;
        if let Some((at, obs)) = self.lock().award_history.get(&family.id)
            && *at == stamp
        {
            return Ok(obs.clone());
        }
        let obs =
            Arc::new(store.history(family.id, now - Duration::days(60)).await?);
        self.lock()
            .award_history
            .insert(family.id, (stamp, obs.clone()));
        Ok(obs)
    }

    async fn finds(
        &self,
        store: &Store,
        settings: &Settings,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Arc<Vec<ExploreFind>>> {
        let stamp = (
            store.latest_explore_hint_at().await?,
            store.config_version().await?,
        );
        if let Some((at, finds)) = &self.lock().finds
            && *at == stamp
        {
            return Ok(finds.clone());
        }
        let params = FindParams::from_settings(settings);
        let hints = store.unwatched_explore_hints(params.since(now)).await?;
        let finds = Arc::new(explore_finds(&hints, now, &params));
        self.lock().finds = Some((stamp, finds.clone()));
        Ok(finds)
    }

    /// Reads the latest cells of every family of `rows` whose
    /// cached cells are older than its last check, in one statement, so
    /// that [`Inner::cells`] then finds them cached.
    async fn prefetch_cells(
        &self,
        store: &Store,
        rows: &[RowSpec<'_>],
    ) -> anyhow::Result<usize> {
        let stale: Vec<(FamilyId, Option<DateTime<Utc>>)> = {
            let cache = self.lock();
            rows.iter()
                .flat_map(|row| row.families.iter())
                .filter(|f| {
                    cache
                        .latest
                        .get(&f.id)
                        .is_none_or(|(at, _)| *at != f.state.last_check)
                })
                .map(|f| (f.id, f.state.last_check))
                .collect()
        };
        if stale.is_empty() {
            return Ok(0);
        }
        let ids: Vec<FamilyId> = stale.iter().map(|(id, _)| *id).collect();
        let reads = ids.chunks(ids.len().div_ceil(READERS)).map(|ids| {
            let (store, ids) = (store.clone(), ids.to_vec());
            tokio::spawn(async move { store.latest_cells_of(&ids).await })
        });
        let mut by_family: HashMap<FamilyId, Vec<LatestCell>> = HashMap::new();
        for read in reads.collect::<Vec<_>>() {
            by_family.extend(read.await??);
        }
        let read = stale.len();
        let mut cache = self.lock();
        for (id, stamp) in stale {
            let cells = by_family.remove(&id).unwrap_or_default();
            cache.latest.insert(id, (stamp, Arc::new(cells)));
        }
        Ok(read)
    }

    /// A row family's latest cells: from the cache [`Inner::prefetch_cells`]
    /// filled, else read.
    async fn latest(
        &self,
        store: &Store,
        family: &FamilyRecord,
    ) -> anyhow::Result<Arc<Vec<LatestCell>>> {
        let stamp = family.state.last_check;
        if let Some((at, cells)) = self.lock().latest.get(&family.id)
            && *at == stamp
        {
            return Ok(cells.clone());
        }
        let cells = Arc::new(
            store
                .latest_cells_of(&[family.id])
                .await?
                .pop()
                .map(|(_, cells)| cells)
                .unwrap_or_default(),
        );
        self.lock().latest.insert(family.id, (stamp, cells.clone()));
        Ok(cells)
    }

    /// Each row's price history (60 days), cached until one of its
    /// families is checked again, and how many rows were read: those
    /// whose cache is stale are read together.
    async fn histories(
        &self,
        store: &Store,
        rows: &[RowSpec<'_>],
        settings: &Settings,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(HashMap<RowKey, Arc<History>>, usize)> {
        let mut out = HashMap::new();
        let mut stale: Vec<(&RowSpec<'_>, Option<DateTime<Utc>>)> = Vec::new();
        {
            let cache = self.lock();
            for row in rows {
                let stamp = row.last_check();
                match cache.history.get(&row.key) {
                    Some((at, h)) if *at == stamp => {
                        out.insert(row.key, h.clone());
                    }
                    _ => stale.push((row, stamp)),
                }
            }
        }
        if stale.is_empty() {
            return Ok((out, 0));
        }
        let mut owner: HashMap<FamilyId, (usize, FamilyKind)> = HashMap::new();
        for (i, (row, _)) in stale.iter().enumerate() {
            for f in &row.families {
                owner.insert(f.id, (i, f.key.kind));
            }
        }
        // Each row's families stay in one read, so that its cells keep
        // their order (by time, then insertion).
        let since = now - Duration::days(60);
        let reads = stale.chunks(stale.len().div_ceil(READERS)).map(|rows| {
            let ids: Vec<FamilyId> = rows
                .iter()
                .flat_map(|(row, _)| row.families.iter().map(|f| f.id))
                .collect();
            let store = store.clone();
            tokio::spawn(async move { store.price_history(&ids, since).await })
        });
        let mut cells: Vec<Vec<(FamilyKind, PriceCell)>> =
            vec![Vec::new(); stale.len()];
        for read in reads.collect::<Vec<_>>() {
            for c in read.await?? {
                if let Some((i, kind)) = owner.get(&c.family) {
                    cells[*i].push((*kind, c));
                }
            }
        }
        let read = stale.len();
        let mut cache = self.lock();
        for ((row, stamp), cells) in stale.into_iter().zip(cells) {
            let h = Arc::new(history::history(
                &cells,
                row.discount,
                settings.trips,
            ));
            cache.history.insert(row.key, (stamp, h.clone()));
            out.insert(row.key, h);
        }
        Ok((out, read))
    }
}

/// How many statements of a batched read run at once, each on its own
/// task and pooled connection: decoding rows is most of a large read's
/// time, and so it spreads over the runtime's threads.
const READERS: usize = 4;

/// A row of the list before it is priced: its route × cabin, the
/// families its prices come from and its route's round-trip discount.
struct RowSpec<'a> {
    route: &'a Route,
    key: RowKey,
    families: Vec<&'a FamilyRecord>,
    discount: f64,
}

impl RowSpec<'_> {
    /// The row's last check: what its cached history is keyed by.
    fn last_check(&self) -> Option<DateTime<Utc>> {
        self.families
            .iter()
            .filter_map(|f| f.state.last_check)
            .max()
    }
}

/// Every row of `routes`, with its families (Google Flights at the home
/// point of sale, legs and round trips, in id order) and discount,
/// grouped in one pass over the families.
fn row_specs<'a>(
    routes: &'a [Route],
    families: &'a [FamilyRecord],
    home: Option<PosId>,
) -> Vec<RowSpec<'a>> {
    let mut by_row: HashMap<RowKey, Vec<&FamilyRecord>> = HashMap::new();
    let mut discounts: HashMap<tm_domain::RouteId, f64> = HashMap::new();
    for f in families {
        // The route's own state, joined onto each of its families.
        discounts
            .entry(f.key.route)
            .or_insert_with(|| discount_of(Some(f.state.route_trip())));
        if f.key.source == Source::GoogleFlights
            && home.is_none_or(|h| f.key.pos == h)
            && !matches!(f.key.kind, FamilyKind::Explore)
        {
            by_row
                .entry(RowKey {
                    route: f.key.route,
                    cabin: f.key.cabin,
                })
                .or_default()
                .push(f);
        }
    }
    routes
        .iter()
        .flat_map(|route| {
            let discount = discounts.get(&route.id).copied().unwrap_or(0.0);
            let by_row = &by_row;
            route.cabins().iter().map(move |&cabin| {
                let key = RowKey {
                    route: route.id,
                    cabin,
                };
                RowSpec {
                    route,
                    key,
                    families: by_row.get(&key).cloned().unwrap_or_default(),
                    discount,
                }
            })
        })
        .collect()
}

/// A miles program's family of a route × cabin at the home point of
/// sale: a leg, or the round trip of `days` days.
fn award_family<'a>(
    families: &'a [FamilyRecord],
    route: tm_domain::RouteId,
    cabin: Cabin,
    program: &str,
    home: Option<PosId>,
    kind: FamilyKind,
) -> Option<&'a FamilyRecord> {
    families.iter().find(|f| {
        f.key.route == route
            && f.key.cabin == cabin
            && f.key.kind == kind
            && home.is_none_or(|h| f.key.pos == h)
            && matches!(&f.key.source, Source::Miles { program: p } if p == program)
    })
}

/// The miles programs that may lead a row: active ones, those the
/// daemon reads first (0083: with LATAM Pass alone, it leads).
fn leading_programs(miles: &[MilesRow]) -> Vec<&MilesRow> {
    let mut out: Vec<&MilesRow> = miles.iter().filter(|m| m.active()).collect();
    out.sort_by_key(|m| !m.has_reader);
    out
}

/// What a program's families say about a route × cabin, read once.
struct AwardRead {
    outbound: Arc<Vec<Observation>>,
    inbound: Arc<Vec<Observation>>,
    round_trip: Arc<Vec<Observation>>,
    others: Vec<(Cabin, Observation)>,
    history: Arc<Vec<Observation>>,
    checked_at: Option<DateTime<Utc>>,
}

#[allow(clippy::too_many_arguments, reason = "one row's award read")]
async fn read_awards(
    inner: &Inner,
    store: &Store,
    families: &[FamilyRecord],
    route: &Route,
    cabin: Cabin,
    program: &str,
    home: Option<PosId>,
    days: u16,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<AwardRead>> {
    let leg = |leg| FamilyKind::Leg { leg };
    let find = |cabin, kind| {
        award_family(families, route.id, cabin, program, home, kind)
    };
    let empty = || Arc::new(Vec::new());
    let out = find(cabin, leg(Leg::Outbound));
    let ret = find(cabin, leg(Leg::Return));
    let rt = find(cabin, FamilyKind::RoundTrip { days });
    let fams: Vec<&FamilyRecord> =
        [out, ret, rt].into_iter().flatten().collect();
    if fams.iter().all(|f| f.state.last_check.is_none()) {
        return Ok(None);
    }
    let mut read = AwardRead {
        outbound: empty(),
        inbound: empty(),
        round_trip: empty(),
        others: Vec::new(),
        history: empty(),
        checked_at: fams.iter().filter_map(|f| f.state.last_check).max(),
    };
    if let Some(f) = out {
        read.outbound = inner.cells(store, f).await?;
        read.history = inner.award_history(store, f, now).await?;
    }
    if let Some(f) = ret {
        read.inbound = inner.cells(store, f).await?;
    }
    if let Some(f) = rt {
        read.round_trip = inner.cells(store, f).await?;
    }
    for &other in route.cabins() {
        if other == cabin {
            continue;
        }
        if let Some(f) = find(other, leg(Leg::Outbound)) {
            for o in inner.cells(store, f).await?.iter() {
                read.others.push((other, o.clone()));
            }
        }
    }
    Ok(Some(read))
}

fn discount_of(state: Option<tm_store::RouteTripState>) -> f64 {
    match state {
        Some(s) if !s.additive && s.rt_discount_n > 0 => {
            s.rt_discount_ewma.clamp(0.0, 0.5)
        }
        _ => 0.0,
    }
}

/// The search link for a trip: a generated `tfs` URL when both ends
/// have Google IDs (0041), else a text search Google understands.
pub fn search_url(
    route: &Route,
    cabin: Cabin,
    settings: &Settings,
    departure: NaiveDate,
    return_date: NaiveDate,
) -> String {
    use tm_gflights::{LegSpec, Locale, PlaceRef, Search, Trip as GTrip, url};
    let place_ref = |p: &Place| {
        p.google_id().map(|id| match p.kind {
            tm_domain::PlaceKind::Airport => PlaceRef::Airport(id.to_owned()),
            tm_domain::PlaceKind::City { .. } => PlaceRef::City(id.to_owned()),
        })
    };
    if let (Some(from), Some(to)) =
        (place_ref(&route.origin), place_ref(&route.destination))
    {
        let search = Search::new(
            GTrip::RoundTrip,
            cabin,
            settings.passengers,
            vec![
                LegSpec::new(departure, from.clone(), to.clone()),
                LegSpec::new(return_date, to, from),
            ],
        );
        return url::search_url(&search, &Locale::english("BR"));
    }
    let cabin = match cabin {
        Cabin::Economy => "",
        Cabin::PremiumEconomy => " premium economy",
        Cabin::Business => " business class",
        Cabin::First => " first class",
    };
    let q = format!(
        "Flights from {} to {} on {departure} through {return_date}{cabin}",
        route.origin.code, route.destination.code
    );
    format!(
        "https://www.google.com/travel/flights?q={}&hl=en&gl=BR&curr=BRL",
        url::encode_component(&q)
    )
}

fn kind_label(kind: FamilyKind) -> String {
    match kind {
        FamilyKind::Leg { leg: Leg::Outbound } => "Outbound leg".to_owned(),
        FamilyKind::Leg { leg: Leg::Return } => "Return leg".to_owned(),
        FamilyKind::RoundTrip { days } => format!("Round trip {days} d"),
        FamilyKind::Explore => "Explore".to_owned(),
    }
}

fn reason_label(reason: &QueryReason) -> String {
    match reason {
        QueryReason::Scheduled { score, .. } => {
            format!("Scheduled · score {score:.2}")
        }
        QueryReason::Random => "Random sample".to_owned(),
        QueryReason::Overdue => "Overdue".to_owned(),
        QueryReason::Hint => "Explore hint".to_owned(),
        QueryReason::FanOut => "Fan-out from a drop".to_owned(),
        QueryReason::Confirmation => "Confirm deal".to_owned(),
        QueryReason::Manual => "Asked from the app".to_owned(),
        QueryReason::Bootstrap => "First check".to_owned(),
    }
}

/// `out`, `return`, `round trip`: what an award check priced.
fn award_kind(kind: FamilyKind) -> &'static str {
    match kind {
        FamilyKind::Leg { leg: Leg::Outbound } => "out",
        FamilyKind::Leg { leg: Leg::Return } => "return",
        FamilyKind::RoundTrip { .. } => "round trip",
        FamilyKind::Explore => "explore",
    }
}

fn activity_row(
    q: &QueryLogEntry,
    pos: &HashMap<PosId, CountryCode>,
) -> ActivityRow {
    let source = match &q.key.source {
        Source::GoogleFlights => String::new(),
        other => format!(" · {}", other.site()),
    };
    let pos = pos.get(&q.key.pos).map_or("?", CountryCode::as_str);
    // An award check reads every cabin: it names the program (0083).
    if let Source::Miles { program } = &q.key.source {
        let (result, tone) = match &q.outcome {
            None => ("running".to_owned(), Tone::System),
            Some(QueryOutcome::Ok) => {
                (format!("{} awards", q.cells), Tone::Neutral)
            }
            Some(QueryOutcome::NoFlights) => {
                ("no award seats".to_owned(), Tone::Neutral)
            }
            Some(QueryOutcome::LoggedOut) => {
                ("login needed".to_owned(), Tone::Bad)
            }
            Some(QueryOutcome::Blocked { detail }) => {
                (format!("blocked: {detail}"), Tone::Bad)
            }
            Some(QueryOutcome::Unexpected { detail }) => {
                (format!("unexpected page: {detail}"), Tone::Bad)
            }
            Some(QueryOutcome::Error { detail }) => {
                (format!("error: {detail}"), Tone::Bad)
            }
        };
        return ActivityRow {
            id: q.id.0,
            at: q.started_at,
            route: format!("{} → {}", q.origin_code, q.destination_code),
            query: format!(
                "Award · {} · {} · {pos}",
                tm_domain::program_name(program),
                award_kind(q.key.kind)
            ),
            reason: reason_label(&q.reason),
            result,
            tone,
            loads: q.page_loads,
            running: q.outcome.is_none(),
        };
    }
    let (result, tone) = match &q.outcome {
        None => ("running".to_owned(), Tone::System),
        Some(QueryOutcome::Ok) if q.sweep => {
            (format!("{} destinations", q.cells), Tone::System)
        }
        Some(QueryOutcome::Ok) => {
            (format!("{} prices", q.cells), Tone::Neutral)
        }
        Some(QueryOutcome::Blocked { detail }) => {
            (format!("blocked: {detail}"), Tone::Bad)
        }
        Some(QueryOutcome::Unexpected { detail }) => {
            (format!("unexpected page: {detail}"), Tone::Bad)
        }
        // The page loaded and has nothing to sell: quiet, not an error
        // (0067).
        Some(QueryOutcome::NoFlights) => {
            ("no flights".to_owned(), Tone::Neutral)
        }
        Some(QueryOutcome::LoggedOut) => ("logged out".to_owned(), Tone::Bad),
        Some(QueryOutcome::Error { detail }) => {
            (format!("error: {detail}"), Tone::Bad)
        }
    };
    ActivityRow {
        id: q.id.0,
        at: q.started_at,
        route: if q.sweep {
            format!("{} → anywhere", q.origin_code)
        } else {
            format!("{} → {}", q.origin_code, q.destination_code)
        },
        query: format!(
            "{} · {} · {pos}{source}",
            kind_label(q.key.kind),
            q.key.cabin.label()
        ),
        reason: reason_label(&q.reason),
        result,
        tone,
        loads: q.page_loads,
        running: q.outcome.is_none(),
    }
}

/// Seller quotes as effective BRL: home prices as shown, foreign ones
/// from the seller's own currency (or the shown BRL) with IOF and the
/// card spread (0038). The cheapest per point of sale.
fn seller_rows(
    deal: &Deal,
    pos: &[PointOfSale],
    rates: &HashMap<String, f64>,
    settings: &Settings,
) -> (Vec<SellerRow>, Option<(String, Money)>) {
    let mut best: Vec<SellerRow> = Vec::new();
    let mut direct: Option<(String, Money)> = None;
    for q in &deal.sellers {
        let Some(p) = pos.iter().find(|p| p.id == q.pos) else {
            continue;
        };
        let effective = if p.is_home {
            q.price.clone()
        } else {
            match &q.quoted {
                Some(quoted) => match rates.get(quoted.currency.as_str()) {
                    Some(rate) => effective_brl(
                        quoted,
                        *rate,
                        settings.iof,
                        settings.card_spread,
                    ),
                    None => effective_brl(
                        &q.price,
                        1.0,
                        settings.iof,
                        settings.card_spread,
                    ),
                },
                // Shown in BRL at Google's rate: the card still adds IOF
                // and its spread.
                None => {
                    let minor = q.price.minor as f64
                        * (1.0 + settings.iof)
                        * (1.0 + settings.card_spread);
                    Money::new(minor.round() as i64, q.price.currency.clone())
                }
            }
        };
        if q.is_airline
            && direct
                .as_ref()
                .is_none_or(|(_, m)| effective.minor < m.minor)
        {
            direct = Some((q.seller.clone(), effective.clone()));
        }
        let row = SellerRow {
            pos: p.country.clone(),
            seller: q.seller.clone(),
            quoted: q.quoted.clone(),
            effective,
            is_airline: q.is_airline,
        };
        match best.iter_mut().find(|r| r.pos == row.pos) {
            Some(r) if r.effective.minor > row.effective.minor => *r = row,
            Some(_) => {}
            None => best.push(row),
        }
    }
    best.sort_by_key(|r| pos.iter().position(|p| p.country == r.pos));
    (best, direct)
}

/// What every row of a snapshot is priced against, read once per
/// snapshot and kept with it for selections ([`Last`]).
struct Pricing {
    now: DateTime<Utc>,
    today: NaiveDate,
    settings: Settings,
    points_of_sale: Vec<PointOfSale>,
    home: Option<PosId>,
    pos_names: HashMap<PosId, CountryCode>,
    rates: HashMap<String, f64>,
    routes: Vec<Route>,
    families: Vec<FamilyRecord>,
    deals: Vec<Deal>,
    miles: Vec<MilesRow>,
}

/// The last full snapshot, the change notices it had seen and what it
/// was priced against.
struct Last {
    query: SnapshotQuery,
    changes: u64,
    at: std::time::Instant,
    pricing: Arc<Pricing>,
    snapshot: Snapshot,
}

/// How long the last snapshot's rows may serve selections. Writes end
/// it sooner (the change watcher's notices); this bounds a watcher that
/// stopped noticing.
const REUSE_FOR: StdDuration = StdDuration::from_secs(30);

/// One row of the list, and the detail when it is `query`'s selection.
async fn price_row(
    inner: &Inner,
    store: &Store,
    ctx: &Pricing,
    spec: &RowSpec<'_>,
    hist: Arc<History>,
    query: &SnapshotQuery,
) -> anyhow::Result<(RouteRow, Option<RouteDetail>)> {
    let (route, key, discount) = (spec.route, spec.key, spec.discount);
    let cabin = key.cabin;
    let fams = &spec.families;
    let horizon_end =
        ctx.today + Duration::days(i64::from(ctx.settings.horizon_days));
    let days = query
        .calendar_days
        .unwrap_or(ctx.settings.trips.round_trip_days);
    let mut detail = None;
    let mut cells: Vec<(FamilyKind, Arc<Vec<LatestCell>>)> = Vec::new();
    for f in fams {
        cells.push((f.key.kind, inner.latest(store, f).await?));
    }
    let inputs: TripInputs = history::trip_inputs(
        cells
            .iter()
            .flat_map(|(k, obs)| obs.iter().map(move |o| (*k, o))),
        discount,
    );
    let baselines = Baselines {
        by_departure: history::baselines_by_departure(
            &hist.points,
            ctx.now,
            ctx.settings.horizon_days,
        ),
    };
    // The newest live cash deal of this row: confirmed, or
    // waiting. Award deals are their own series (0023).
    let deal = ctx.deals.iter().find(|d| {
        d.series == tm_domain::Series::Cash
            && d.return_date.is_some()
            && d.route == route.id
            && d.cabin == cabin
            && d.departure >= ctx.today
            && matches!(d.status, DealStatus::Confirmed | DealStatus::Candidate)
    });
    let carrier_of = |dep: NaiveDate| {
        cells
            .iter()
            .flat_map(|(_, obs)| obs.iter())
            .find(|o| o.departure == dep && o.carrier.is_some())
    };
    let best = match deal.and_then(|d| Some((d, d.return_date?))) {
        Some((d, return_date)) => Some(Trip {
            departure: d.departure,
            return_date,
            price: d.effective_brl.clone(),
            carrier: d.carrier.clone(),
            stops: carrier_of(d.departure).and_then(|o| o.stops),
            duration_minutes: carrier_of(d.departure)
                .and_then(|o| o.duration_minutes),
            estimated: d.estimated,
        }),
        None => history::best_trip(
            &inputs,
            ctx.today,
            horizon_end,
            ctx.settings.trips,
        )
        .map(|(dep, n, p)| {
            let cell = carrier_of(dep);
            Trip {
                departure: dep,
                return_date: dep + Duration::days(i64::from(n)),
                price: Money::new(p.minor, tm_domain::Currency::brl()),
                carrier: cell.and_then(|o| o.carrier.clone()),
                stops: cell.and_then(|o| o.stops),
                duration_minutes: cell.and_then(|o| o.duration_minutes),
                estimated: !p.confirmed,
            }
        }),
    };
    let baseline = match deal {
        Some(d) => d.baseline.clone(),
        None => best
            .as_ref()
            .and_then(|t| baselines.at(t.departure))
            .map(|m| Money::new(m, tm_domain::Currency::brl())),
    };
    let change = match (deal.and_then(|d| d.change), &best, &baseline) {
        (Some(c), _, _) => Some(c),
        (None, Some(t), Some(b)) if b.minor > 0 => {
            Some(t.price.minor as f64 / b.minor as f64 - 1.0)
        }
        _ => None,
    };
    // The first program with awards on this route leads (0083).
    let mut award_read = None;
    for program in leading_programs(&ctx.miles) {
        if let Some(read) = read_awards(
            inner,
            store,
            &ctx.families,
            route,
            cabin,
            &program.program,
            ctx.home,
            ctx.settings.trips.round_trip_days,
            ctx.now,
        )
        .await?
        {
            award_read = Some((program, read));
            break;
        }
    }
    let award_inputs = award_read.as_ref().map(|(program, read)| {
        let series = Series::Award {
            program: program.program.clone(),
        };
        AwardInputs {
            program,
            cabin,
            outbound: &read.outbound,
            inbound: &read.inbound,
            round_trip: &read.round_trip,
            other_cabins: &read.others,
            history: &read.history,
            deal: ctx.deals.iter().find(|d| {
                d.series == series
                    && d.route == route.id
                    && d.cabin == cabin
                    && d.departure >= ctx.today
                    && matches!(
                        d.status,
                        DealStatus::Confirmed | DealStatus::Candidate
                    )
            }),
            checked_at: read.checked_at,
        }
    });
    let award = award_inputs.as_ref().and_then(|a| {
        summary(a, &inputs.legs, best.as_ref(), &ctx.settings, ctx.now)
    });
    let row = RouteRow {
        award,
        route: route.clone(),
        cabin,
        status: match deal.map(|d| d.status) {
            Some(DealStatus::Confirmed) => RowStatus::Deal,
            Some(DealStatus::Candidate) => RowStatus::Confirming,
            _ => RowStatus::Watching,
        },
        best: best.clone(),
        baseline,
        change,
        history: hist.daily_min.clone(),
        last_checked: fams.iter().filter_map(|f| f.state.last_check).max(),
        deal: deal.and_then(|d| d.id),
        found_at: deal.map(|d| d.confirmed_at.unwrap_or(d.found_at)),
    };
    if query.selected == Some(key) {
        let (sellers, airline_direct) = deal
            .map(|d| {
                seller_rows(d, &ctx.points_of_sale, &ctx.rates, &ctx.settings)
            })
            .unwrap_or_default();
        let url = deal.and_then(|d| d.url.clone()).or_else(|| {
            best.as_ref().map(|t| {
                search_url(
                    route,
                    cabin,
                    &ctx.settings,
                    t.departure,
                    t.return_date,
                )
            })
        });
        // Learned changes per day, from the outbound leg's
        // Gamma posterior (per hour).
        let change_rate = fams
            .iter()
            .find(|f| {
                matches!(f.key.kind, FamilyKind::Leg { leg: Leg::Outbound })
            })
            .filter(|f| f.state.chg_b > 0.0)
            .map(|f| f.state.chg_a / f.state.chg_b * 24.0);
        let checks = store.recent_queries(400).await?;
        let inputs = DetailInputs {
            trip: best.clone(),
            deal: row.status == RowStatus::Deal,
            trips: inputs.clone(),
            baselines,
            sellers,
            airline_direct,
            url,
            lowest_ever: hist.lowest.map(|(m, day)| {
                (
                    Money::new(m, tm_domain::Currency::brl()),
                    day,
                    "60 days".to_owned(),
                )
            }),
            change_rate,
            recent_checks: checks
                .iter()
                .filter(|q| q.key.route == route.id && q.key.cabin == cabin)
                .take(8)
                .map(|q| activity_row(q, &ctx.pos_names))
                .collect(),
            next_check: None,
            awards: award_inputs.as_ref().map(|a| {
                route_awards(
                    a,
                    &inputs.legs,
                    query.departure.or(best.as_ref().map(|t| t.departure)),
                    days,
                    &ctx.settings,
                    ctx.now,
                )
            }),
            award_marks: award_inputs.as_ref().map(AwardMarks::of),
            favorite: route.favorite,
        };
        detail = Some(assemble(
            key,
            inputs,
            &ctx.settings,
            ctx.today,
            days,
            query.departure,
        ));
    }
    Ok((row, detail))
}

/// A snapshot for `query` from `last`'s rows: nothing was written since
/// it was read, and `query` only picks another row, departure or trip
/// length, so only the detail is priced (DECISIONS.md 0057: the app
/// re-reads on the daemon's change notices).
async fn reselect(
    inner: &Inner,
    last: &Last,
    query: SnapshotQuery,
) -> anyhow::Result<Snapshot> {
    let store = inner.store().await?;
    let ctx = &last.pricing;
    let mut snapshot = last.snapshot.clone();
    snapshot.detail = None;
    let specs = row_specs(&ctx.routes, &ctx.families, ctx.home);
    if let Some(spec) = specs.iter().find(|s| Some(s.key) == query.selected) {
        let hist = inner
            .histories(
                store,
                std::slice::from_ref(spec),
                &ctx.settings,
                ctx.now,
            )
            .await?
            .0
            .remove(&spec.key)
            .context("a row without its history")?;
        snapshot.detail =
            price_row(inner, store, ctx, spec, hist, &query).await?.1;
    }
    Ok(snapshot)
}

/// What a snapshot read, for its log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// The last snapshot's rows, and the selection's detail.
    Selection,
    /// Every row: the cells of `cells` families and the histories of
    /// `histories` rows from the database (checked since they were
    /// cached), the rest from the cache.
    Rows { cells: usize, histories: usize },
}

/// A snapshot for `query`, one at a time: a refresh asked for while
/// another snapshot is being read waits for it, then finds what it
/// read cached, instead of reading the same cells and histories again
/// alongside it (the first open's reads take a while, and the daemon
/// writes meanwhile).
async fn snapshot(
    inner: &Inner,
    query: SnapshotQuery,
    changes: Option<u64>,
) -> anyhow::Result<Snapshot> {
    let _turn = inner.turn.lock().await;
    let started = std::time::Instant::now();
    let at_once = inner.reading.fetch_add(1, Ordering::Relaxed) + 1;
    inner.most_at_once.fetch_max(at_once, Ordering::Relaxed);
    let result = read_snapshot(inner, query, changes).await;
    inner.reading.fetch_sub(1, Ordering::Relaxed);
    let (snapshot, read) = result?;
    let ms = started.elapsed().as_millis();
    let rows = snapshot.rows.len();
    match read {
        Read::Selection => {
            tracing::debug!("snapshot: selection, {ms} ms, {rows} rows reused");
        }
        Read::Rows {
            cells: 0,
            histories: 0,
        } => tracing::debug!("snapshot: warm, {ms} ms, {rows} rows"),
        Read::Rows { cells, histories } => {
            // The first read and slow ones are worth a line; refreshes
            // that re-read the few families just checked come every few
            // seconds while the daemon works.
            static FIRST: std::sync::Once = std::sync::Once::new();
            let mut first = false;
            FIRST.call_once(|| first = true);
            if first || ms >= 1000 {
                tracing::info!(
                    "snapshot: cold, {ms} ms, {rows} rows ({cells} families' \
                     cells, {histories} histories read)"
                );
            } else {
                tracing::debug!(
                    "snapshot: {ms} ms, {rows} rows ({cells} families' \
                     cells, {histories} histories read)"
                );
            }
        }
    }
    if let Read::Rows { cells, .. } = read
        && cells > 0
    {
        inner.cold_reads.fetch_add(1, Ordering::Relaxed);
    }
    Ok(snapshot)
}

/// A snapshot for `query`. `changes` is the change watcher's count of
/// notices, when it is watching: while it has not moved, a query that
/// only changes the selection reuses the last snapshot's rows.
async fn read_snapshot(
    inner: &Inner,
    query: SnapshotQuery,
    changes: Option<u64>,
) -> anyhow::Result<(Snapshot, Read)> {
    if let Some(changes) = changes
        && let Some(last) = inner.reusable(&query, changes)
    {
        let snapshot = reselect(inner, &last, query).await?;
        inner.keep(Last {
            query,
            changes,
            at: last.at,
            pricing: last.pricing.clone(),
            snapshot: snapshot.clone(),
        });
        return Ok((snapshot, Read::Selection));
    }
    let started = std::time::Instant::now();
    let store = inner.store().await?;
    let now = Utc::now();
    let today = now.date_naive();
    let settings = store.settings().await?;
    let origins: Vec<Place> = store
        .list_origins(true)
        .await?
        .into_iter()
        .map(|o| o.place)
        .collect();
    let (favorites, destinations) = destinations_of(store).await?;
    let points_of_sale = store.list_points_of_sale(false).await?;
    let home = points_of_sale.iter().find(|p| p.is_home).map(|p| p.id);
    let pos_names: HashMap<PosId, CountryCode> = points_of_sale
        .iter()
        .map(|p| (p.id, p.country.clone()))
        .collect();
    let rates: HashMap<String, f64> = store
        .latest_exchange_rates()
        .await?
        .into_iter()
        .map(|r| (r.currency.as_str().to_owned(), r.brl_per_unit))
        .collect();
    let origin = query
        .origin
        .and_then(|id| origins.iter().find(|p| p.id == id))
        .or_else(|| origins.first())
        .cloned();
    let routes: Vec<Route> = store
        .list_routes(true)
        .await?
        .into_iter()
        .filter(|r| origin.as_ref().is_some_and(|o| o.id == r.origin.id))
        .collect();
    let families = store.list_families(true).await?;
    let deals = store
        .list_deals(&DealFilter {
            status: None,
            unseen_only: false,
            limit: Some(500),
        })
        .await?;
    let miles = miles_rows(store, now).await?;
    let ctx = Arc::new(Pricing {
        now,
        today,
        settings,
        points_of_sale,
        home,
        pos_names,
        rates,
        routes,
        families,
        deals,
        miles,
    });

    let specs = row_specs(&ctx.routes, &ctx.families, ctx.home);
    let (cells, histories) = tokio::join!(
        inner.prefetch_cells(store, &specs),
        inner.histories(store, &specs, &ctx.settings, now),
    );
    let read_cells = cells?;
    let (histories, read_histories) = histories?;
    let mut rows = Vec::new();
    let mut detail = None;
    for spec in &specs {
        let hist = histories
            .get(&spec.key)
            .cloned()
            .context("a row without its history")?;
        let (row, d) =
            price_row(inner, store, &ctx, spec, hist, &query).await?;
        if d.is_some() {
            detail = d;
        }
        rows.push(row);
    }

    let queries = store.recent_queries(80).await?;
    let activity: Vec<ActivityRow> = queries
        .iter()
        .map(|q| activity_row(q, &ctx.pos_names))
        .collect();
    let blocks: Vec<BlockRow> = store
        .blocks_since(now - Duration::days(7))
        .await?
        .into_iter()
        .rev()
        .map(|b| BlockRow {
            at: b.at,
            site: b.site,
            detail: b.detail,
            loads_last_hour: b.loads_last_hour,
        })
        .collect();
    // The daemon's budget day is the local date (0060), so today's loads
    // and blocks are counted by it, not by the UTC date.
    let budget_day = chrono::Local::now().date_naive();
    let blocks_today = blocks
        .iter()
        .filter(|b| {
            b.at.with_timezone(&chrono::Local).date_naive() == budget_day
        })
        .count() as u32;
    let daemon = daemon_status(
        store,
        &ctx.settings,
        blocks_today,
        budget_day,
        &ctx.pos_names,
    )
    .await?;
    let sweep = queries
        .iter()
        .find(|q| q.sweep && matches!(q.outcome, Some(QueryOutcome::Ok)));
    let finds = inner.finds(store, &ctx.settings, now).await?;
    let explore = ExploreSummary {
        last_sweep: sweep.map(|q| q.finished_at.unwrap_or(q.started_at)),
        destinations: sweep.map_or(0, |q| q.cells),
        found: found::found_rows(&finds, origin.as_ref().map(|o| o.id)),
    };
    let all_routes = store.list_routes(false).await?;
    let mut feed: Vec<FeedItem> = ctx
        .deals
        .iter()
        .take(20)
        .filter_map(|d| {
            let r = all_routes.iter().find(|r| r.id == d.route)?;
            let change = d.change.map(format::change).unwrap_or_default();
            // An award deal reads in its own series (0083).
            let price = match (d.award_price(), d.series.program()) {
                (Some(a), Some(program)) => format!(
                    "{} ({})",
                    a.text(tm_domain::MilesUnit::of(program)),
                    tm_domain::program_name(program)
                ),
                _ => d.effective_brl.to_string(),
            };
            let what = if d.series.is_award() {
                "Miles deal"
            } else {
                "Deal"
            };
            let (at, text, tone) = match d.status {
                DealStatus::Confirmed => (
                    d.confirmed_at.unwrap_or(d.found_at),
                    format!(
                        "{what} confirmed: {} {price} {change}",
                        r.destination.code
                    ),
                    Tone::Deal,
                ),
                DealStatus::Candidate => (
                    d.found_at,
                    format!(
                        "{} {change}, confirming with a second query",
                        r.destination.code
                    ),
                    Tone::Deal,
                ),
                DealStatus::Rejected => (
                    d.found_at,
                    format!("{} {change} did not hold", r.destination.code),
                    Tone::Neutral,
                ),
                DealStatus::Expired => return None,
            };
            Some(FeedItem { at, text, tone })
        })
        .collect();
    feed.extend(
        queries
            .iter()
            .filter(|q| q.sweep)
            .take(3)
            .map(|q| FeedItem {
                at: q.started_at,
                text: format!(
                    "Explore sweep: {} destinations, {}",
                    q.cells,
                    q.key.cabin.label().to_lowercase()
                ),
                tone: Tone::System,
            }),
    );
    feed.extend(blocks.iter().take(3).map(|b| FeedItem {
        at: b.at,
        text: format!("Blocked on {}: {}", b.site, b.detail),
        tone: Tone::Bad,
    }));
    feed.sort_by_key(|f| std::cmp::Reverse(f.at));
    feed.truncate(8);
    let snapshot = Snapshot {
        taken_at: now,
        today,
        origin: origin.map(|o| o.id),
        origins,
        destinations,
        favorites,
        points_of_sale: ctx.points_of_sale.clone(),
        settings: ctx.settings.clone(),
        miles: ctx.miles.clone(),
        rows,
        daemon,
        activity,
        blocks,
        feed,
        explore,
        detail,
    };
    if let Some(changes) = changes {
        inner.keep(Last {
            query,
            changes,
            at: started,
            pricing: ctx,
            snapshot: snapshot.clone(),
        });
    }
    let read = Read::Rows {
        cells: read_cells,
        histories: read_histories,
    };
    Ok((snapshot, read))
}

/// The watched destinations, and which of them the user starred (0094).
async fn destinations_of(
    store: &Store,
) -> anyhow::Result<(BTreeSet<PlaceId>, Vec<Place>)> {
    let destinations = store.list_destinations(true).await?;
    let favorites = destinations
        .iter()
        .filter(|d| d.favorite)
        .map(|d| d.place.id)
        .collect();
    Ok((
        favorites,
        destinations.into_iter().map(|d| d.place).collect(),
    ))
}

/// Every miles program, with what the user set and its measured value
/// of a mile: the median of its last 30 days of award checks (0083).
async fn miles_rows(
    store: &Store,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<MilesRow>> {
    use tm_scheduler::awards::{RATE_WINDOW_DAYS, RatePoint, usual_rate};
    let window = Duration::days(RATE_WINDOW_DAYS);
    let mut out = Vec::new();
    for m in store.list_miles_programs().await? {
        let points: Vec<RatePoint> = store
            .award_rates(&m.program, now - window)
            .await?
            .into_iter()
            .map(|r| RatePoint {
                observed_at: r.observed_at,
                brl_per_1000: r.brl_per_1000,
            })
            .collect();
        let usual = usual_rate(&points, now, window);
        out.push(MilesRow {
            settings: m.settings(),
            has_reader: has_award_reader(&m.program),
            unit: tm_domain::MilesUnit::of(&m.program),
            usual_rate: usual.map(|u| u.median),
            rate_checks: usual.map_or(0, |u| u.n),
            session_until: m.session_until,
            login: match m.login_state {
                LoginState::LoggedIn => Login::LoggedIn,
                LoginState::LoggedOut => Login::LoggedOut,
                LoginState::Expired => Login::Expired,
            },
            last_checked: m.last_checked,
            program: m.program,
            name: m.name,
        });
    }
    Ok(out)
}

/// The daemon's heartbeat, each site's budget and status, and the checks
/// running now (0057, 0060, 0062).
async fn daemon_status(
    store: &Store,
    settings: &Settings,
    blocks_today: u32,
    today: NaiveDate,
    pos: &HashMap<PosId, CountryCode>,
) -> anyhow::Result<DaemonStatus> {
    let airlines: Vec<(String, String)> = store
        .list_airline_sources()
        .await?
        .into_iter()
        .map(|a| (a.code, a.name))
        .collect();
    let programs: Vec<(String, String)> = store
        .list_miles_programs()
        .await?
        .into_iter()
        .map(|m| (m.program, m.name))
        .collect();
    let label = |site: &str| site_label(site, &airlines, &programs);
    let budgets = store.site_budgets().await?;
    let statuses = store.site_statuses().await?;
    let mut sites: Vec<SiteRow> = budgets
        .iter()
        .map(|b| SiteRow {
            site: b.site.clone(),
            label: label(&b.site),
            state: String::new(),
            loads_today: b.loads_on(today),
            budget: Some(b.daily_budget),
            max_budget: Some(b.max_daily_budget),
        })
        .collect();
    let mut checks = Vec::new();
    let mut next_check_at: Option<DateTime<Utc>> = None;
    for status in &statuses {
        match sites.iter_mut().find(|s| s.site == status.site) {
            Some(row) => row.state.clone_from(&status.state),
            None => sites.push(SiteRow {
                site: status.site.clone(),
                label: label(&status.site),
                state: status.state.clone(),
                ..SiteRow::default()
            }),
        }
        if status.state == "checking" {
            // Several per site (0072).
            for c in &status.checks {
                if let Some(mut check) = current_check(store, c, pos).await? {
                    if status.site != tm_domain::GOOGLE_FLIGHTS {
                        check.site = Some(label(&status.site));
                    }
                    checks.push(check);
                }
            }
        } else if status.state == "idle"
            && let Some(at) = status.next_check_at
        {
            next_check_at = Some(next_check_at.map_or(at, |n| n.min(at)));
        }
    }
    // Google Flights first, then by site (the store's order).
    sites
        .sort_by_key(|s| (s.site != tm_domain::GOOGLE_FLIGHTS, s.site.clone()));
    let total_loads = sites.iter().map(|s| s.loads_today).sum();
    let total_budget = sites.iter().filter_map(|s| s.budget).sum();
    Ok(match store.heartbeat().await? {
        Some(h) => DaemonStatus {
            heartbeat: Some(h.updated_at),
            state: h.state,
            next_check: h.next_check,
            next_check_at: next_check_at.or(h.next_check_at),
            checks,
            sites,
            loads_today: total_loads,
            budget: total_budget,
            blocks_today,
        },
        None => DaemonStatus {
            heartbeat: None,
            state: String::new(),
            next_check: None,
            next_check_at: None,
            checks: Vec::new(),
            loads_today: total_loads,
            budget: if sites.is_empty() {
                settings.daily_budget
            } else {
                total_budget
            },
            sites,
            blocks_today,
        },
    })
}

/// A site's short name: `Google`, an airline's first word (`TAP`), a
/// miles program's name (`Smiles`).
pub(crate) fn site_label(
    site: &str,
    airlines: &[(String, String)],
    programs: &[(String, String)],
) -> String {
    match Source::from_site(site) {
        Some(Source::GoogleFlights) => "Google".to_owned(),
        Some(Source::Airline { code }) => airlines
            .iter()
            .find(|(c, _)| *c == code)
            .and_then(|(_, name)| name.split_whitespace().next())
            .map_or(code, str::to_owned),
        Some(Source::Miles { program }) => programs
            .iter()
            .find(|(p, _)| *p == program)
            .map_or(program, |(_, name)| name.clone()),
        None => site.to_owned(),
    }
}

/// The check in progress with its route, cabin, kind and point of sale
/// filled in from its family (0057).
async fn current_check(
    store: &Store,
    check: &tm_store::CheckInProgress,
    pos: &HashMap<PosId, CountryCode>,
) -> anyhow::Result<Option<CurrentCheck>> {
    let Some(family) = store.family(check.family).await? else {
        return Ok(None);
    };
    let Some(route) = store.route(family.key.route).await? else {
        return Ok(None);
    };
    let sweep = matches!(family.key.kind, FamilyKind::Explore);
    let award = matches!(family.key.source, Source::Miles { .. });
    let kind = match (&check.reason, family.key.kind) {
        (_, kind) if award => format!("award {}", award_kind(kind)),
        (QueryReason::Confirmation, _) => "confirm".to_owned(),
        (_, FamilyKind::Leg { leg: Leg::Outbound }) => "outbound".to_owned(),
        (_, FamilyKind::Leg { leg: Leg::Return }) => "return".to_owned(),
        (_, FamilyKind::RoundTrip { days }) => format!("round trip {days} d"),
        (_, FamilyKind::Explore) => "explore".to_owned(),
    };
    Ok(Some(CurrentCheck {
        query: check.query.map(|q| q.0),
        route: if sweep {
            format!("{}→anywhere", route.origin.code)
        } else {
            format!("{}→{}", route.origin.code, route.destination.code)
        },
        cabin: family.key.cabin,
        kind,
        pos: pos
            .get(&family.key.pos)
            .map_or_else(|| "?".to_owned(), |c| c.as_str().to_owned()),
        site: match &family.key.source {
            Source::GoogleFlights => None,
            other => Some(other.site()),
        },
        award,
        reason: reason_label(&check.reason),
        started_at: check.started_at,
    }))
}

fn request_kind(request: Request) -> RequestKind {
    match request {
        Request::CheckRoute(route) => RequestKind::CheckRoute { route },
        Request::ConfirmDeal(deal) => RequestKind::ConfirmDeal { deal },
        Request::CheckAward {
            route,
            cabin,
            departure,
            return_date,
        } => RequestKind::CheckAward {
            route,
            cabin,
            departure,
            return_date,
        },
        Request::OpenLogin(program) => RequestKind::OpenLogin { program },
        Request::CloseLogin(program) => RequestKind::CloseLogin {
            program: Some(program),
        },
        Request::ReloadConfig => RequestKind::ReloadConfig,
    }
}

fn to_store_place(p: &NewPlace) -> tm_store::NewPlace {
    tm_store::NewPlace {
        code: p.code.clone(),
        name: p.name.clone(),
        country: p.country.clone(),
        kind: p.kind.clone(),
        lat: p.lat,
        lon: p.lon,
    }
}

#[async_trait]
impl Backend for StoreBackend {
    async fn snapshot(&self, query: SnapshotQuery) -> anyhow::Result<Snapshot> {
        let changes = self.changes.get().map(ChangeSender::count);
        self.run(
            move |inner| async move { snapshot(&inner, query, changes).await },
        )
        .await
    }

    fn changes(&self) -> Changes {
        self.changes
            .get_or_init(|| {
                let (tx, _) = Changes::channel();
                let watcher = tx.clone();
                self.tokio.spawn(watch_changes(
                    Arc::downgrade(&self.inner),
                    self.inner.path.clone(),
                    watcher,
                ));
                tx
            })
            .subscribe()
    }

    async fn enqueue(&self, request: Request) -> anyhow::Result<Ticket> {
        self.run(move |inner| async move {
            let id =
                inner.store().await?.enqueue(&request_kind(request)).await?;
            Ok(Ticket(id.0))
        })
        .await
    }

    async fn ticket(&self, ticket: Ticket) -> anyhow::Result<TicketState> {
        self.run(move |inner| async move {
            let record = inner
                .store()
                .await?
                .request(RequestId(ticket.0))
                .await?
                .with_context(|| format!("no request {}", ticket.0))?;
            Ok(match record.status {
                RequestStatus::Pending => TicketState::Pending,
                RequestStatus::Claimed => TicketState::Running,
                RequestStatus::Done => TicketState::Done,
                RequestStatus::Failed => TicketState::Failed,
            })
        })
        .await
    }

    async fn save_settings(&self, settings: Settings) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            Ok(inner.store().await?.set_settings(&settings).await?)
        })
        .await
    }

    async fn add_place(
        &self,
        role: Role,
        place: NewPlace,
    ) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            let store = inner.store().await?;
            let new = to_store_place(&place);
            match role {
                Role::Origin => {
                    store.add_origin(&new).await?;
                }
                Role::Destination => {
                    store.add_destination(&new, 1.0).await?;
                }
            }
            store.sync_routes().await?;
            Ok(())
        })
        .await
    }

    async fn remove_place(
        &self,
        role: Role,
        place: PlaceId,
    ) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            let store = inner.store().await?;
            match role {
                Role::Origin => {
                    if store.list_origins(true).await?.len() <= 1 {
                        anyhow::bail!("at least one origin must stay");
                    }
                    store.remove_origin(place).await?
                }
                Role::Destination => store.remove_destination(place).await?,
            }
            store.sync_routes().await?;
            Ok(())
        })
        .await
    }

    async fn add_pos(&self, pos: NewPos) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            inner
                .store()
                .await?
                .add_point_of_sale(&pos.country, &pos.currency, false)
                .await?;
            Ok(())
        })
        .await
    }

    async fn set_pos_active(
        &self,
        pos: PosId,
        active: bool,
    ) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            Ok(inner
                .store()
                .await?
                .set_point_of_sale_active(pos, active)
                .await?)
        })
        .await
    }

    async fn save_program(
        &self,
        program: String,
        settings: ProgramSettings,
    ) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            Ok(inner
                .store()
                .await?
                .save_miles_settings(&program, &settings)
                .await?)
        })
        .await
    }

    async fn set_site_limits(
        &self,
        site: String,
        limits: SiteLimits,
    ) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            let now = Utc::now();
            let today = chrono::Local::now().date_naive();
            inner
                .store()
                .await?
                .configure_site_budget(&site, limits, today, now)
                .await?;
            Ok(())
        })
        .await
    }

    async fn set_favorite(
        &self,
        place: PlaceId,
        favorite: bool,
    ) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            Ok(inner
                .store()
                .await?
                .set_destination_favorite(place, favorite)
                .await?)
        })
        .await
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] is a synchronous entry point"
)]
mod tests {
    use tm_domain::RouteId;

    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tm-app-store-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    }

    /// A fresh database is seeded and reads as a snapshot with no
    /// prices; edits and requests go through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_a_fresh_database() {
        let dir = dir();
        let backend = backend(dir.join("tm.db"), Handle::current());
        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("snapshot");
        assert_eq!(s.origin_place().map(|p| p.code.as_str()), Some("SAO"));
        assert!(!s.rows.is_empty(), "the watchlist is seeded");
        assert!(
            s.rows
                .iter()
                .all(|r| r.best.is_none() && r.status == RowStatus::Watching)
        );
        assert_eq!(s.daemon.heartbeat, None);
        let key = s.rows[0].key();
        let s = backend
            .snapshot(SnapshotQuery {
                selected: Some(key),
                ..Default::default()
            })
            .await
            .expect("detail");
        assert_eq!(s.detail.as_ref().map(|d| d.key), Some(key));

        let ticket = backend
            .enqueue(Request::CheckRoute(key.route))
            .await
            .expect("enqueue");
        assert_eq!(
            backend.ticket(ticket).await.expect("state"),
            TicketState::Pending
        );

        let mut settings = s.settings.clone();
        settings.passengers.adults = 2;
        backend.save_settings(settings).await.expect("save");
        backend
            .add_place(
                Role::Destination,
                NewPlace {
                    code: "HNL".into(),
                    name: "Honolulu".into(),
                    country: CountryCode::new("US"),
                    kind: tm_domain::PlaceKind::Airport,
                    lat: 21.3,
                    lon: -157.9,
                },
            )
            .await
            .expect("add");
        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("again");
        assert_eq!(s.settings.passengers.adults, 2);
        assert!(s.rows.iter().any(|r| r.code() == "HNL"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A database with legs, a confirmed deal and its sellers reads as
    /// the designs show it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prices_deals_and_sellers() {
        use tm_domain::{Currency, FamilyKey, SellerQuote};
        let dir = dir();
        let path = dir.join("tm.db");
        let store = Store::open(&path).await.expect("open");
        store.seed_defaults().await.expect("seed");
        let now = Utc::now();
        let today = now.date_naive();
        let home = store
            .home_point_of_sale()
            .await
            .expect("pos")
            .expect("home");
        let us = store
            .list_points_of_sale(true)
            .await
            .expect("pos")
            .into_iter()
            .find(|p| p.country.as_str() == "US")
            .expect("US");
        store
            .set_exchange_rate(&Currency::new("USD"), 5.0, now)
            .await
            .expect("rate");
        let mia = store
            .route_between("SAO", "MIA")
            .await
            .expect("route")
            .expect("SAO → MIA");
        // Both legs of MIA economy, 30 to 40 days out.
        for (leg, whole) in [(Leg::Outbound, 2000), (Leg::Return, 2100)] {
            let key = FamilyKey {
                route: mia,
                cabin: Cabin::Economy,
                source: Source::GoogleFlights,
                pos: home.id,
                kind: FamilyKind::Leg { leg },
            };
            let family = store
                .family_id(&key)
                .await
                .expect("family")
                .expect("leg family");
            let q = store
                .start_query(family, &QueryReason::Bootstrap, now)
                .await
                .expect("start");
            let obs: Vec<Observation> = (30..=40)
                .map(|i| Observation {
                    id: None,
                    query: q,
                    family,
                    observed_at: now,
                    departure: today + Duration::days(i),
                    return_date: None,
                    price: Some(Money::brl_whole(whole)),
                    miles: None,
                    carrier: Some("LATAM".into()),
                    stops: Some(0),
                    duration_minutes: Some(500),
                    seller: None,
                    flights: None,
                    award: None,
                })
                .collect();
            store.insert_observations(&obs).await.expect("observations");
            store
                .finish_query(q, &QueryOutcome::Ok, 7, now)
                .await
                .expect("finish");
        }
        let mut fam = store
            .list_families(true)
            .await
            .expect("families")
            .into_iter()
            .filter(|f| f.key.route == mia && f.key.cabin == Cabin::Economy);
        for f in fam.by_ref() {
            let mut state = f.state.clone();
            state.last_check = Some(now);
            store.save_family_state(f.id, &state).await.expect("state");
        }
        // A confirmed LIS deal with two sellers.
        let lis = store
            .route_between("SAO", "LIS")
            .await
            .expect("route")
            .expect("SAO → LIS");
        let departure = today + Duration::days(35);
        let deal = Deal {
            id: None,
            route: lis,
            cabin: Cabin::Economy,
            pos: home.id,
            series: tm_domain::Series::Cash,
            leg: None,
            departure,
            return_date: Some(departure + Duration::days(7)),
            price: Money::brl_whole(3932),
            effective_brl: Money::brl_whole(3932),
            baseline: Some(Money::brl_whole(4854)),
            miles: None,
            baseline_miles: None,
            brand: None,
            seats: None,
            cash: None,
            reason: tm_domain::DealReason::BelowMedian,
            change: Some(-0.19),
            estimated: false,
            status: DealStatus::Candidate,
            found_at: now,
            confirmed_at: None,
            seen: false,
            carrier: Some("TAP".into()),
            seller: Some("Kiwi.com".into()),
            sellers: vec![
                SellerQuote {
                    pos: home.id,
                    seller: "Kiwi.com".into(),
                    is_airline: false,
                    price: Money::brl_whole(3932),
                    quoted: None,
                },
                SellerQuote {
                    pos: us.id,
                    seller: "Flightnetwork".into(),
                    is_airline: false,
                    price: Money::brl_whole(3700),
                    quoted: Some(Money::from_whole(746, Currency::new("USD"))),
                },
                SellerQuote {
                    pos: home.id,
                    seller: "TAP".into(),
                    is_airline: true,
                    price: Money::brl_whole(4122),
                    quoted: None,
                },
            ],
            url: Some(
                "https://www.google.com/travel/flights/search?tfs=x".into(),
            ),
        };
        let id = store.insert_deal(&deal).await.expect("deal");
        store
            .set_deal_status(id, DealStatus::Confirmed, now)
            .await
            .expect("confirm");
        store.close().await;

        let backend = backend(path, Handle::current());
        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("snapshot");
        let row = |code: &str| {
            s.rows
                .iter()
                .find(|r| r.code() == code && r.cabin == Cabin::Economy)
                .cloned()
                .expect("row")
        };
        let mia_row = row("MIA");
        assert_eq!(mia_row.status, RowStatus::Watching);
        let best = mia_row.best.as_ref().expect("MIA has trips");
        // No learned discount yet: the legs add up.
        assert_eq!(best.price, Money::brl_whole(4100));
        assert!(best.estimated);
        assert_eq!(best.carrier_label().as_deref(), Some("LATAM nonstop"));
        assert_eq!(mia_row.history.len(), 1);
        let lis_row = row("LIS");
        assert_eq!(lis_row.status, RowStatus::Deal);
        assert_eq!(lis_row.change, Some(-0.19));

        let s = backend
            .snapshot(SnapshotQuery {
                selected: Some(lis_row.key()),
                ..Default::default()
            })
            .await
            .expect("detail");
        let d = s.detail.expect("detail");
        assert_eq!(d.sellers.len(), 2);
        let us_row = d
            .sellers
            .iter()
            .find(|r| r.pos.as_str() == "US")
            .expect("US");
        // 746 × 5.0 × 1.035 × 1.01.
        assert_eq!(us_row.effective.whole(), 3899);
        assert_eq!(
            d.airline_direct,
            Some(("TAP".into(), Money::brl_whole(4122)))
        );
        assert_eq!(
            d.url.as_deref(),
            Some("https://www.google.com/travel/flights/search?tfs=x")
        );
        assert_eq!(d.grid.rows.len(), 5);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The daemon's writes reach the window as change notices, and its
    /// sites' statuses as the checks in progress (0057, 0062).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notices_the_daemons_writes() {
        let dir = dir();
        let path = dir.join("tm.db");
        let backend = backend(path.clone(), Handle::current());
        let mut changes = backend.changes();
        backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("creates and reads");
        // The watcher opens the file once it exists, and says so.
        let wait = |changes: &mut Changes| {
            let mut changes = changes.clone();
            async move {
                tokio::time::timeout(StdDuration::from_secs(5), changes.next())
                    .await
                    .expect("a notice in time")
            }
        };
        assert!(wait(&mut changes).await);
        changes.take();

        let daemon = Store::open(&path).await.expect("the daemon's pool");
        let lis = daemon
            .route_between("SAO", "LIS")
            .await
            .expect("route")
            .expect("SAO → LIS");
        let home = daemon.home_point_of_sale().await.expect("pos").expect("BR");
        let family = daemon
            .family_id(&tm_domain::FamilyKey {
                route: lis,
                cabin: Cabin::Economy,
                source: Source::GoogleFlights,
                pos: home.id,
                kind: FamilyKind::Leg { leg: Leg::Return },
            })
            .await
            .expect("family")
            .expect("the return leg");
        let tap_family = daemon
            .family_id(&tm_domain::FamilyKey {
                route: lis,
                cabin: Cabin::Economy,
                source: Source::Airline { code: "TP".into() },
                pos: home.id,
                kind: FamilyKind::Leg { leg: Leg::Outbound },
            })
            .await
            .expect("family")
            .expect("TAP's outbound leg");
        let started = Utc::now();
        let query = daemon
            .start_query(family, &QueryReason::Bootstrap, started)
            .await
            .expect("starts");
        let tap_query = daemon
            .start_query(tap_family, &QueryReason::Manual, started)
            .await
            .expect("starts");
        let today = chrono::Local::now().date_naive();
        daemon
            .add_site_loads("google_flights", 1234, today)
            .await
            .expect("counts");
        daemon
            .add_site_loads("airline:TP", 40, today)
            .await
            .expect("counts");
        daemon
            .write_heartbeat(&tm_store::Heartbeat {
                pid: 1,
                updated_at: started,
                state: "running".into(),
                next_check: None,
                next_check_at: None,
                loads_today: 1274,
                budget: 30_300,
            })
            .await
            .expect("beats");
        // Two sites check at once (0062); Smiles waits for a login.
        for (site, family, query, reason) in [
            ("google_flights", family, query, QueryReason::Bootstrap),
            ("airline:TP", tap_family, tap_query, QueryReason::Manual),
        ] {
            daemon
                .write_site_status(&tm_store::SiteStatus {
                    site: site.into(),
                    updated_at: started,
                    state: "checking".into(),
                    next_check_at: None,
                    checks: vec![tm_store::CheckInProgress {
                        family,
                        query: Some(query),
                        reason,
                        started_at: started,
                    }],
                })
                .await
                .expect("writes");
        }
        daemon
            .write_site_status(&tm_store::SiteStatus {
                site: "miles:smiles".into(),
                updated_at: started,
                state: "paused for login: smiles".into(),
                next_check_at: None,
                checks: Vec::new(),
            })
            .await
            .expect("writes");
        assert!(wait(&mut changes).await, "the commits were noticed");

        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("reads");
        let checks = s.daemon.checking();
        assert_eq!(checks.len(), 2);
        let check = &checks[0];
        assert_eq!(check.route, "SAO→LIS");
        assert_eq!(check.kind, "return");
        assert_eq!(check.cabin, Cabin::Economy);
        assert_eq!(check.pos, "BR");
        assert_eq!(check.site, None);
        assert_eq!(check.reason, "First check");
        assert_eq!(check.query, Some(query.0));
        assert_eq!(checks[1].site.as_deref(), Some("TAP"));
        assert_eq!(checks[1].kind, "outbound");
        assert_eq!(checks[1].query, Some(tap_query.0));
        assert!(s.activity.iter().any(|a| a.id == query.0 && a.running));
        let loads: Vec<String> =
            s.daemon.budgets().filter_map(SiteRow::loads).collect();
        assert_eq!(loads, ["Google 1,234/30,000", "TAP 40/1,200"]);
        assert_eq!((s.daemon.loads_today, s.daemon.budget), (1274, 31_200));
        let paused: Vec<&str> =
            s.daemon.paused_sites().map(|r| r.label.as_str()).collect();
        assert_eq!(paused, ["Smiles"]);
        daemon.close().await;
        drop(backend);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// "No flights" reads as a quiet result, not an error (0067); only
    /// failures are red.
    #[test]
    fn no_flights_is_a_quiet_result() {
        let at = Utc::now();
        let row = |outcome: QueryOutcome| {
            let entry = QueryLogEntry {
                id: tm_domain::QueryId(1),
                family: FamilyId(2),
                key: tm_domain::FamilyKey {
                    route: RouteId(3),
                    cabin: Cabin::Business,
                    source: Source::GoogleFlights,
                    pos: PosId(1),
                    kind: FamilyKind::Leg { leg: Leg::Outbound },
                },
                origin_code: "SAO".into(),
                destination_code: "GOH".into(),
                sweep: false,
                site: "google_flights".into(),
                reason: QueryReason::Bootstrap,
                score: None,
                runner_up: None,
                scheduled_at: at,
                started_at: at,
                finished_at: Some(at),
                outcome: Some(outcome),
                page_loads: Some(1),
                cells: 0,
            };
            let pos = HashMap::from([(PosId(1), CountryCode::new("BR"))]);
            activity_row(&entry, &pos)
        };
        let quiet = row(QueryOutcome::NoFlights);
        assert_eq!(quiet.result, "no flights");
        assert_eq!(quiet.tone, Tone::Neutral);
        assert!(!quiet.running);
        assert_eq!(quiet.loads, Some(1));
        for outcome in [
            QueryOutcome::Ok,
            QueryOutcome::NoFlights,
            QueryOutcome::Blocked { detail: "x".into() },
            QueryOutcome::Unexpected { detail: "x".into() },
            QueryOutcome::LoggedOut,
            QueryOutcome::Error { detail: "x".into() },
        ] {
            let bad = row(outcome.clone()).tone == Tone::Bad;
            assert_eq!(bad, outcome.is_failure(), "{outcome:?}");
        }
    }

    #[test]
    fn sites_have_short_labels() {
        let airlines = [("TP".to_owned(), "TAP Air Portugal".to_owned())];
        let programs = [("latam_pass".to_owned(), "LATAM Pass".to_owned())];
        let label = |site| site_label(site, &airlines, &programs);
        assert_eq!(label("google_flights"), "Google");
        assert_eq!(label("airline:TP"), "TAP");
        assert_eq!(label("airline:LA"), "LA");
        assert_eq!(label("miles:latam_pass"), "LATAM Pass");
        assert_eq!(label("miles:smiles"), "smiles");
        assert_eq!(label("bing"), "bing");
    }

    #[test]
    fn links_fall_back_to_a_text_search() {
        let place = |id, code: &str| Place {
            id: PlaceId(id),
            code: code.into(),
            name: code.into(),
            country: CountryCode::new("BR"),
            kind: tm_domain::PlaceKind::City { freebase_id: None },
            lat: 0.0,
            lon: 0.0,
        };
        let route = Route {
            id: RouteId(1),
            origin: place(1, "SAO"),
            destination: place(2, "RIO"),
            weight: 1.0,
            active: true,
            favorite: false,
        };
        let d = NaiveDate::from_ymd_opt(2026, 11, 10).expect("date");
        let url = search_url(
            &route,
            Cabin::Business,
            &Settings::default(),
            d,
            d + Duration::days(7),
        );
        assert!(url.starts_with("https://www.google.com/travel/flights?q=Flights%20from%20SAO%20to%20RIO"));
        assert!(url.contains("business%20class"));
        let mut route = route;
        route.origin.kind = tm_domain::PlaceKind::City {
            freebase_id: Some("/m/022pfm".into()),
        };
        route.destination.kind = tm_domain::PlaceKind::Airport;
        let url = search_url(
            &route,
            Cabin::Economy,
            &Settings::default(),
            d,
            d + Duration::days(7),
        );
        assert!(url.contains("/flights/search?tfs="), "{url}");
    }

    /// Prices both legs of SAO → MIA economy, 30 to 40 days out, as one
    /// query each observed at `at`, and records the check on the
    /// families only when `checked`.
    async fn price_mia(
        store: &Store,
        at: DateTime<Utc>,
        whole: i64,
        checked: bool,
    ) {
        use tm_domain::FamilyKey;
        let home = store
            .home_point_of_sale()
            .await
            .expect("pos")
            .expect("home");
        let mia = store
            .route_between("SAO", "MIA")
            .await
            .expect("route")
            .expect("SAO → MIA");
        for leg in [Leg::Outbound, Leg::Return] {
            let key = FamilyKey {
                route: mia,
                cabin: Cabin::Economy,
                source: Source::GoogleFlights,
                pos: home.id,
                kind: FamilyKind::Leg { leg },
            };
            let family = store
                .family_id(&key)
                .await
                .expect("family")
                .expect("leg family");
            let q = store
                .start_query(family, &QueryReason::Random, at)
                .await
                .expect("start");
            let obs: Vec<Observation> = (30..=40)
                .map(|i| Observation {
                    id: None,
                    query: q,
                    family,
                    observed_at: at,
                    departure: at.date_naive() + Duration::days(i),
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
            store.insert_observations(&obs).await.expect("observations");
            store
                .finish_query(q, &QueryOutcome::Ok, 1, at)
                .await
                .expect("finish");
            if checked {
                let record =
                    store.family(family).await.expect("reads").expect("family");
                let mut state = record.state.clone();
                state.last_check = Some(at);
                store
                    .save_family_state(family, &state)
                    .await
                    .expect("state");
            }
        }
    }

    /// A row's cells and history are cached until one of its families
    /// is checked again; then both are read again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_checked_row_is_read_again() {
        let dir = dir();
        let path = dir.join("tm.db");
        let store = Store::open(&path).await.expect("open");
        store.seed_defaults().await.expect("seed");
        let backend = backend(path, Handle::current());
        let mia = |s: &Snapshot| {
            let row = s
                .rows
                .iter()
                .find(|r| r.code() == "MIA" && r.cabin == Cabin::Economy)
                .expect("MIA row")
                .clone();
            (row.best.map(|t| t.price.whole()), row.history)
        };
        let now = Utc::now();
        let yesterday = (now.date_naive() - Duration::days(1))
            .and_hms_opt(12, 0, 0)
            .expect("noon")
            .and_utc();
        price_mia(&store, yesterday, 2000, true).await;
        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("reads");
        assert_eq!(mia(&s), (Some(4000), vec![400_000.0]));

        // Written but not checked yet: the cached prices stand.
        let later = yesterday + Duration::minutes(1);
        price_mia(&store, later, 1500, false).await;
        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("reads");
        assert_eq!(mia(&s), (Some(4000), vec![400_000.0]));

        // Checked today: the latest cells, and both days read again.
        price_mia(&store, now, 1000, true).await;
        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("reads");
        assert_eq!(mia(&s), (Some(2000), vec![300_000.0, 200_000.0]));
        store.close().await;
        let _ = std::fs::remove_dir_all(dir);
    }

    /// While the watcher has noticed no write, a query that only picks
    /// another row reuses the last snapshot's rows and prices just the
    /// detail; a noticed write, or the same query asked again, reads
    /// everything.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn selections_reuse_rows_until_a_write() {
        let dir = dir();
        let path = dir.join("tm.db");
        let daemon = Store::open(&path).await.expect("open");
        daemon.seed_defaults().await.expect("seed");
        let yesterday = Utc::now() - Duration::days(1);
        price_mia(&daemon, yesterday, 2000, true).await;
        let backend = backend(path, Handle::current());
        let mut changes = backend.changes();
        let notice = async |changes: &mut Changes| {
            tokio::time::timeout(StdDuration::from_secs(5), changes.next())
                .await
                .expect("a notice in time")
        };
        assert!(notice(&mut changes).await, "the watcher opened");

        let all = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("reads");
        let mia = all
            .rows
            .iter()
            .find(|r| r.code() == "MIA" && r.cabin == Cabin::Economy)
            .expect("MIA")
            .key();
        let lis = all
            .rows
            .iter()
            .find(|r| r.code() == "LIS")
            .expect("LIS")
            .key();
        let select = |key| SnapshotQuery {
            origin: all.origin,
            selected: Some(key),
            ..Default::default()
        };
        let reused = backend.snapshot(select(mia)).await.expect("reads");
        assert_eq!(reused.taken_at, all.taken_at, "the rows were reused");
        assert_eq!(reused.rows, all.rows);
        let detail = reused.detail.as_ref().expect("the detail");
        assert_eq!(detail.key, mia);
        assert_eq!(detail.trip.as_ref().map(|t| t.price.whole()), Some(4000));

        // The same query again (the fallback refresh) reads everything.
        let again = backend.snapshot(select(mia)).await.expect("reads");
        assert!(again.taken_at > all.taken_at);
        assert_eq!(again.detail, reused.detail);

        // A noticed write ends the reuse.
        price_mia(&daemon, Utc::now(), 1000, true).await;
        assert!(notice(&mut changes).await, "the write was noticed");
        let read = backend.snapshot(select(lis)).await.expect("reads");
        assert!(read.taken_at > again.taken_at);
        let row = read.row(mia).expect("MIA");
        assert_eq!(row.best.as_ref().map(|t| t.price.whole()), Some(2000));
        assert_eq!(read.detail.map(|d| d.key), Some(lis));
        daemon.close().await;
        drop(backend);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Two snapshots asked for at once (the first open, and a refresh
    /// for a write meanwhile) read the cells once: the second waits for
    /// the first and finds them cached.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn snapshots_take_turns() {
        let dir = dir();
        let path = dir.join("tm.db");
        let daemon = Store::open(&path).await.expect("open");
        daemon.seed_defaults().await.expect("seed");
        price_mia(&daemon, Utc::now() - Duration::hours(1), 2000, true).await;
        let backend = Arc::new(StoreBackend::new(path, Handle::current()));
        let (a, b) = tokio::join!(
            backend.snapshot(SnapshotQuery::default()),
            backend.snapshot(SnapshotQuery::default()),
        );
        let (a, b) = (a.expect("reads"), b.expect("reads"));
        assert_eq!(a.rows, b.rows);
        assert_eq!(backend.inner.most_at_once.load(Ordering::Relaxed), 1);
        assert_eq!(backend.inner.cold_reads.load(Ordering::Relaxed), 1);

        // A check makes the next one read again.
        price_mia(&daemon, Utc::now(), 1000, true).await;
        backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("reads");
        assert_eq!(backend.inner.cold_reads.load(Ordering::Relaxed), 2);
        daemon.close().await;
        drop(backend);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Times snapshots against a copy of a real database:
    /// `TM_BENCH_DB=<copy> cargo test --release -p tm-app --bin tm --
    /// --ignored bench_snapshot --nocapture`. Never point it at the live
    /// file: opening it runs the migrations.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs TM_BENCH_DB, a copy of a real database"]
    async fn bench_snapshot() {
        let Some(path) = std::env::var_os("TM_BENCH_DB") else {
            return;
        };
        let watched = backend(PathBuf::from(&path), Handle::current());
        let full = backend(PathBuf::from(&path), Handle::current());
        let backend = watched;
        let time = async |label: &str, query: SnapshotQuery| {
            let started = std::time::Instant::now();
            let s = backend.snapshot(query).await.expect("snapshot");
            eprintln!(
                "{label}: {:?} ({} rows)",
                started.elapsed(),
                s.rows.len()
            );
            s
        };
        // The first open with a refresh asked for meanwhile.
        let twice = super::backend(PathBuf::from(&path), Handle::current());
        let started = std::time::Instant::now();
        let (a, b) = tokio::join!(
            twice.snapshot(SnapshotQuery::default()),
            twice.snapshot(SnapshotQuery::default()),
        );
        a.and(b).expect("snapshots");
        eprintln!("two at once, cold: {:?}", started.elapsed());
        let s = time("cold", SnapshotQuery::default()).await;
        for _ in 0..3 {
            time("warm", SnapshotQuery::default()).await;
        }
        let select = |row: &RouteRow| SnapshotQuery {
            selected: Some(row.key()),
            ..Default::default()
        };
        for row in s.rows.iter().take(3) {
            time("selected, not watching", select(row)).await;
        }
        // Watching: selections reuse the rows until a write is noticed.
        let mut changes = backend.changes();
        assert!(changes.next().await, "the watcher opened");
        time("warm, watching", SnapshotQuery::default()).await;
        for row in s.rows.iter().skip(5).step_by(50) {
            let fast = time("selected, watching", select(row)).await;
            let read = full.snapshot(select(row)).await.expect("snapshot");
            assert_eq!(fast.detail, read.detail);
            assert_eq!(fast.rows, read.rows);
        }
    }
}
