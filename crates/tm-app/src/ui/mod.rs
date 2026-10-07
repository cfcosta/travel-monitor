//! The window: a top bar and five screens (DECISIONS.md 0044).
//!
//! [`TravelMonitor`] owns the latest [`Snapshot`] and the interface
//! state (screen, filters, selection). Snapshots are fetched on a
//! background task when the backend says its data changed (debounced),
//! when the selection changes, and every half minute as a fallback;
//! nothing here waits on the database (0057).
//!
//! A one-second clock re-renders relative times, the heartbeat's age,
//! countdowns and the running check's elapsed time without reading
//! anything. Rows that just arrived are tinted for two seconds.

mod activity;
mod deals;
mod detail;
pub mod map;
mod routes;
mod settings;
pub mod text_field;
mod topbar;
pub mod widgets;

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use chrono::{DateTime, NaiveDate, Utc};
use gpui::{
    AnyElement,
    Context,
    Entity,
    FocusHandle,
    Focusable,
    SharedString,
    Task,
    Window,
    div,
    prelude::*,
    px,
};
use tm_domain::{Cabin, PlaceId};

pub use self::settings::SettingsForm;
pub use crate::data::PayWith;
use crate::{
    data::{Request, RouteRow, RowKey, SharedBackend, Snapshot, SnapshotQuery},
    live::{self, Clock, Debounce, Fresh, Highlights},
    login::LoginConfig,
    theme::{self, color},
};

/// How often a fading highlight is redrawn (20 frames a second).
const FADE_FRAME: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Deals,
    Routes,
    Detail,
    Activity,
    Settings,
}

