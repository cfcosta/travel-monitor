//! Every conversion between domain types and rows lives here.
//!
//! Enums are stored as their serde snake_case names (a test checks the
//! two agree), money as minor units plus a currency column, dates and
//! timestamps as ISO text (sqlx's chrono encoding).

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{
    Cabin,
    CountryCode,
    Currency,
    Deal,
    DealId,
    DealReason,
    DealStatus,
    ExploreHint,
    FamilyId,
    FamilyKey,
    FamilyKind,
    Leg,
    Money,
    Observation,
    ObservationId,
    Place,
    PlaceId,
    PlaceKind,
    PointOfSale,
    PosId,
    QueryId,
    QueryOutcome,
    Route,
    RouteId,
    SellerQuote,
    Settings,
    Source,
    settings::{Passengers, TripRange},
};

use crate::{
    error::{Result, StoreError},
    records::{
        FamilyRecord,
        FamilyStateRecord,
        LoginState,
        QueryLogEntry,
        RequestRecord,
        RequestStatus,
    },
};

fn corrupt(what: &str, value: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("{what}: {value}"))
}

/// A count column as an unsigned number.
pub(crate) fn count<T: TryFrom<i64>>(what: &str, value: i64) -> Result<T> {
    T::try_from(value).map_err(|_| corrupt(what, value))
}

pub(crate) fn opt_count<T: TryFrom<i64>>(
    what: &str,
    value: Option<i64>,
) -> Result<Option<T>> {
    value.map(|v| count(what, v)).transpose()
}

// ---------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------

pub(crate) fn cabin_name(cabin: Cabin) -> &'static str {
    match cabin {
        Cabin::Economy => "economy",
        Cabin::PremiumEconomy => "premium_economy",
        Cabin::Business => "business",
        Cabin::First => "first",
    }
}

pub(crate) fn parse_cabin(value: &str) -> Result<Cabin> {
    Cabin::ALL
        .into_iter()
        .find(|cabin| cabin_name(*cabin) == value)
        .ok_or_else(|| corrupt("cabin", value))
}

pub(crate) fn leg_name(leg: Leg) -> &'static str {
    match leg {
        Leg::Outbound => "outbound",
        Leg::Return => "return",
    }
}

pub(crate) fn parse_leg(value: &str) -> Result<Leg> {
    match value {
        "outbound" => Ok(Leg::Outbound),
        "return" => Ok(Leg::Return),
        other => Err(corrupt("leg", other)),
    }
}

pub(crate) fn deal_status_name(status: DealStatus) -> &'static str {
    match status {
        DealStatus::Candidate => "candidate",
        DealStatus::Confirmed => "confirmed",
        DealStatus::Rejected => "rejected",
        DealStatus::Expired => "expired",
    }
}

pub(crate) fn parse_deal_status(value: &str) -> Result<DealStatus> {
    match value {
        "candidate" => Ok(DealStatus::Candidate),
        "confirmed" => Ok(DealStatus::Confirmed),
        "rejected" => Ok(DealStatus::Rejected),
        "expired" => Ok(DealStatus::Expired),
        other => Err(corrupt("deal status", other)),
    }
}

pub(crate) fn deal_reason_name(reason: DealReason) -> &'static str {
    match reason {
        DealReason::BelowMedian => "below_median",
        DealReason::AllTimeLow => "all_time_low",
    }
}

pub(crate) fn parse_deal_reason(value: &str) -> Result<DealReason> {
    match value {
        "below_median" => Ok(DealReason::BelowMedian),
        "all_time_low" => Ok(DealReason::AllTimeLow),
        other => Err(corrupt("deal reason", other)),
    }
}

pub(crate) fn login_state_name(state: LoginState) -> &'static str {
    match state {
        LoginState::LoggedIn => "logged_in",
        LoginState::LoggedOut => "logged_out",
        LoginState::Expired => "expired",
    }
}

