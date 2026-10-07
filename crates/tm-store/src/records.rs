//! Types the store adds to `tm-domain`'s vocabulary: configuration rows,
//! scheduler state, the query log, requests and daemon bookkeeping.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tm_domain::{
    CountryCode,
    DealId,
    DealStatus,
    FamilyId,
    FamilyKey,
    Place,
    PlaceId,
    PlaceKind,
    QueryId,
    QueryOutcome,
    QueryReason,
    RequestId,
    RouteId,
};

/// A place to add as an origin or destination (DECISIONS.md 0035).
#[derive(Debug, Clone, PartialEq)]
pub struct NewPlace {
    pub code: String,
    pub name: String,
    pub country: CountryCode,
    pub kind: PlaceKind,
    pub lat: f64,
    pub lon: f64,
}

impl NewPlace {
    pub fn airport(
        code: &str,
        name: &str,
        country: &str,
        lat: f64,
        lon: f64,
    ) -> Self {
        Self {
            code: code.to_owned(),
            name: name.to_owned(),
            country: CountryCode::new(country),
            kind: PlaceKind::Airport,
            lat,
            lon,
        }
    }

    pub fn city(
        code: &str,
        name: &str,
        country: &str,
        freebase_id: Option<&str>,
        lat: f64,
        lon: f64,
    ) -> Self {
        Self {
            code: code.to_owned(),
            name: name.to_owned(),
            country: CountryCode::new(country),
            kind: PlaceKind::City {
                freebase_id: freebase_id.map(str::to_owned),
            },
            lat,
            lon,
        }
    }
}

/// A configured origin. Removing one keeps the row with `active = false`.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub place: Place,
    pub active: bool,
}

/// A configured destination, with its scheduler weight.
#[derive(Debug, Clone, PartialEq)]
pub struct Destination {
    pub place: Place,
    pub weight: f64,
    pub active: bool,
}

/// An airline site searched directly (DECISIONS.md 0012), and the
/// destinations it is searched for.
#[derive(Debug, Clone, PartialEq)]
pub struct AirlineSource {
    /// IATA airline code (`"LA"`).
    pub code: String,
    pub name: String,
    pub active: bool,
    pub destinations: Vec<PlaceId>,
}

/// Whether a miles program's session in the daemon's Chromium profile
/// works (DECISIONS.md 0024).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginState {
    LoggedIn,
    /// Never logged in.
    LoggedOut,
    /// Was logged in; the session ended and the app should ask again.
    Expired,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MilesProgram {
    /// `"smiles"`, `"latam_pass"`, `"azul_fidelidade"`.
    pub program: String,
    pub name: String,
    pub login_state: LoginState,
    /// When the login state was last checked.
    pub last_checked: Option<DateTime<Utc>>,
    pub active: bool,
    /// The user's value of 1,000 miles and the program's price for
    /// 1,000, in BRL; unset by default (0083).
    pub value_per_1000: Option<tm_domain::Money>,
    pub buy_per_1000: Option<tm_domain::Money>,
    pub dates: tm_domain::AwardDates,
    /// When the login session in the program's profile ends, if known
    /// (LATAM Pass: about five days, renewed by every load).
    pub session_until: Option<DateTime<Utc>>,
}

impl MilesProgram {
    /// What the user sets for this program.
    pub fn settings(&self) -> tm_domain::ProgramSettings {
        tm_domain::ProgramSettings {
            active: self.active,
            value_per_1000: self.value_per_1000.clone(),
            buy_per_1000: self.buy_per_1000.clone(),
            dates: self.dates,
        }
    }
}

/// One award check's implied value of a mile (DECISIONS.md 0083).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AwardRate {
    pub observed_at: DateTime<Utc>,
    /// The mean over the check's awards of fare before taxes ÷ miles ×
    /// 1,000, in BRL.
    pub brl_per_1000: f64,
    /// Awards the mean is over.
    pub awards: u32,
}

/// What a family sync did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FamilySync {
    /// Families active after the sync.
    pub active: usize,
    /// Families created by this sync.
    pub created: usize,
}

/// A route's round-trip discount learning and additive stop rule
/// (DECISIONS.md 0042). Stored on the route, shared by its families.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RouteTripState {
    /// EWMA of `1 − round trip / (outbound + return)`.
    pub rt_discount_ewma: f64,
    /// Pairs the EWMA has seen.
    pub rt_discount_n: u32,
    /// Checks in a row where the round trip matched its legs.
    pub additive_streak: u32,
    /// Round-trip tracking stopped for this route: `sync_families`
    /// deactivates its round-trip families.
    pub additive: bool,
}