impl Screen {
    /// The tabs of the top bar; the route detail is reached from a row.
    pub const TABS: [Screen; 4] = [
        Screen::Deals,
        Screen::Routes,
        Screen::Activity,
        Screen::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Screen::Deals => "Deals",
            Screen::Routes | Screen::Detail => "Routes",
            Screen::Activity => "Activity",
            Screen::Settings => "Settings",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListTab {
    Deals,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    BiggestDrop,
    LowestPrice,
    /// Price over the round trip's great-circle distance: a cheap trip
    /// that goes far ranks above an equally cheap short hop.
    LowestPricePerKm,
    RecentlyFound,
}

impl Sort {
    pub const ALL: [Sort; 4] = [
        Sort::BiggestDrop,
        Sort::LowestPrice,
        Sort::LowestPricePerKm,
        Sort::RecentlyFound,
    ];

    /// The sort's name; paying in miles, prices are miles (0083).
    pub fn label(self, pay: PayWith) -> &'static str {
        match (self, pay) {
            (Sort::BiggestDrop, _) => "Biggest drop",
            (Sort::LowestPrice, PayWith::Miles) => "Fewest miles",
            (Sort::LowestPrice, _) => "Lowest price",
            (Sort::LowestPricePerKm, PayWith::Miles) => "Fewest miles per km",
            (Sort::LowestPricePerKm, _) => "Lowest price per km",
            (Sort::RecentlyFound, _) => "Recently found",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutesFilter {
    All,
    Deals,
    Domestic,
    International,
}

impl RoutesFilter {
    pub const ALL: [RoutesFilter; 4] = [
        RoutesFilter::All,
        RoutesFilter::Deals,
        RoutesFilter::Domestic,
        RoutesFilter::International,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RoutesFilter::All => "All",
            RoutesFilter::Deals => "Deals",
            RoutesFilter::Domestic => "Domestic",
            RoutesFilter::International => "International",
        }
    }
}

/// The dropdowns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Origin,
    Cabin,
    Sort,
}

/// What the app was started with.
#[derive(Clone)]
pub struct AppEnv {
    pub backend: SharedBackend,
    pub login: LoginConfig,
    /// The runtime that runs the login window; `None` in tests.
    pub tokio: Option<tokio::runtime::Handle>,
    /// The fallback refresh, for changes the backend did not notice.
    pub refresh_every: Duration,
    pub debounce: Debounce,
    pub clock: Clock,
}

pub struct TravelMonitor {
    pub(crate) env: AppEnv,
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) error: Option<SharedString>,
    pub(crate) screen: Screen,
    pub(crate) list_tab: ListTab,
    /// Cash, miles or both (0083).
    pub(crate) pay: PayWith,
    pub(crate) cabin: Option<Cabin>,
    pub(crate) sort: Sort,
    pub(crate) routes_filter: RoutesFilter,
    pub(crate) selected: Option<RowKey>,
    pub(crate) origin: Option<PlaceId>,
    pub(crate) calendar_days: Option<u16>,
    pub(crate) departure: Option<NaiveDate>,
    pub(crate) menu: Option<Menu>,
    /// The menu a click outside just closed, so the same click on its
    /// trigger does not open it again.
    menu_closed: Option<(Menu, std::time::Instant)>,
    pub(crate) toast: Option<SharedString>,
    pub(crate) settings_form: Option<Entity<SettingsForm>>,
    /// Miles programs with a login window open.
    pub(crate) logins: HashSet<String>,
    /// The Deals map's zoom and pan (0073).
    pub(crate) deals_map: map::MapState,
    generation: u64,
    /// The wall clock, moved every second (0057).
    pub(crate) now: DateTime<Utc>,
    /// The executor's time when the window opened, for [`Clock::From`].
    opened_at: Instant,
    /// The executor's time of the frame being drawn.
    frame_at: Instant,
    /// Rows that just arrived.
    pub(crate) fresh: Highlights<Fresh>,
    fading: bool,
    focus: FocusHandle,
    loading: Option<Task<()>>,
    toast_task: Option<Task<()>>,
    _poll: Option<Task<()>>,
    _watch: Option<Task<()>>,
    _clock: Option<Task<()>>,
}

impl TravelMonitor {
    pub fn new(env: AppEnv, cx: &mut Context<Self>) -> Self {
        let opened_at = cx.background_executor().now();
        let now = env.clock.read(Duration::ZERO);
        let mut this = Self {
            env,
            snapshot: None,
            error: None,
            screen: Screen::Deals,
            list_tab: ListTab::Deals,
            pay: PayWith::Both,
            cabin: None,
            sort: Sort::BiggestDrop,
            routes_filter: RoutesFilter::All,
            selected: None,
            origin: None,
            calendar_days: None,
            departure: None,
            menu: None,
            menu_closed: None,
            toast: None,
            settings_form: None,
            logins: HashSet::new(),
            deals_map: map::MapState::default(),
            generation: 0,
            now,
            opened_at,
            frame_at: opened_at,
            fresh: Highlights::default(),
            fading: false,
            focus: cx.focus_handle(),
            loading: None,
            toast_task: None,
            _poll: None,
            _watch: None,
            _clock: None,
        };
        this.refresh(cx);
        this.start_polling(cx);
        this.start_watching(cx);
        this.start_clock(cx);
        this
    }