pub(crate) fn parse_login_state(value: &str) -> Result<LoginState> {
    match value {
        "logged_in" => Ok(LoginState::LoggedIn),
        "logged_out" => Ok(LoginState::LoggedOut),
        "expired" => Ok(LoginState::Expired),
        other => Err(corrupt("login state", other)),
    }
}

pub(crate) fn request_status_name(status: RequestStatus) -> &'static str {
    match status {
        RequestStatus::Pending => "pending",
        RequestStatus::Claimed => "claimed",
        RequestStatus::Done => "done",
        RequestStatus::Failed => "failed",
    }
}

pub(crate) fn parse_request_status(value: &str) -> Result<RequestStatus> {
    match value {
        "pending" => Ok(RequestStatus::Pending),
        "claimed" => Ok(RequestStatus::Claimed),
        "done" => Ok(RequestStatus::Done),
        "failed" => Ok(RequestStatus::Failed),
        other => Err(corrupt("request status", other)),
    }
}

/// The `outcome` column: `QueryOutcome`'s serde tag, and its detail.
pub(crate) fn outcome_parts(
    outcome: &QueryOutcome,
) -> (&'static str, Option<&str>) {
    match outcome {
        QueryOutcome::Ok => ("ok", None),
        QueryOutcome::Blocked { detail } => ("blocked", Some(detail)),
        QueryOutcome::Unexpected { detail } => ("unexpected", Some(detail)),
        QueryOutcome::LoggedOut => ("logged_out", None),
        QueryOutcome::Error { detail } => ("error", Some(detail)),
    }
}

// ---------------------------------------------------------------------
// Money
// ---------------------------------------------------------------------

pub(crate) fn money(minor: i64, currency: String) -> Money {
    Money::new(minor, Currency(currency))
}

pub(crate) fn opt_money(
    what: &str,
    minor: Option<i64>,
    currency: Option<String>,
) -> Result<Option<Money>> {
    match (minor, currency) {
        (Some(minor), Some(currency)) => Ok(Some(money(minor, currency))),
        (None, None) => Ok(None),
        _ => Err(corrupt(what, "amount without currency")),
    }
}

pub(crate) fn money_parts(
    price: Option<&Money>,
) -> (Option<i64>, Option<&str>) {
    match price {
        Some(m) => (Some(m.minor), Some(m.currency.as_str())),
        None => (None, None),
    }
}

// ---------------------------------------------------------------------
// Places and routes
// ---------------------------------------------------------------------

/// The `kind` and `freebase_id` columns of a place.
pub(crate) fn place_kind_parts(
    kind: &PlaceKind,
) -> (&'static str, Option<&str>) {
    match kind {
        PlaceKind::Airport => ("airport", None),
        PlaceKind::City { freebase_id } => ("city", freebase_id.as_deref()),
    }
}

pub(crate) struct PlaceRow {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub country: String,
    pub kind: String,
    pub freebase_id: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

impl TryFrom<PlaceRow> for Place {
    type Error = StoreError;

    fn try_from(row: PlaceRow) -> Result<Self> {
        let kind = match row.kind.as_str() {
            "airport" => PlaceKind::Airport,
            "city" => PlaceKind::City {
                freebase_id: row.freebase_id,
            },
            other => return Err(corrupt("place kind", other)),
        };
        Ok(Place {
            id: PlaceId(row.id),
            code: row.code,
            name: row.name,
            country: CountryCode(row.country),
            kind,
            lat: row.lat,
            lon: row.lon,
        })
    }
}

/// A route joined with both of its places.
pub(crate) struct RouteRow {
    pub id: i64,
    pub active: bool,
    pub weight: f64,
    pub o_id: i64,
    pub o_code: String,
    pub o_name: String,
    pub o_country: String,
    pub o_kind: String,
    pub o_freebase_id: Option<String>,
    pub o_lat: f64,
    pub o_lon: f64,
    pub d_id: i64,
    pub d_code: String,
    pub d_name: String,
    pub d_country: String,
    pub d_kind: String,
    pub d_freebase_id: Option<String>,
    pub d_lat: f64,
    pub d_lon: f64,
}

impl TryFrom<RouteRow> for Route {
    type Error = StoreError;

