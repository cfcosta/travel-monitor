//! The real sources, in the daemon's own Chromium (DECISIONS.md 0021).
//!
//! - Google Flights price graphs and Explore: load the generated URL,
//!   open the graph with `tm-gflights`' snippet, and read the page's own
//!   `GetCalendarGraph` / `GetExploreDestinations` responses through
//!   CDP (0032). When a snippet finds nothing and Jev is configured, Jev
//!   is asked to open the graph, then the capture is tried once more
//!   (0041).
//! - Confirmations: the round trip's results page (first itineraries
//!   from the HTML-embedded `ds:1`), the returning flights for the
//!   cheapest outbound, and that point of sale's own booking page (0031,
//!   0034).
//! - Airline sites and miles programs: Jev drives the page to a results
//!   list; prices are read from the visible text and marked unverified
//!   (0047).
//!
//! Each point of sale has its own browser context (isolated cookies,
//! 0033); airline and miles pages use the profile's own context, where
//! miles logins live (0024).

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use rand::Rng;
use serde_json::Value;
use tm_browser::{
    Chromium,
    ChromiumConfig,
    Context,
    ContextOptions,
    Page,
    agent::{self, AgentOutcome, Task},
};
use tm_domain::{Currency, PointOfSale, PosId, QueryOutcome};
use tm_gflights::{
    Method,
    page::{
        Block,
        DATA_SERVICE_REQUESTS_JS,
        OPEN_PRICE_GRAPH_JS,
        PAGE_TEXT_JS,
        SCROLL_FORWARD_JS,
        detect_block,
        init_data_js,
    },
    plan::reaches,
    rpc::{
        CalendarCell,
        ExploreDestination,
        Itinerary,
        parse_booking_results,
        parse_calendar,
        parse_calendar_request,
        parse_data_service_requests,
        parse_explore,
        parse_shopping_results,
        shopping_results_from_init_data,
    },
    tfs::PlaceRef,
    url::{booking_url, search_url},
};
use tm_jev::Jev;
use tracing::{debug, info, warn};

use super::{
    AgentJob,
    CalendarJob,
    Cell,
    ConfirmJob,
    Confirmation,
    ExploreJob,
    Fetch,
    ResolveJob,
    Sources,
};
use crate::{extract, jobs::rebase};

/// How long each step may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub navigation: Duration,
    /// Until the "Price graph" control shows up.
    pub open_graph: Duration,
    /// Until a captured RPC response arrives.
    pub response: Duration,
    /// After a response, for streamed or follow-up calls.
    pub settle: Duration,
    /// Between presses of "Scroll forward", random up to twice this.
    pub scroll_pause: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            navigation: Duration::from_secs(45),
            open_graph: Duration::from_secs(25),
            response: Duration::from_secs(25),
            settle: Duration::from_millis(1500),
            scroll_pause: Duration::from_millis(700),
        }
    }
}

/// Clicks Google's cookie consent away, if it is showing.
const CONSENT_JS: &str = r#"(() => {
  const el = [...document.querySelectorAll('button, [role="button"], input[type="submit"]')]
    .find(e => /^(Reject all|Accept all|Rejeitar tudo|Aceitar tudo)$/i.test((e.innerText || e.value || '').trim()));
  if (!el) return false;
  el.click();
  return true;
})()"#;

struct Running {
    chromium: Chromium,
    contexts: HashMap<PosId, Context>,
}

pub struct BrowserSources {
    config: ChromiumConfig,
    jev: Option<Arc<dyn Jev>>,
    timeouts: Timeouts,
    running: Option<Running>,
}