    /// Refreshes when the backend says its data changed, once a burst of
    /// notices settles (0057).
    fn start_watching(&mut self, cx: &mut Context<Self>) {
        let mut changes = self.env.backend.changes();
        let debounce = self.env.debounce;
        self._watch = Some(cx.spawn(async move |this, cx| {
            while changes.next().await {
                let mut wait = debounce.first();
                let mut waited = Duration::ZERO;
                loop {
                    cx.background_executor().timer(wait).await;
                    waited += wait;
                    match debounce.next(waited, changes.take()) {
                        Some(more) => wait = more,
                        None => break,
                    }
                }
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Moves the wall clock every second; nothing is read.
    fn start_clock(&mut self, cx: &mut Context<Self>) {
        self._clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// One tick of the clock.
    pub fn tick(&mut self, cx: &mut Context<Self>) {
        self.now = self.read_clock(cx);
        self.fresh.prune(cx.background_executor().now());
        cx.notify();
    }

    fn read_clock(&self, cx: &gpui::App) -> DateTime<Utc> {
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.opened_at);
        self.env.clock.read(elapsed)
    }

    /// Tints `keys` and redraws while the tint fades.
    fn mark_fresh(&mut self, keys: Vec<Fresh>, cx: &mut Context<Self>) {
        if keys.is_empty() {
            return;
        }
        self.fresh.mark(keys, cx.background_executor().now());
        if self.fading {
            return;
        }
        self.fading = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FADE_FRAME).await;
                let more = this.update(cx, |this, cx| {
                    this.fresh.prune(cx.background_executor().now());
                    this.fading = !this.fresh.is_empty();
                    cx.notify();
                    this.fading
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// How strongly `key` is tinted in the frame being drawn, 0 to 1.
    pub(crate) fn glow(&self, key: Fresh) -> f32 {
        self.fresh.strength(&key, self.frame_at)
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        let every = self.env.refresh_every;
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(every).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn query(&self) -> SnapshotQuery {
        SnapshotQuery {
            origin: self.origin,
            selected: self.selected,
            calendar_days: self.calendar_days,
            departure: self.departure,
        }
    }

    /// Fetches a snapshot for the current query on a background task.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let backend = self.env.backend.clone();
        let query = self.query();
        let fetch =
            cx.background_spawn(async move { backend.snapshot(query).await });
        self.loading = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.apply(result, cx);
                }
            });
        }));
    }

    pub fn apply(
        &mut self,
        result: anyhow::Result<Snapshot>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(snapshot) => {
                let arrived = self
                    .snapshot
                    .as_ref()
                    .map(|old| live::arrivals(old, &snapshot))
                    .unwrap_or_default();
                self.error = None;
                self.origin = snapshot.origin;
                self.snapshot = Some(snapshot);
                self.now = self.read_clock(cx);
                self.mark_fresh(arrived, cx);
                if let Some(form) = &self.settings_form {
                    form.update(cx, |form, cx| {
                        if let Some(s) = &self.snapshot {
                            form.sync(s, cx);
                        }
                    });
                }
                let visible = self.visible_rows();
                let still_there = self.selected.is_some_and(|key| {
                    self.snapshot.as_ref().is_some_and(|s| s.row(key).is_some())
                });
                if !still_there {
                    let first =
                        visible.first().map(|r| r.key()).or_else(|| {
                            self.snapshot
                                .as_ref()
                                .and_then(|s| s.rows.first().map(RouteRow::key))
                        });
                    if first.is_some() {
                        self.selected = first;
                        self.departure = None;
                        self.refresh(cx);
                    }
                }
            }
            Err(error) => {
                tracing::warn!("cannot read the database: {error:#}");
                self.error = Some(format!("{error:#}").into());
            }
        }
        cx.notify();
    }

    /// The rows of the main list, filtered and sorted.
    pub fn visible_rows(&self) -> Vec<&RouteRow> {
        let Some(s) = &self.snapshot else {
            return Vec::new();
        };
        let mut rows: Vec<&RouteRow> = s
            .rows
            .iter()
            .filter(|r| match self.list_tab {
                ListTab::Deals => r.is_deal(self.pay),
                // Paying in miles, only routes with an award are listed.
                ListTab::All => self.pay != PayWith::Miles || r.award.is_some(),
            })
            .filter(|r| self.cabin.is_none_or(|c| c == r.cabin))
            .collect();
        sort_rows(&mut rows, self.sort, self.pay);
        rows
    }

    pub fn select(&mut self, key: RowKey, cx: &mut Context<Self>) {
        if self.selected != Some(key) {
            self.selected = Some(key);
            self.departure = None;
            self.refresh(cx);
        }
        cx.notify();
    }

    pub fn navigate(
        &mut self,
        screen: Screen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.screen = screen;
        self.menu = None;
        if screen == Screen::Settings && self.settings_form.is_none() {
            let snapshot = self.snapshot.clone();
            self.settings_form =
                Some(cx.new(|cx| {
                    SettingsForm::new(snapshot.as_ref(), window, cx)
                }));
        }
        cx.notify();
    }

