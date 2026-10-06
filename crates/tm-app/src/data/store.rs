//! The daemon's SQLite file, read through `tm-store` (DECISIONS.md
//! 0020, 0040).
//!
//! sqlx runs on tokio, so every call is spawned on the app's tokio
//! runtime and only its result is awaited by the interface. The store
//! is opened on first use: a missing or locked file shows as an error
//! in the window, retried with every refresh. Leg and history reads are
//! cached per family and only re-read when the family was checked
//! again.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use tm_domain::{
    Cabin,
    CountryCode,
    Deal,
    DealId,
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
    QueryOutcome,
    QueryReason,
    RequestId,
    Route,
    Settings,
    Source,
    effective_brl,
};
use tm_store::{
    DealFilter,
    FamilyRecord,
    LoginState,
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
    DaemonStatus,
    ExploreSummary,
    FeedItem,
    Login,
    MilesRow,
    NewPlace,
    NewPos,
    Notice,
    Request,
    Role,
    RouteRow,
    RowKey,
    RowStatus,
    SellerRow,
    SharedBackend,
    Snapshot,
    SnapshotQuery,
    Ticket,
    TicketState,
    Tone,
    Trip,
    detail::{Baselines, DetailInputs, assemble},
    history::{self, History},
};
use crate::{format, grid::TripInputs};

/// A cached read and the last check it reflects.
type Stamped<T> = (Option<DateTime<Utc>>, Arc<T>);

/// Cached reads, keyed by when the family (or route) was last checked.
#[derive(Default)]
struct Cache {
    cells: HashMap<FamilyId, Stamped<Vec<Observation>>>,
    history: HashMap<RowKey, Stamped<History>>,
}

struct Inner {
    path: PathBuf,
    store: OnceCell<Store>,
    cache: Mutex<Cache>,
}

pub struct StoreBackend {
    inner: Arc<Inner>,
    tokio: Handle,
}