/// The scheduler's state for one family (DECISIONS.md 0028), as plain
/// numbers. The route-level round-trip fields are read from the route
/// and written with [`crate::Store::save_route_trip_state`];
/// [`crate::Store::save_family_state`] ignores them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyStateRecord {
    /// Gamma posterior (shape, rate) on the change rate, in changes per
    /// day (as tm-scheduler's `FamilyState`).
    pub chg_a: f64,
    pub chg_b: f64,
    /// Beta posterior on P(change is a drop).
    pub deal_a: f64,
    pub deal_b: f64,
    /// Beta posterior on Explore hint precision.
    pub prec_a: f64,
    pub prec_b: f64,
    /// Explore hints since the last check.
    pub n_hints: u32,
    pub last_check: Option<DateTime<Utc>>,
    /// The lowest price the last check saw, in minor units.
    pub last_min_price_minor: Option<i64>,
    pub checks: u32,
    /// EWMA of page loads per check.
    pub cost_loads: f64,
    pub rt_discount_ewma: f64,
    pub rt_discount_n: u32,
    pub additive_streak: u32,
    pub additive: bool,
}

impl Default for FamilyStateRecord {
    /// The priors a new family starts with, the same as tm-scheduler's:
    /// fares change about once a day (Gamma(1, 1), mean one change per
    /// day, DECISIONS.md 0029); a uniform Beta(1, 1) on the drop share;
    /// Beta(1, 2) on the hint precision; three page loads per check.
    fn default() -> Self {
        Self {
            chg_a: 1.0,
            chg_b: 1.0,
            deal_a: 1.0,
            deal_b: 1.0,
            prec_a: 1.0,
            prec_b: 2.0,
            n_hints: 0,
            last_check: None,
            last_min_price_minor: None,
            checks: 0,
            cost_loads: 8.0,
            rt_discount_ewma: 0.0,
            rt_discount_n: 0,
            additive_streak: 0,
            additive: false,
        }
    }
}

impl FamilyStateRecord {
    /// The route-level part.
    pub fn route_trip(&self) -> RouteTripState {
        RouteTripState {
            rt_discount_ewma: self.rt_discount_ewma,
            rt_discount_n: self.rt_discount_n,
            additive_streak: self.additive_streak,
            additive: self.additive,
        }
    }
}

/// A query family with its scheduler state.
#[derive(Debug, Clone, PartialEq)]
pub struct FamilyRecord {
    pub id: FamilyId,
    pub key: FamilyKey,
    pub active: bool,
    /// The route's weight (its destination's weight; 1.0 for Explore).
    pub weight: f64,
    /// The rate-limit bucket (`Source::site`).
    pub site: String,
    pub state: FamilyStateRecord,
}

/// One query of the log, with what the app's activity view shows.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryLogEntry {
    pub id: QueryId,
    pub family: FamilyId,
    pub key: FamilyKey,
    pub origin_code: String,
    /// The origin's code again for Explore sweeps.
    pub destination_code: String,
    /// An Explore sweep (pseudo-route).
    pub sweep: bool,
    pub site: String,
    pub reason: QueryReason,
    pub score: Option<f64>,
    pub runner_up: Option<f64>,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// `None` while running.
    pub outcome: Option<QueryOutcome>,
    pub page_loads: Option<u32>,
    /// Observations recorded by this query.
    pub cells: u32,
}

/// Which deals to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DealFilter {
    pub status: Option<DealStatus>,
    /// Only deals the app has not shown yet.
    pub unseen_only: bool,
    /// Newest first; `None` for all.
    pub limit: Option<u32>,
}

/// Work the app asks the daemon for (DECISIONS.md 0046).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestKind {
    /// Check every active family of a route now.
    CheckRoute {
        route: RouteId,
    },
    CheckFamily {
        family: FamilyId,
    },
    /// Re-check a deal candidate now.
    ConfirmDeal {
        deal: DealId,
    },
    /// Price a trip's legs on every active, logged-in miles program
    /// (0083): the outbound on `departure` and, when given, the return
    /// on `return_date`. One page load per leg reads every cabin.
    CheckAward {
        route: RouteId,
        cabin: tm_domain::Cabin,
        departure: NaiveDate,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        return_date: Option<NaiveDate>,
    },
    /// Pause the program's site so the app can open that site's
    /// profile for a miles login (0024, 0062). Every other site keeps
    /// running.
    OpenLogin {
        program: String,
    },
    /// The login window closed: check the program's profile for a
    /// session and resume its site. `None` (an app before 0062) resumes
    /// every paused site.
    CloseLogin {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<String>,
    },
    ReloadConfig,
}