    pub fn set_origin(&mut self, origin: PlaceId, cx: &mut Context<Self>) {
        self.origin = Some(origin);
        self.selected = None;
        self.menu = None;
        self.refresh(cx);
        cx.notify();
    }

    pub fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        if let Some((closed, at)) = self.menu_closed.take()
            && closed == menu
            && at.elapsed() < Duration::from_millis(300)
        {
            return;
        }
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_closed =
            self.menu.take().map(|m| (m, std::time::Instant::now()));
        cx.notify();
    }

    pub fn show_toast(
        &mut self,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.toast = Some(text.into());
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Asks the daemon to check the selected route now (0046).
    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        let Some(row) =
            self.selected.and_then(|k| self.snapshot.as_ref()?.row(k))
        else {
            return;
        };
        let label = format!("{} → {}", row.route.origin.code, row.code());
        let request = match (row.status, row.deal) {
            (crate::data::RowStatus::Confirming, Some(deal)) => {
                Request::ConfirmDeal(deal)
            }
            _ => Request::CheckRoute(row.route.id),
        };
        self.enqueue(request, format!("Check requested: {label}"), cx);
    }

    /// Asks the daemon to price the selected trip's legs on the miles
    /// programs (0083): the outbound on its departure and the return.
    pub fn check_award(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.snapshot.as_ref() else {
            return;
        };
        let Some(row) = self.selected.and_then(|k| s.row(k)) else {
            return;
        };
        let days = self
            .calendar_days
            .unwrap_or(s.settings.trips.round_trip_days);
        let departure = self
            .departure
            .or(row.best.as_ref().map(|t| t.departure))
            .or(row.award.as_ref().map(|a| a.best.departure));
        let Some(departure) = departure else {
            return self.show_toast("No dates to check yet", cx);
        };
        let return_date = departure + chrono::Duration::days(i64::from(days));
        let label = format!("{} → {}", row.route.origin.code, row.code());
        let request = Request::CheckAward {
            route: row.route.id,
            cabin: row.cabin,
            departure,
            return_date: Some(return_date),
        };
        self.enqueue(request, format!("Award check requested: {label}"), cx);
    }