/// The backend for the database at `path`; nothing is opened yet.
pub fn backend(path: PathBuf, tokio: Handle) -> SharedBackend {
    Arc::new(StoreBackend {
        inner: Arc::new(Inner {
            path,
            store: OnceCell::new(),
            cache: Mutex::new(Cache::default()),
        }),
        tokio,
    })
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
                let store =
                    Store::open(&self.path).await.with_context(|| {
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

    async fn history(
        &self,
        store: &Store,
        key: RowKey,
        families: &[&FamilyRecord],
        discount: f64,
        settings: &Settings,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Arc<History>> {
        let stamp = families.iter().filter_map(|f| f.state.last_check).max();
        if let Some((at, h)) = self.lock().history.get(&key)
            && *at == stamp
        {
            return Ok(h.clone());
        }
        let kinds: HashMap<FamilyId, FamilyKind> =
            families.iter().map(|f| (f.id, f.key.kind)).collect();
        let rows: Vec<(FamilyKind, Observation)> = store
            .history_for_route(key.route, key.cabin, now - Duration::days(60))
            .await?
            .into_iter()
            .filter_map(|o| Some((*kinds.get(&o.family)?, o)))
            .collect();
        let h = Arc::new(history::history(&rows, discount, settings.trips));
        self.lock().history.insert(key, (stamp, h.clone()));
        Ok(h)
    }
}

/// The families a row's prices come from: Google Flights at the home
/// point of sale, legs and round trips.
fn row_families(
    families: &[FamilyRecord],
    key: RowKey,
    home: Option<PosId>,
) -> Vec<&FamilyRecord> {
    families
        .iter()
        .filter(|f| {
            f.key.route == key.route
                && f.key.cabin == key.cabin
                && f.key.source == Source::GoogleFlights
                && home.is_none_or(|h| f.key.pos == h)
                && !matches!(f.key.kind, FamilyKind::Explore)
        })
        .collect()
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

fn activity_row(
    q: &QueryLogEntry,
    pos: &HashMap<PosId, CountryCode>,
) -> ActivityRow {
    let source = match &q.key.source {
        Source::GoogleFlights => String::new(),
        other => format!(" · {}", other.site()),
    };
    let pos = pos.get(&q.key.pos).map_or("?", CountryCode::as_str);
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
        Some(QueryOutcome::LoggedOut) => ("logged out".to_owned(), Tone::Bad),
        Some(QueryOutcome::Error { detail }) => {
            (format!("error: {detail}"), Tone::Bad)
        }
    };
    ActivityRow {
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

fn notice(deal: &Deal, routes: &[Route]) -> Option<Notice> {
    let route = routes.iter().find(|r| r.id == deal.route)?;
    Some(Notice {
        deal: deal.id?,
        title: format!(
            "{} → {}: {}{}",
            route.origin.code,
            route.destination.code,
            deal.effective_brl,
            deal.change
                .map(|c| format!(" ({})", format::change(c)))
                .unwrap_or_default()
        ),
        body: [
            Some(route.destination.name.clone()),
            Some(deal.cabin.label().to_owned()),
            Some(format::date_range(deal.departure, deal.return_date)),
            deal.carrier.clone(),
            deal.seller.clone().map(|s| format!("via {s}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
    })
}

async fn snapshot(
    inner: &Inner,
    query: SnapshotQuery,
) -> anyhow::Result<Snapshot> {
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
    let destinations: Vec<Place> = store
        .list_destinations(true)
        .await?
        .into_iter()
        .map(|d| d.place)
        .collect();
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
    let horizon_end = today + Duration::days(i64::from(settings.horizon_days));

    let mut rows = Vec::new();
    let mut detail = None;
    for route in &routes {
        let discount = discount_of(store.route_trip_state(route.id).await?);
        for &cabin in route.cabins() {
            let key = RowKey {
                route: route.id,
                cabin,
            };
            let fams = row_families(&families, key, home);
            let mut cells: Vec<(FamilyKind, Arc<Vec<Observation>>)> =
                Vec::new();
            for f in &fams {
                cells.push((f.key.kind, inner.cells(store, f).await?));
            }
            let inputs: TripInputs = history::trip_inputs(
                cells
                    .iter()
                    .flat_map(|(k, obs)| obs.iter().map(move |o| (*k, o))),
                discount,
            );
            let hist = inner
                .history(store, key, &fams, discount, &settings, now)
                .await?;
            let baselines = Baselines {
                by_departure: history::baselines_by_departure(
                    &hist.points,
                    now,
                    settings.horizon_days,
                ),
            };
            // The newest live deal of this row: confirmed, or waiting.
            let deal = deals.iter().find(|d| {
                d.route == route.id
                    && d.cabin == cabin
                    && d.departure >= today
                    && matches!(
                        d.status,
                        DealStatus::Confirmed | DealStatus::Candidate
                    )
            });
            let carrier_of = |dep: NaiveDate| {
                cells
                    .iter()
                    .flat_map(|(_, obs)| obs.iter())
                    .find(|o| o.departure == dep && o.carrier.is_some())
            };
            let best = match deal {
                Some(d) => Some(Trip {
                    departure: d.departure,
                    return_date: d.return_date,
                    price: d.effective_brl.clone(),
                    carrier: d.carrier.clone(),
                    stops: carrier_of(d.departure).and_then(|o| o.stops),
                    duration_minutes: carrier_of(d.departure)
                        .and_then(|o| o.duration_minutes),
                    estimated: d.estimated,
                }),
                None => history::best_trip(
                    &inputs,
                    today,
                    horizon_end,
                    settings.trips,
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
            let row = RouteRow {
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
                last_checked: fams
                    .iter()
                    .filter_map(|f| f.state.last_check)
                    .max(),
                deal: deal.and_then(|d| d.id),
                found_at: deal.map(|d| d.confirmed_at.unwrap_or(d.found_at)),
            };
            if query.selected == Some(key) {
                let (sellers, airline_direct) = deal
                    .map(|d| seller_rows(d, &points_of_sale, &rates, &settings))
                    .unwrap_or_default();
                let url = deal.and_then(|d| d.url.clone()).or_else(|| {
                    best.as_ref().map(|t| {
                        search_url(
                            route,
                            cabin,
                            &settings,
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
                        matches!(
                            f.key.kind,
                            FamilyKind::Leg { leg: Leg::Outbound }
                        )
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
                        .filter(|q| {
                            q.key.route == route.id && q.key.cabin == cabin
                        })
                        .take(8)
                        .map(|q| activity_row(q, &pos_names))
                        .collect(),
                    next_check: None,
                };
                let days = query
                    .calendar_days
                    .unwrap_or(settings.trips.round_trip_days);
                detail = Some(assemble(
                    key,
                    inputs,
                    &settings,
                    today,
                    days,
                    query.departure,
                ));
            }
            rows.push(row);
        }
    }

    let queries = store.recent_queries(80).await?;
    let activity: Vec<ActivityRow> = queries
        .iter()
        .map(|q| activity_row(q, &pos_names))
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
    let blocks_today =
        blocks.iter().filter(|b| b.at.date_naive() == today).count() as u32;
    let budget = store.budget().await?;
    let daemon = match store.heartbeat().await? {
        Some(h) => DaemonStatus {
            heartbeat: Some(h.updated_at),
            state: h.state,
            next_check: h.next_check,
            loads_today: h.loads_today,
            budget: h.budget,
            blocks_today,
        },
        None => DaemonStatus {
            heartbeat: None,
            state: String::new(),
            next_check: None,
            loads_today: budget
                .filter(|b| b.day == today)
                .map_or(0, |b| b.loads_today),
            budget: budget.map_or(settings.daily_budget, |b| b.daily_budget),
            blocks_today,
        },
    };
    let sweep = queries
        .iter()
        .find(|q| q.sweep && matches!(q.outcome, Some(QueryOutcome::Ok)));
    let explore = ExploreSummary {
        last_sweep: sweep.map(|q| q.finished_at.unwrap_or(q.started_at)),
        destinations: sweep.map_or(0, |q| q.cells),
    };
    let all_routes = store.list_routes(false).await?;
    let mut feed: Vec<FeedItem> = deals
        .iter()
        .take(20)
        .filter_map(|d| {
            let r = all_routes.iter().find(|r| r.id == d.route)?;
            let change = d.change.map(format::change).unwrap_or_default();
            let (at, text, tone) = match d.status {
                DealStatus::Confirmed => (
                    d.confirmed_at.unwrap_or(d.found_at),
                    format!(
                        "Deal confirmed: {} {} {change}",
                        r.destination.code, d.effective_brl
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
    let notices = store
        .unseen_confirmed()
        .await?
        .iter()
        .filter_map(|d| notice(d, &all_routes))
        .collect();
    let miles = store
        .list_miles_programs()
        .await?
        .into_iter()
        .filter(|m| m.active)
        .map(|m| MilesRow {
            program: m.program,
            name: m.name,
            login: match m.login_state {
                LoginState::LoggedIn => Login::LoggedIn,
                LoginState::LoggedOut => Login::LoggedOut,
                LoginState::Expired => Login::Expired,
            },
            last_checked: m.last_checked,
        })
        .collect();
    Ok(Snapshot {
        taken_at: now,
        today,
        origin: origin.map(|o| o.id),
        origins,
        destinations,
        points_of_sale,
        settings,
        miles,
        rows,
        daemon,
        activity,
        blocks,
        feed,
        explore,
        notices,
        detail,
    })
}

fn request_kind(request: Request) -> RequestKind {
    match request {
        Request::CheckRoute(route) => RequestKind::CheckRoute { route },
        Request::ConfirmDeal(deal) => RequestKind::ConfirmDeal { deal },
        Request::OpenLogin(program) => RequestKind::OpenLogin { program },
        Request::CloseLogin => RequestKind::CloseLogin,
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
        self.run(move |inner| async move { snapshot(&inner, query).await })
            .await
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

    async fn mark_seen(&self, deals: Vec<DealId>) -> anyhow::Result<()> {
        self.run(move |inner| async move {
            let store = inner.store().await?;
            for deal in deals {
                store.mark_deal_seen(deal).await?;
            }
            Ok(())
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
                })
                .collect();
            store.insert_observations(&obs).await.expect("observations");
            store
                .finish_query(q, &QueryOutcome::Ok, 7, 0, now)
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
            departure,
            return_date: departure + Duration::days(7),
            price: Money::brl_whole(3932),
            effective_brl: Money::brl_whole(3932),
            baseline: Some(Money::brl_whole(4854)),
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
        assert_eq!(s.notices.len(), 1);
        assert!(s.notices[0].title.starts_with("SAO → LIS: R$3,932"));

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

        backend.mark_seen(vec![DealId(id.0)]).await.expect("seen");
        let s = backend
            .snapshot(SnapshotQuery::default())
            .await
            .expect("after");
        assert!(s.notices.is_empty());
        let _ = std::fs::remove_dir_all(dir);
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
}