impl RequestKind {
    /// The serde tag, stored in the `kind` column.
    pub fn name(&self) -> &'static str {
        match self {
            RequestKind::CheckRoute { .. } => "check_route",
            RequestKind::CheckFamily { .. } => "check_family",
            RequestKind::ConfirmDeal { .. } => "confirm_deal",
            RequestKind::CheckAward { .. } => "check_award",
            RequestKind::OpenLogin { .. } => "open_login",
            RequestKind::CloseLogin { .. } => "close_login",
            RequestKind::ReloadConfig => "reload_config",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Pending,
    /// The daemon took it and is working on it.
    Claimed,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestRecord {
    pub id: RequestId,
    pub kind: RequestKind,
    pub status: RequestStatus,
    pub created_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

/// One site's daily page-load budget (DECISIONS.md 0039, 0060): a row
/// of `site_budgets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteBudget {
    /// [`tm_domain::Source::site`]: `google_flights`, `airline:TP`.
    pub site: String,
    /// Where the budget starts; a configuration that changes it resets
    /// `daily_budget` to it.
    pub start_budget: u32,
    /// The budget in force (ramped or halved).
    pub daily_budget: u32,
    /// The most the ramp may raise it to.
    pub max_daily_budget: u32,
    pub loads_today: u32,
    /// The day `loads_today` counts.
    pub day: NaiveDate,
    /// The start of the current block-free stretch (the last ramp step,
    /// block or reset).
    pub last_ramp_at: Option<DateTime<Utc>>,
}

impl SiteBudget {
    /// A site's budget before anything was spent on it: its limits,
    /// nothing counted on `day`.
    pub fn new(
        site: impl Into<String>,
        limits: tm_domain::SiteLimits,
        day: NaiveDate,
    ) -> Self {
        let limits = limits.normalized();
        Self {
            site: site.into(),
            start_budget: limits.start,
            daily_budget: limits.start,
            max_daily_budget: limits.max,
            loads_today: 0,
            day,
            last_ramp_at: None,
        }
    }

    /// The loads counted on `today`: none when the row counts another
    /// day.
    pub fn loads_on(&self, today: NaiveDate) -> u32 {
        if self.day == today {
            self.loads_today
        } else {
            0
        }
    }
}

/// A CAPTCHA, "unusual traffic" page or block (DECISIONS.md 0039).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRecord {
    pub id: i64,
    pub site: String,
    pub at: DateTime<Utc>,
    pub detail: String,
    /// Page loads in the hour before it.
    pub loads_last_hour: u32,
}

/// BRL per unit of a currency (DECISIONS.md 0038).
#[derive(Debug, Clone, PartialEq)]
pub struct ExchangeRate {
    pub currency: tm_domain::Currency,
    pub brl_per_unit: f64,
    pub at: DateTime<Utc>,
}

/// The daemon's status, for the app's status chip, written every 30 s
/// and when the daemon starts and stops (DECISIONS.md 0057). What each
/// site is doing is in [`SiteStatus`] (0062).
#[derive(Debug, Clone, PartialEq)]
pub struct Heartbeat {
    pub pid: u32,
    pub updated_at: DateTime<Utc>,
    /// Free text: `"starting"`, `"running"`, `"stopped"`.
    pub state: String,
    /// Free text describing the next check.
    pub next_check: Option<String>,
    /// When the daemon looks for work again, if it knows.
    pub next_check_at: Option<DateTime<Utc>>,
    /// Loads today and today's budgets, summed over the sites.
    pub loads_today: u32,
    pub budget: u32,
}

/// What one site's worker is doing (DECISIONS.md 0057, 0062): a row of
/// `site_status` and its `site_checks`, written when a check starts and
/// ends and when its state changes.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteStatus {
    pub site: String,
    pub updated_at: DateTime<Utc>,
    /// `"idle"`, `"checking"`, `"paused for login: smiles"`,
    /// `"cooling down"`, `"browser unavailable"`, `"stopped"`.
    pub state: String,
    /// When the site looks for work again, if it knows.
    pub next_check_at: Option<DateTime<Utc>>,
    /// The checks in progress, oldest first, while `state` is
    /// `"checking"` (0072: several at once).
    pub checks: Vec<CheckInProgress>,
}

/// The check the daemon is running (DECISIONS.md 0057). The family
/// gives the route, cabin, source, point of sale and kind.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckInProgress {
    pub family: tm_domain::FamilyId,
    /// Its row in the query log.
    pub query: Option<tm_domain::QueryId>,
    pub reason: tm_domain::QueryReason,
    pub started_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// New families start from tm-scheduler's `FamilyState` priors
    /// (change rate per day).
    #[test]
    fn default_state_is_the_schedulers_prior() {
        let s = FamilyStateRecord::default();
        assert_eq!((s.chg_a, s.chg_b), (1.0, 1.0));
        assert_eq!((s.deal_a, s.deal_b), (1.0, 1.0));
        assert_eq!((s.prec_a, s.prec_b), (1.0, 2.0));
        assert_eq!(s.n_hints, 0);
        assert_eq!(s.last_check, None);
        assert_eq!(s.last_min_price_minor, None);
        assert_eq!(s.checks, 0);
        assert_eq!(s.cost_loads, 8.0);
        assert_eq!(s.route_trip(), RouteTripState::default());
    }
}