    fn try_from(row: RouteRow) -> Result<Self> {
        let origin = PlaceRow {
            id: row.o_id,
            code: row.o_code,
            name: row.o_name,
            country: row.o_country,
            kind: row.o_kind,
            freebase_id: row.o_freebase_id,
            lat: row.o_lat,
            lon: row.o_lon,
        };
        let destination = PlaceRow {
            id: row.d_id,
            code: row.d_code,
            name: row.d_name,
            country: row.d_country,
            kind: row.d_kind,
            freebase_id: row.d_freebase_id,
            lat: row.d_lat,
            lon: row.d_lon,
        };
        Ok(Route {
            id: RouteId(row.id),
            origin: origin.try_into()?,
            destination: destination.try_into()?,
            weight: row.weight,
            active: row.active,
        })
    }
}

pub(crate) struct PosRow {
    pub id: i64,
    pub country: String,
    pub currency: String,
    pub is_home: bool,
    pub active: bool,
}

impl From<PosRow> for PointOfSale {
    fn from(row: PosRow) -> Self {
        PointOfSale {
            id: PosId(row.id),
            country: CountryCode(row.country),
            currency: Currency(row.currency),
            is_home: row.is_home,
            active: row.active,
        }
    }
}

// ---------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------

pub(crate) struct SettingsRow {
    pub adults: i64,
    pub children: i64,
    pub infants_in_seat: i64,
    pub infants_on_lap: i64,
    pub trip_min_days: i64,
    pub trip_max_days: i64,
    pub round_trip_days: i64,
    pub horizon_days: i64,
    pub iof: f64,
    pub card_spread: f64,
    pub daily_budget: i64,
    pub max_daily_budget: i64,
    pub max_interval_hours: i64,
    pub min_interval_hours: i64,
    pub random_share: f64,
    pub deal_threshold: f64,
    pub min_history_weeks: i64,
    pub additive_checks_to_stop: i64,
    pub language: String,
}

impl TryFrom<SettingsRow> for Settings {
    type Error = StoreError;

    fn try_from(row: SettingsRow) -> Result<Self> {
        Ok(Settings {
            passengers: Passengers {
                adults: count("adults", row.adults)?,
                children: count("children", row.children)?,
                infants_in_seat: count("infants_in_seat", row.infants_in_seat)?,
                infants_on_lap: count("infants_on_lap", row.infants_on_lap)?,
            },
            trips: TripRange {
                min_days: count("trip_min_days", row.trip_min_days)?,
                max_days: count("trip_max_days", row.trip_max_days)?,
                round_trip_days: count("round_trip_days", row.round_trip_days)?,
            },
            horizon_days: count("horizon_days", row.horizon_days)?,
            iof: row.iof,
            card_spread: row.card_spread,
            daily_budget: count("daily_budget", row.daily_budget)?,
            max_daily_budget: count("max_daily_budget", row.max_daily_budget)?,
            max_interval_hours: count(
                "max_interval_hours",
                row.max_interval_hours,
            )?,
            min_interval_hours: count(
                "min_interval_hours",
                row.min_interval_hours,
            )?,
            random_share: row.random_share,
            deal_threshold: row.deal_threshold,
            min_history_weeks: count(
                "min_history_weeks",
                row.min_history_weeks,
            )?,
            additive_checks_to_stop: count(
                "additive_checks_to_stop",
                row.additive_checks_to_stop,
            )?,
            language: row.language,
        })
    }
}

// ---------------------------------------------------------------------
// Families
// ---------------------------------------------------------------------

/// A family key as columns.
pub(crate) struct FamilyColumns<'a> {
    /// Canonical text, unique per family.
    pub key: String,
    pub cabin: &'static str,
    pub source: &'static str,
    pub source_code: Option<&'a str>,
    pub kind: &'static str,
    pub leg: Option<&'static str>,
    pub days: Option<i64>,
    pub site: String,
}