    /// A miles program the daemon reads that is active and logged in:
    /// "Check award" is offered only then.
    pub fn can_check_award(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| {
            s.miles.iter().any(|m| {
                m.active()
                    && m.has_reader
                    && m.login == crate::data::Login::LoggedIn
            })
        })
    }

    pub fn enqueue(
        &mut self,
        request: Request,
        done: String,
        cx: &mut Context<Self>,
    ) {
        let backend = self.env.backend.clone();
        let sent =
            cx.background_spawn(async move { backend.enqueue(request).await });
        cx.spawn(async move |this, cx| {
            let result = sent.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => this.show_toast(done, cx),
                Err(error) => this.show_toast(
                    format!("Could not ask the daemon: {error:#}"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Opens the selected trip in Google Flights with the system opener.
    pub fn open_in_google_flights(&mut self, cx: &mut Context<Self>) {
        let url = self
            .snapshot
            .as_ref()
            .and_then(|s| s.detail.as_ref())
            .filter(|d| Some(d.key) == self.selected)
            .and_then(|d| d.url.clone());
        match url {
            Some(url) => cx.open_url(&url),
            None => self.show_toast("No search link for this trip yet", cx),
        }
    }

    /// Runs a configuration edit on a background task, asks the daemon
    /// to reload, then refreshes.
    pub fn write(
        &mut self,
        edit: impl Future<Output = anyhow::Result<()>> + Send + 'static,
        done: &'static str,
        cx: &mut Context<Self>,
    ) {
        let task = cx.background_spawn(edit);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.show_toast(done, cx);
                        let backend = this.env.backend.clone();
                        cx.background_spawn(async move {
                            let _ =
                                backend.enqueue(Request::ReloadConfig).await;
                        })
                        .detach();
                    }
                    Err(error) => this.show_toast(format!("{error:#}"), cx),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }
}

/// Sorts rows: biggest drop first, lowest price first, lowest price per
/// kilometre first, or newest deal first (rows without a value go last).
/// Paying in miles, prices are miles (then taxes); paying either way,
/// the bigger of a row's two drops counts, and rows priced only in miles
/// come after the cash ones (0083).
pub fn sort_rows(rows: &mut [&RouteRow], sort: Sort, pay: PayWith) {
    match sort {
        Sort::BiggestDrop => rows.sort_by(|a, b| {
            let key = |r: &RouteRow| {
                let cash = r.change.filter(|_| pay != PayWith::Miles);
                let miles = r.award_for(pay).and_then(|a| a.change);
                match (cash, miles) {
                    (Some(c), Some(m)) => c.min(m),
                    (c, m) => c.or(m).unwrap_or(f64::INFINITY),
                }
            };
            key(a).total_cmp(&key(b))
        }),
        Sort::LowestPrice if pay == PayWith::Miles => rows.sort_by_key(|r| {
            r.award_for(pay).map_or((i64::MAX, i64::MAX), |a| {
                (a.best.price.miles, a.best.price.taxes.minor)
            })
        }),
        Sort::LowestPrice => rows.sort_by_key(|r| {
            r.best.as_ref().map_or(i64::MAX, |t| t.price.minor)
        }),
        Sort::LowestPricePerKm => rows.sort_by(|a, b| {
            let key = |r: &RouteRow| {
                if pay == PayWith::Miles {
                    r.award_for(pay)
                        .map(|a| a.best.price.miles as f64)
                        .filter(|_| r.distance_km() >= 1.0)
                        .map(|m| m / r.distance_km())
                } else {
                    r.price_per_km()
                }
                .unwrap_or(f64::INFINITY)
            };
            key(a).total_cmp(&key(b))
        }),
        Sort::RecentlyFound => rows.sort_by_key(|r| {
            let miles = r.award_for(pay).and_then(|a| a.found_at);
            let cash = r.found_at.filter(|_| pay != PayWith::Miles);
            std::cmp::Reverse(cash.max(miles))
        }),
    }
}

impl Focusable for TravelMonitor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TravelMonitor {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.frame_at = cx.background_executor().now();
        let fonts = theme::fonts(cx);
        let body: AnyElement = match (&self.snapshot, self.screen) {
            (None, _) => self.render_loading(),
            (Some(_), Screen::Deals) => self.render_deals(&fonts, window, cx),
            (Some(_), Screen::Routes) => self.render_routes(&fonts, cx),
            (Some(_), Screen::Detail) => self.render_detail(&fonts, cx),
            (Some(_), Screen::Activity) => self.render_activity(&fonts, cx),
            (Some(_), Screen::Settings) => self.render_settings(&fonts, cx),
        };
        div()
            .id("travel-monitor")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(color(theme::GROUND))
            .text_color(color(theme::TEXT))
            .font_family(fonts.sans.clone())
            .text_size(px(14.))
            .child(self.render_topbar(&fonts, cx))
            .children(self.error.clone().map(|e| {
                div()
                    .px(px(24.))
                    .py(px(8.))
                    .bg(color(0x2A1414))
                    .text_color(color(theme::RED))
                    .text_size(px(13.))
                    .child(SharedString::from(format!(
                        "Cannot read the database: {e}"
                    )))
            }))
            .child(div().flex_1().min_h_0().flex().child(body))
            .children(self.toast.clone().map(|t| {
                div()
                    .absolute()
                    .bottom(px(20.))
                    .left(px(20.))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(10.))
                    .bg(color(theme::CARD))
                    .border_1()
                    .border_color(color(theme::BORDER_INPUT))
                    .text_size(px(13.))
                    .debug_selector(|| "toast".into())
                    .child(t)
            }))
    }
}

impl TravelMonitor {
    fn render_loading(&self) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color(theme::TEXT_FAINT))
            .child(if self.error.is_some() {
                "Waiting for the database…"
            } else {
                "Loading…"
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod screenshots;
#[cfg(test)]
mod tests;