impl BrowserSources {
    pub fn new(config: ChromiumConfig, jev: Option<Arc<dyn Jev>>) -> Self {
        Self {
            config,
            jev,
            timeouts: Timeouts::default(),
            running: None,
        }
    }

    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// A new page under `pos`'s context (created if missing), or in the
    /// profile's own context.
    async fn page(&mut self, pos: Option<PosId>) -> anyhow::Result<Page> {
        let running = self
            .running
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the browser is not running"))?;
        let Some(pos) = pos else {
            return Ok(running.chromium.browser().new_page().await?);
        };
        if !running.contexts.contains_key(&pos) {
            let context = running
                .chromium
                .browser()
                .new_context(ContextOptions::default())
                .await?;
            running.contexts.insert(pos, context);
        }
        let context = running
            .contexts
            .get(&pos)
            .ok_or_else(|| anyhow::anyhow!("no context for {pos}"))?;
        Ok(context.new_page().await?)
    }
}

/// Whether the page is a block page. A consent interstitial is answered
/// once; if it stays, the check is `Unexpected`.
async fn blocked(page: &Page) -> Option<QueryOutcome> {
    let url = page.url().await.unwrap_or_default();
    let text: String = page.eval_as(PAGE_TEXT_JS).await.unwrap_or_default();
    match detect_block(&url, &text)? {
        Block::Consent => {
            info!("answering the cookie consent page");
            let clicked: bool = page.eval_as(CONSENT_JS).await.unwrap_or(false);
            tokio::time::sleep(Duration::from_secs(4)).await;
            let url = page.url().await.unwrap_or_default();
            let text: String =
                page.eval_as(PAGE_TEXT_JS).await.unwrap_or_default();
            match detect_block(&url, &text) {
                None => None,
                Some(Block::Consent) => Some(QueryOutcome::Unexpected {
                    detail: format!(
                        "cookie consent page ({})",
                        if clicked {
                            "did not go away"
                        } else {
                            "no button found"
                        }
                    ),
                }),
                Some(other) => Some(other.outcome()),
            }
        }
        block => Some(block.outcome()),
    }
}

async fn navigate(
    page: &Page,
    url: &str,
    timeout: Duration,
) -> Result<(), QueryOutcome> {
    debug!(%url, "navigating");
    page.navigate(url, timeout)
        .await
        .map_err(|e| QueryOutcome::Error {
            detail: format!("navigation failed: {e}"),
        })
}

/// What Jev is asked when a price-graph snippet finds nothing (0041).
fn open_graph_task() -> Task {
    let mut task = Task::new(
        "Open the price graph of this Google Flights results page: the \
         control labelled 'Price graph', next to 'Date grid', near the top \
         of the results. Stop as soon as the price graph chart is visible. \
         Do not select a flight and do not change the search.",
    );
    task.max_steps = 8;
    task
}

fn jev_calls(outcome: &AgentOutcome) -> u32 {
    u32::try_from(outcome.trace.decisions.len()).unwrap_or(u32::MAX)
}

/// Navigations an agent run caused: steps after which the URL changed.
fn agent_loads(outcome: &AgentOutcome) -> u32 {
    let mut urls: Vec<&str> =
        outcome.trace.steps.iter().map(|s| s.url.as_str()).collect();
    if let Some(last) = outcome.trace.final_url.as_deref() {
        urls.push(last);
    }
    let changes = urls.windows(2).filter(|w| w[0] != w[1]).count();
    u32::try_from(changes).unwrap_or(u32::MAX)
}

fn cells_of(calendar: Vec<CalendarCell>, display: &Currency) -> Vec<Cell> {
    calendar
        .into_iter()
        .map(|c| Cell::priced(c.departure, c.return_date, c.money(display)))
        .collect()
}

/// Merges calendar cells, the latest response winning per date pair.
fn merge(into: &mut Vec<CalendarCell>, cells: Vec<CalendarCell>) {
    for c in cells {
        match into.iter_mut().find(|x| {
            x.departure == c.departure && x.return_date == c.return_date
        }) {
            Some(existing) => *existing = c,
            None => into.push(c),
        }
    }
}