pub(crate) fn family_columns(key: &FamilyKey) -> FamilyColumns<'_> {
    let (source, source_code) = match &key.source {
        Source::GoogleFlights => ("google_flights", None),
        Source::Airline { code } => ("airline", Some(code.as_str())),
        Source::Miles { program } => ("miles", Some(program.as_str())),
    };
    let (kind, leg, days, kind_text) = match key.kind {
        FamilyKind::Leg { leg } => (
            "leg",
            Some(leg_name(leg)),
            None,
            format!("leg:{}", leg_name(leg)),
        ),
        FamilyKind::RoundTrip { days } => (
            "round_trip",
            None,
            Some(i64::from(days)),
            format!("round_trip:{days}"),
        ),
        FamilyKind::Explore => ("explore", None, None, "explore".to_owned()),
    };
    let site = key.source.site();
    FamilyColumns {
        key: format!(
            "{}/{}/{}/{}/{}",
            key.route,
            cabin_name(key.cabin),
            site,
            key.pos,
            kind_text
        ),
        cabin: cabin_name(key.cabin),
        source,
        source_code,
        kind,
        leg,
        days,
        site,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn family_key(
    route_id: i64,
    cabin: &str,
    source: &str,
    source_code: Option<String>,
    pos_id: i64,
    kind: &str,
    leg: Option<&str>,
    days: Option<i64>,
) -> Result<FamilyKey> {
    let source = match (source, source_code) {
        ("google_flights", None) => Source::GoogleFlights,
        ("airline", Some(code)) => Source::Airline { code },
        ("miles", Some(program)) => Source::Miles { program },
        (other, code) => {
            return Err(corrupt("source", format!("{other} {code:?}")));
        }
    };
    let kind = match (kind, leg, days) {
        ("leg", Some(leg), None) => FamilyKind::Leg {
            leg: parse_leg(leg)?,
        },
        ("round_trip", None, Some(days)) => FamilyKind::RoundTrip {
            days: count("days", days)?,
        },
        ("explore", None, None) => FamilyKind::Explore,
        (other, leg, days) => {
            return Err(corrupt(
                "family kind",
                format!("{other} {leg:?} {days:?}"),
            ));
        }
    };
    Ok(FamilyKey {
        route: RouteId(route_id),
        cabin: parse_cabin(cabin)?,
        source,
        pos: PosId(pos_id),
        kind,
    })
}

pub(crate) struct FamilyRow {
    pub id: i64,
    pub route_id: i64,
    pub cabin: String,
    pub source: String,
    pub source_code: Option<String>,
    pub pos_id: i64,
    pub kind: String,
    pub leg: Option<String>,
    pub days: Option<i64>,
    pub site: String,
    pub active: bool,
    pub weight: f64,
    pub chg_a: f64,
    pub chg_b: f64,
    pub deal_a: f64,
    pub deal_b: f64,
    pub prec_a: f64,
    pub prec_b: f64,
    pub n_hints: i64,
    pub last_check: Option<DateTime<Utc>>,
    pub last_min_price_minor: Option<i64>,
    pub checks: i64,
    pub cost_loads: f64,
    pub rt_discount_ewma: f64,
    pub rt_discount_n: i64,
    pub additive_streak: i64,
    pub additive: bool,
}

impl TryFrom<FamilyRow> for FamilyRecord {
    type Error = StoreError;

    fn try_from(row: FamilyRow) -> Result<Self> {
        let key = family_key(
            row.route_id,
            &row.cabin,
            &row.source,
            row.source_code,
            row.pos_id,
            &row.kind,
            row.leg.as_deref(),
            row.days,
        )?;
        Ok(FamilyRecord {
            id: FamilyId(row.id),
            key,
            active: row.active,
            weight: row.weight,
            site: row.site,
            state: FamilyStateRecord {
                chg_a: row.chg_a,
                chg_b: row.chg_b,
                deal_a: row.deal_a,
                deal_b: row.deal_b,
                prec_a: row.prec_a,
                prec_b: row.prec_b,
                n_hints: count("n_hints", row.n_hints)?,
                last_check: row.last_check,
                last_min_price_minor: row.last_min_price_minor,
                checks: count("checks", row.checks)?,
                cost_loads: row.cost_loads,
                rt_discount_ewma: row.rt_discount_ewma,
                rt_discount_n: count("rt_discount_n", row.rt_discount_n)?,
                additive_streak: count("additive_streak", row.additive_streak)?,
                additive: row.additive,
            },
        })
    }
}

// ---------------------------------------------------------------------
// Query log
// ---------------------------------------------------------------------

pub(crate) struct QueryLogRow {
    pub id: i64,
    pub family_id: i64,
    pub route_id: i64,
    pub cabin: String,
    pub source: String,
    pub source_code: Option<String>,
    pub pos_id: i64,
    pub kind: String,
    pub leg: Option<String>,
    pub days: Option<i64>,
    pub site: String,
    pub origin_code: String,
    pub destination_code: String,
    pub sweep: bool,
    pub reason_json: String,
    pub score: Option<f64>,
    pub runner_up: Option<f64>,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome_json: Option<String>,
    pub page_loads: Option<i64>,
    pub cells: i64,
}

impl TryFrom<QueryLogRow> for QueryLogEntry {
    type Error = StoreError;

    fn try_from(row: QueryLogRow) -> Result<Self> {
        let key = family_key(
            row.route_id,
            &row.cabin,
            &row.source,
            row.source_code,
            row.pos_id,
            &row.kind,
            row.leg.as_deref(),
            row.days,
        )?;
        let outcome = row
            .outcome_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        Ok(QueryLogEntry {
            id: QueryId(row.id),
            family: FamilyId(row.family_id),
            key,
            origin_code: row.origin_code,
            destination_code: row.destination_code,
            sweep: row.sweep,
            site: row.site,
            reason: serde_json::from_str(&row.reason_json)?,
            score: row.score,
            runner_up: row.runner_up,
            scheduled_at: row.scheduled_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
            outcome,
            page_loads: opt_count("page_loads", row.page_loads)?,
            cells: count("cells", row.cells)?,
        })
    }
}

// ---------------------------------------------------------------------
// Observations and hints
// ---------------------------------------------------------------------

pub(crate) struct ObservationRow {
    pub id: i64,
    pub query_id: i64,
    pub family_id: i64,
    pub observed_at: DateTime<Utc>,
    pub departure: NaiveDate,
    pub return_date: Option<NaiveDate>,
    pub price_minor: Option<i64>,
    pub currency: Option<String>,
    pub miles: Option<i64>,
    pub carrier: Option<String>,
    pub stops: Option<i64>,
    pub duration_minutes: Option<i64>,
    pub seller: Option<String>,
}

impl TryFrom<ObservationRow> for Observation {
    type Error = StoreError;

    fn try_from(row: ObservationRow) -> Result<Self> {
        Ok(Observation {
            id: Some(ObservationId(row.id)),
            query: QueryId(row.query_id),
            family: FamilyId(row.family_id),
            observed_at: row.observed_at,
            departure: row.departure,
            return_date: row.return_date,
            price: opt_money(
                "observation price",
                row.price_minor,
                row.currency,
            )?,
            miles: row.miles,
            carrier: row.carrier,
            stops: opt_count("stops", row.stops)?,
            duration_minutes: opt_count(
                "duration_minutes",
                row.duration_minutes,
            )?,
            seller: row.seller,
        })
    }
}

pub(crate) struct HintRow {
    pub query_id: i64,
    pub origin_id: i64,
    pub cabin: String,
    pub pos_id: i64,
    pub observed_at: DateTime<Utc>,
    pub destination_freebase_id: String,
    pub destination_airport: String,
    pub price_minor: i64,
    pub currency: String,
    pub departure: Option<NaiveDate>,
    pub return_date: Option<NaiveDate>,
    pub stops: Option<i64>,
    pub duration_minutes: Option<i64>,
    pub carrier: Option<String>,
}

impl TryFrom<HintRow> for ExploreHint {
    type Error = StoreError;

    fn try_from(row: HintRow) -> Result<Self> {
        Ok(ExploreHint {
            query: QueryId(row.query_id),
            origin: PlaceId(row.origin_id),
            cabin: parse_cabin(&row.cabin)?,
            pos: PosId(row.pos_id),
            observed_at: row.observed_at,
            destination_freebase_id: row.destination_freebase_id,
            destination_airport: row.destination_airport,
            price: money(row.price_minor, row.currency),
            departure: row.departure,
            return_date: row.return_date,
            stops: opt_count("stops", row.stops)?,
            duration_minutes: opt_count(
                "duration_minutes",
                row.duration_minutes,
            )?,
            carrier: row.carrier,
        })
    }
}

// ---------------------------------------------------------------------
// Deals
// ---------------------------------------------------------------------

pub(crate) struct DealRow {
    pub id: i64,
    pub route_id: i64,
    pub cabin: String,
    pub pos_id: i64,
    pub departure: NaiveDate,
    pub return_date: NaiveDate,
    pub price_minor: i64,
    pub currency: String,
    pub effective_minor: i64,
    pub effective_currency: String,
    pub baseline_minor: Option<i64>,
    pub baseline_currency: Option<String>,
    pub reason: String,
    pub change: Option<f64>,
    pub estimated: bool,
    pub status: String,
    pub found_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub seen: bool,
    pub carrier: Option<String>,
    pub seller: Option<String>,
    pub url: Option<String>,
}

impl DealRow {
    pub(crate) fn into_deal(self, sellers: Vec<SellerQuote>) -> Result<Deal> {
        Ok(Deal {
            id: Some(DealId(self.id)),
            route: RouteId(self.route_id),
            cabin: parse_cabin(&self.cabin)?,
            pos: PosId(self.pos_id),
            departure: self.departure,
            return_date: self.return_date,
            price: money(self.price_minor, self.currency),
            effective_brl: money(self.effective_minor, self.effective_currency),
            baseline: opt_money(
                "deal baseline",
                self.baseline_minor,
                self.baseline_currency,
            )?,
            reason: parse_deal_reason(&self.reason)?,
            change: self.change,
            estimated: self.estimated,
            status: parse_deal_status(&self.status)?,
            found_at: self.found_at,
            confirmed_at: self.confirmed_at,
            seen: self.seen,
            carrier: self.carrier,
            seller: self.seller,
            sellers,
            url: self.url,
        })
    }
}

pub(crate) struct SellerQuoteRow {
    pub deal_id: i64,
    pub pos_id: i64,
    pub seller: String,
    pub is_airline: bool,
    pub price_minor: i64,
    pub currency: String,
    pub quoted_minor: Option<i64>,
    pub quoted_currency: Option<String>,
}

impl TryFrom<SellerQuoteRow> for SellerQuote {
    type Error = StoreError;

    fn try_from(row: SellerQuoteRow) -> Result<Self> {
        Ok(SellerQuote {
            pos: PosId(row.pos_id),
            seller: row.seller,
            is_airline: row.is_airline,
            price: money(row.price_minor, row.currency),
            quoted: opt_money(
                "quoted price",
                row.quoted_minor,
                row.quoted_currency,
            )?,
        })
    }
}

// ---------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------

pub(crate) struct RequestRow {
    pub id: i64,
    pub payload: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

impl TryFrom<RequestRow> for RequestRecord {
    type Error = StoreError;

    fn try_from(row: RequestRow) -> Result<Self> {
        Ok(RequestRecord {
            id: tm_domain::RequestId(row.id),
            kind: serde_json::from_str(&row.payload)?,
            status: parse_request_status(&row.status)?,
            created_at: row.created_at,
            claimed_at: row.claimed_at,
            finished_at: row.finished_at,
            detail: row.detail,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::records::RequestKind;

    fn serde_name<T: Serialize>(value: &T) -> String {
        match serde_json::to_value(value) {
            Ok(serde_json::Value::String(name)) => name,
            other => panic!("not a unit enum: {other:?}"),
        }
    }

    fn serde_tag<T: Serialize>(value: &T, tag: &str) -> String {
        let value = serde_json::to_value(value).expect("serializes");
        value[tag].as_str().expect("tagged").to_owned()
    }

    #[test]
    fn enum_names_match_serde() {
        for cabin in Cabin::ALL {
            assert_eq!(cabin_name(cabin), serde_name(&cabin));
            assert_eq!(parse_cabin(cabin_name(cabin)).ok(), Some(cabin));
        }
        for leg in [Leg::Outbound, Leg::Return] {
            assert_eq!(leg_name(leg), serde_name(&leg));
        }
        for status in [
            DealStatus::Candidate,
            DealStatus::Confirmed,
            DealStatus::Rejected,
            DealStatus::Expired,
        ] {
            assert_eq!(deal_status_name(status), serde_name(&status));
        }
        for reason in [DealReason::BelowMedian, DealReason::AllTimeLow] {
            assert_eq!(deal_reason_name(reason), serde_name(&reason));
        }
        for state in [
            LoginState::LoggedIn,
            LoginState::LoggedOut,
            LoginState::Expired,
        ] {
            assert_eq!(login_state_name(state), serde_name(&state));
        }
        for status in [
            RequestStatus::Pending,
            RequestStatus::Claimed,
            RequestStatus::Done,
            RequestStatus::Failed,
        ] {
            assert_eq!(request_status_name(status), serde_name(&status));
        }
        for outcome in [
            QueryOutcome::Ok,
            QueryOutcome::Blocked { detail: "x".into() },
            QueryOutcome::Unexpected { detail: "x".into() },
            QueryOutcome::LoggedOut,
            QueryOutcome::Error { detail: "x".into() },
        ] {
            assert_eq!(
                outcome_parts(&outcome).0,
                serde_tag(&outcome, "outcome")
            );
        }
        for kind in [
            RequestKind::CheckRoute { route: RouteId(1) },
            RequestKind::CheckFamily {
                family: FamilyId(1),
            },
            RequestKind::ConfirmDeal { deal: DealId(1) },
            RequestKind::OpenLogin {
                program: "smiles".into(),
            },
            RequestKind::CloseLogin { program: None },
            RequestKind::CloseLogin {
                program: Some("smiles".into()),
            },
            RequestKind::ReloadConfig,
        ] {
            assert_eq!(kind.name(), serde_tag(&kind, "kind"));
        }
    }

    /// An app before 0062 asks to close a login without naming the
    /// program.
    #[test]
    fn an_old_close_login_still_reads() {
        let old: RequestKind =
            serde_json::from_str(r#"{"kind":"close_login"}"#).expect("parses");
        assert_eq!(old, RequestKind::CloseLogin { program: None });
        let new = RequestKind::CloseLogin {
            program: Some("smiles".into()),
        };
        let json = serde_json::to_string(&new).expect("serializes");
        assert_eq!(json, r#"{"kind":"close_login","program":"smiles"}"#);
    }

    #[test]
    fn family_keys_round_trip_through_columns() {
        let kinds = [
            FamilyKind::Leg { leg: Leg::Outbound },
            FamilyKind::Leg { leg: Leg::Return },
            FamilyKind::RoundTrip { days: 7 },
            FamilyKind::Explore,
        ];
        let sources = [
            Source::GoogleFlights,
            Source::Airline { code: "LA".into() },
            Source::Miles {
                program: "smiles".into(),
            },
        ];
        for kind in kinds {
            for source in &sources {
                let key = FamilyKey {
                    route: RouteId(3),
                    cabin: Cabin::Business,
                    source: source.clone(),
                    pos: PosId(2),
                    kind,
                };
                let c = family_columns(&key);
                assert_eq!(serde_tag(&key.kind, "kind"), c.kind);
                assert_eq!(serde_tag(&key.source, "source"), c.source);
                let back = family_key(
                    3,
                    c.cabin,
                    c.source,
                    c.source_code.map(str::to_owned),
                    2,
                    c.kind,
                    c.leg,
                    c.days,
                )
                .expect("parses");
                assert_eq!(back, key);
            }
        }
    }
}