async fn calendar_on(
    page: &Page,
    job: &CalendarJob,
    jev: Option<&dyn Jev>,
    t: Timeouts,
) -> Fetch<Vec<Cell>> {
    let capture = match page.capture(Method::CalendarGraph.name()).await {
        Ok(c) => c,
        Err(e) => return Fetch::error(format!("capture failed: {e}")),
    };
    let mut loads = 1;
    let mut jev_used = 0;
    if let Err(outcome) = navigate(page, &job.url, t.navigation).await {
        return Fetch::failed(outcome, loads, 0);
    }
    if let Some(outcome) = blocked(page).await {
        return Fetch::failed(outcome, loads, 0);
    }
    let opened = page
        .wait_until(
            OPEN_PRICE_GRAPH_JS,
            t.open_graph,
            Duration::from_millis(500),
        )
        .await
        .is_ok();
    let mut got = opened && capture.wait_for(1, t.response).await.is_ok();
    if !got {
        let why = if opened {
            "no GetCalendarGraph response after opening the price graph"
        } else {
            "the 'Price graph' control was not found"
        };
        let text: String = page.eval_as(PAGE_TEXT_JS).await.unwrap_or_default();
        let excerpt: String =
            text.split_whitespace().collect::<Vec<_>>().join(" ");
        let excerpt: String = excerpt.chars().take(400).collect();
        let url = page.url().await.unwrap_or_default();
        warn!(%url, page = %excerpt, "{why}");
        let Some(jev) = jev else {
            return Fetch::failed(
                QueryOutcome::Unexpected {
                    detail: why.to_owned(),
                },
                loads,
                0,
            );
        };
        warn!(url = %job.url, "{why}; asking Jev");
        let outcome = agent::run(page, jev, &open_graph_task()).await;
        jev_used += jev_calls(&outcome);
        loads += agent_loads(&outcome);
        got = capture.wait_for(1, t.response).await.is_ok();
        if !got {
            return Fetch::failed(
                QueryOutcome::Unexpected {
                    detail: format!(
                        "{why}; Jev recovery ended {:?} ({})",
                        outcome.status,
                        outcome.reason.unwrap_or_default()
                    ),
                },
                loads,
                jev_used,
            );
        }
    }
    tokio::time::sleep(t.settle).await;
    let mut responses = capture.drain();
    let mut pages = 1;
    let requests = |rs: &[tm_browser::Captured]| {
        rs.iter()
            .filter_map(|c| c.post_data.as_deref())
            .filter_map(|f| parse_calendar_request(f).ok())
            .collect::<Vec<_>>()
    };
    while pages < job.max_pages
        && !reaches(&requests(&responses), job.horizon_end)
    {
        let pause = t.scroll_pause.as_millis() as u64;
        let jitter = rand::rng()
            .random_range(pause..=pause.saturating_mul(2).max(pause));
        tokio::time::sleep(Duration::from_millis(jitter)).await;
        let pressed: bool =
            page.eval_as(SCROLL_FORWARD_JS).await.unwrap_or(false);
        if !pressed {
            debug!("no 'Scroll forward' control: the graph ends here");
            break;
        }
        if capture.wait_for(1, t.response).await.is_err() {
            warn!("no response after 'Scroll forward'");
            break;
        }
        tokio::time::sleep(t.settle / 2).await;
        responses.extend(capture.drain());
        pages += 1;
    }
    loads += pages;
    let mut cells = Vec::new();
    let mut errors = Vec::new();
    let mut parsed = 0;
    for r in &responses {
        if !r.is_ok() {
            errors.push(format!(
                "status {:?} {}",
                r.status,
                r.error.as_deref().unwrap_or("")
            ));
            continue;
        }
        match parse_calendar(&r.text()) {
            Ok(c) => {
                parsed += 1;
                merge(&mut cells, c);
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    if parsed == 0 {
        return Fetch::failed(
            QueryOutcome::Unexpected {
                detail: format!(
                    "no readable GetCalendarGraph response: {}",
                    errors.join("; ")
                ),
            },
            loads,
            jev_used,
        );
    }
    if !errors.is_empty() {
        warn!(?errors, "some calendar responses were not read");
    }
    info!(cells = cells.len(), pages, "price graph read");
    Fetch::ok(cells_of(cells, &job.display), loads, jev_used)
}

async fn explore_on(
    page: &Page,
    job: &ExploreJob,
    t: Timeouts,
) -> Fetch<Vec<ExploreDestination>> {
    let capture = match page.capture(Method::ExploreDestinations.name()).await {
        Ok(c) => c,
        Err(e) => return Fetch::error(format!("capture failed: {e}")),
    };
    if let Err(outcome) = navigate(page, &job.url, t.navigation).await {
        return Fetch::failed(outcome, 1, 0);
    }
    if let Some(outcome) = blocked(page).await {
        return Fetch::failed(outcome, 1, 0);
    }
    if capture.wait_for(1, t.response * 2).await.is_err() {
        return Fetch::failed(
            QueryOutcome::Unexpected {
                detail: "no GetExploreDestinations response".into(),
            },
            1,
            0,
        );
    }
    tokio::time::sleep(t.settle).await;
    let mut out: Vec<ExploreDestination> = Vec::new();
    let mut errors = Vec::new();
    for r in capture.drain().iter().filter(|r| r.is_ok()) {
        match parse_explore(&r.text()) {
            Ok(list) => {
                for d in list {
                    match out
                        .iter_mut()
                        .find(|x| x.freebase_id == d.freebase_id)
                    {
                        Some(x) => *x = d,
                        None => out.push(d),
                    }
                }
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    if out.is_empty() && !errors.is_empty() {
        return Fetch::failed(
            QueryOutcome::Unexpected {
                detail: format!(
                    "unreadable Explore response: {}",
                    errors.join("; ")
                ),
            },
            1,
            0,
        );
    }
    info!(destinations = out.len(), "Explore read");
    Fetch::ok(out, 1, 0)
}

async fn agent_on(
    page: &Page,
    job: &AgentJob,
    jev: &dyn Jev,
    t: Timeouts,
) -> Fetch<Vec<Cell>> {
    if let Err(outcome) = navigate(page, &job.start_url, t.navigation).await {
        return Fetch::failed(outcome, 1, 0);
    }
    let outcome = agent::run(page, jev, &job.task).await;
    let calls = jev_calls(&outcome);
    let loads = 1 + agent_loads(&outcome);
    let text: String = page.eval_as(PAGE_TEXT_JS).await.unwrap_or_default();
    let password: bool = page
        .eval_as(extract::PASSWORD_FIELD_JS)
        .await
        .unwrap_or(false);
    let prices = extract::brl_amounts(&text);
    let miles = extract::mile_amounts(&text);
    let found = if job.miles {
        !miles.is_empty()
    } else {
        !prices.is_empty()
    };
    if job.miles && !found && extract::looks_logged_out(&text, password) {
        return Fetch::failed(QueryOutcome::LoggedOut, loads, calls);
    }
    if !found {
        return Fetch::failed(
            QueryOutcome::Unexpected {
                detail: format!(
                    "no prices visible on {} (Jev ended {:?}: {})",
                    job.site,
                    outcome.status,
                    outcome.reason.unwrap_or_default()
                ),
            },
            loads,
            calls,
        );
    }
    let price = prices
        .iter()
        .min()
        .map(|&minor| tm_domain::Money::new(minor, Currency::brl()));
    let cell = Cell {
        departure: job.departure,
        return_date: job.return_date,
        price,
        miles: if job.miles {
            miles.iter().min().copied()
        } else {
            None
        },
        carrier: job.carrier.clone(),
        stops: None,
        duration_minutes: None,
        seller: Some(format!("unverified:{}", job.site)),
    };
    info!(site = %job.site, ?cell.price, ?cell.miles, "unverified price read");
    Fetch::ok(vec![cell], loads, calls)
}

fn cheapest(itineraries: &[Itinerary]) -> Option<&Itinerary> {
    itineraries
        .iter()
        .filter(|i| i.price.is_some())
        .min_by_key(|i| i.price)
}

async fn confirm_on(
    page: &Page,
    job: &ConfirmJob,
    t: Timeouts,
) -> Fetch<Option<Confirmation>> {
    let origin = job.origin_override.as_deref();
    let first_url = rebase(&search_url(&job.search, &job.locale), origin);
    if let Err(outcome) = navigate(page, &first_url, t.navigation).await {
        return Fetch::failed(outcome, 1, 0);
    }
    if let Some(outcome) = blocked(page).await {
        return Fetch::failed(outcome, 1, 0);
    }
    let script = match page
        .wait_until(
            &init_data_js("ds:1"),
            t.open_graph,
            Duration::from_millis(500),
        )
        .await
    {
        Ok(Value::String(s)) => s,
        _ => {
            return Fetch::failed(
                QueryOutcome::Unexpected {
                    detail: "the results page has no ds:1 data".into(),
                },
                1,
                0,
            );
        }
    };
    let outbound = match shopping_results_from_init_data(&script) {
        Ok(list) => list,
        Err(e) => {
            return Fetch::failed(
                QueryOutcome::Unexpected {
                    detail: format!("unreadable ds:1: {e}"),
                },
                1,
                0,
            );
        }
    };
    let mut confirmation = Confirmation {
        search_url: first_url,
        ..Confirmation::default()
    };
    let Some(best) = cheapest(&outbound).cloned() else {
        info!("the confirmation found no priced itinerary");
        return Fetch::ok(Some(confirmation), 1, 0);
    };
    confirmation.cheapest = best.price;
    confirmation.carrier = Some(best.carrier.clone());
    let mut loads = 1;
    // The returning flights for the cheapest outbound, then that point
    // of sale's own booking page (its `tfu`, 0034).
    let booking = async {
        let mut search = job.search.clone();
        search.legs.first_mut()?.flights = best.flight_specs();
        let capture =
            page.capture(Method::ShoppingResults.name()).await.ok()?;
        let url = rebase(&search_url(&search, &job.locale), origin);
        loads += 1;
        navigate(page, &url, t.navigation).await.ok()?;
        if blocked(page).await.is_some() {
            return None;
        }
        capture.wait_for(1, t.response).await.ok()?;
        tokio::time::sleep(t.settle).await;
        let returning = capture
            .drain()
            .iter()
            .filter(|r| r.is_ok())
            .filter_map(|r| parse_shopping_results(&r.text()).ok())
            .rfind(|l| !l.is_empty())?;
        let back = cheapest(&returning)?.clone();
        search.legs.get_mut(1)?.flights = back.flight_specs();
        let url = rebase(
            &booking_url(&search, back.token.as_deref(), &job.locale),
            origin,
        );
        let capture = page.capture(Method::BookingResults.name()).await.ok()?;
        loads += 1;
        navigate(page, &url, t.navigation).await.ok()?;
        capture.wait_for(1, t.response).await.ok()?;
        tokio::time::sleep(t.settle).await;
        let offers = capture
            .drain()
            .iter()
            .filter(|r| r.is_ok())
            .filter_map(|r| parse_booking_results(&r.text()).ok())
            .next_back()?;
        Some((url, offers))
    }
    .await;
    match booking {
        Some((url, offers)) => {
            info!(sellers = offers.len(), "booking page read");
            confirmation.booking_url = Some(url);
            confirmation.offers = offers;
        }
        None => warn!(
            "the booking page could not be read; keeping the results price"
        ),
    }
    Fetch::ok(Some(confirmation), loads, 0)
}

async fn resolve_on(
    page: &Page,
    job: &ResolveJob,
    t: Timeouts,
) -> Fetch<Option<String>> {
    if let Err(outcome) = navigate(page, &job.url, t.navigation).await {
        return Fetch::failed(outcome, 1, 0);
    }
    if let Some(outcome) = blocked(page).await {
        return Fetch::failed(outcome, 1, 0);
    }
    let script = match page
        .wait_until(
            DATA_SERVICE_REQUESTS_JS,
            t.open_graph,
            Duration::from_millis(500),
        )
        .await
    {
        Ok(Value::String(s)) => s,
        _ => {
            return Fetch::failed(
                QueryOutcome::Unexpected {
                    detail: "no AF_dataServiceRequests on the q= page".into(),
                },
                1,
                0,
            );
        }
    };
    match parse_data_service_requests(&script) {
        Ok(search) => {
            let id = search.legs.first().and_then(|leg| match &leg.to {
                PlaceRef::City(id) => Some(id.clone()),
                PlaceRef::Airport(_) => None,
            });
            Fetch::ok(id, 1, 0)
        }
        Err(e) => Fetch::failed(
            QueryOutcome::Unexpected {
                detail: format!("unreadable q= search: {e}"),
            },
            1,
            0,
        ),
    }
}

/// Runs `body` on a fresh page and closes the page afterwards.
macro_rules! on_page {
    ($self:ident, $pos:expr, |$page:ident| $body:expr) => {{
        match $self.page($pos).await {
            Ok($page) => {
                let result = $body;
                if let Err(e) = $page.close().await {
                    debug!("closing a page failed: {e}");
                }
                result
            }
            Err(e) => Fetch::error(format!("no page: {e}")),
        }
    }};
}

#[async_trait]
impl Sources for BrowserSources {
    async fn start(
        &mut self,
        points_of_sale: &[PointOfSale],
    ) -> anyhow::Result<()> {
        if self.running.is_some() {
            return Ok(());
        }
        info!(
            binary = %self.config.binary.display(),
            profile = %self.config.user_data_dir.display(),
            headless = self.config.headless,
            "starting Chromium"
        );
        let chromium = Chromium::launch(self.config.clone()).await?;
        let mut contexts = HashMap::new();
        for pos in points_of_sale.iter().filter(|p| p.active) {
            let context = chromium
                .browser()
                .new_context(ContextOptions::default())
                .await?;
            contexts.insert(pos.id, context);
        }
        info!(contexts = contexts.len(), ws = %chromium.ws_url(), "Chromium is up");
        self.running = Some(Running { chromium, contexts });
        Ok(())
    }

    async fn stop(&mut self) {
        let Some(running) = self.running.take() else {
            return;
        };
        for (_, context) in running.contexts {
            let _ = context.close().await;
        }
        if let Err(e) = running.chromium.shutdown().await {
            warn!("Chromium did not shut down cleanly: {e}");
        } else {
            info!("Chromium stopped");
        }
    }

    fn is_running(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| !r.chromium.browser().is_closed())
    }

    fn has_jev(&self) -> bool {
        self.jev.is_some()
    }

    async fn calendar(&mut self, job: &CalendarJob) -> Fetch<Vec<Cell>> {
        let t = self.timeouts;
        let jev = self.jev.clone();
        on_page!(self, Some(job.pos), |page| {
            calendar_on(&page, job, jev.as_deref(), t).await
        })
    }

    async fn explore(
        &mut self,
        job: &ExploreJob,
    ) -> Fetch<Vec<ExploreDestination>> {
        let t = self.timeouts;
        on_page!(self, Some(job.pos), |page| explore_on(&page, job, t).await)
    }

    async fn agent_search(&mut self, job: &AgentJob) -> Fetch<Vec<Cell>> {
        let t = self.timeouts;
        let Some(jev) = self.jev.clone() else {
            return Fetch::error("Jev is not configured");
        };
        on_page!(self, None, |page| agent_on(&page, job, &*jev, t).await)
    }

    async fn confirm(
        &mut self,
        job: &ConfirmJob,
    ) -> Fetch<Option<Confirmation>> {
        let t = self.timeouts;
        on_page!(self, Some(job.pos), |page| confirm_on(&page, job, t).await)
    }

    async fn resolve(&mut self, job: &ResolveJob) -> Fetch<Option<String>> {
        let t = self.timeouts;
        on_page!(self, Some(job.pos), |page| resolve_on(&page, job, t).await)
    }
}
