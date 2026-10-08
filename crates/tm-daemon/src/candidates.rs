//! Which families the scheduler may pick this tick (DECISIONS.md 0028),
//! and the conversions between the store's records and the
//! scheduler's state.

use std::collections::{BTreeMap, HashMap, HashSet};

use tm_domain::{
    Cabin,
    FamilyId,
    FamilyKey,
    FamilyKind,
    Passengers,
    PosId,
    RouteId,
    Source,
};
use tm_scheduler::{Candidate, FamilyState, RoundTripDiscount};
use tm_store::{FamilyRecord, FamilyStateRecord, RouteTripState};

use crate::sites;

/// The scheduler's view of a stored state.
pub fn to_state(r: &FamilyStateRecord) -> FamilyState {
    FamilyState {
        chg_a: r.chg_a,
        chg_b: r.chg_b,
        deal_a: r.deal_a,
        deal_b: r.deal_b,
        prec_a: r.prec_a,
        prec_b: r.prec_b,
        n_hints: r.n_hints,
        last_check: r.last_check,
        last_min_price_minor: r.last_min_price_minor,
        checks: r.checks,
        cost_loads: r.cost_loads,
    }
}

/// `base` with the scheduler's fields replaced by `s` (the route-level
/// round-trip fields are kept: the store ignores them here).
pub fn to_record(
    s: &FamilyState,
    base: &FamilyStateRecord,
) -> FamilyStateRecord {
    FamilyStateRecord {
        chg_a: s.chg_a,
        chg_b: s.chg_b,
        deal_a: s.deal_a,
        deal_b: s.deal_b,
        prec_a: s.prec_a,
        prec_b: s.prec_b,
        n_hints: s.n_hints,
        last_check: s.last_check,
        last_min_price_minor: s.last_min_price_minor,
        checks: s.checks,
        cost_loads: s.cost_loads,
        ..base.clone()
    }
}

pub fn to_discount(r: &RouteTripState) -> RoundTripDiscount {
    RoundTripDiscount {
        ewma: r.rt_discount_ewma,
        n: r.rt_discount_n,
        additive_streak: r.additive_streak,
        additive: r.additive,
    }
}

pub fn from_discount(d: &RoundTripDiscount) -> RouteTripState {
    RouteTripState {
        rt_discount_ewma: d.ewma,
        rt_discount_n: d.n,
        additive_streak: d.additive_streak,
        additive: d.additive,
    }
}

/// A debugging restriction from the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Only {
    pub family: Option<FamilyId>,
    /// Routes to this destination code (no Explore sweeps).
    pub destination: Option<String>,
}

impl Only {
    pub fn is_set(&self) -> bool {
        self.family.is_some() || self.destination.is_some()
    }
}

/// What the filters need to know besides the families.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Routes whose places Google cannot be told about yet (a city
    /// without its Freebase ID, 0041).
    pub unresolved: HashSet<RouteId>,
    /// Sites in a block cooldown (0039).
    pub cooling: HashSet<String>,
    /// The destination code of each route, for [`Only::destination`];
    /// sweeps map to `None`.
    pub destinations: HashMap<RouteId, Option<String>>,
    /// Sites paused for a login (0024, 0062).
    pub paused: HashSet<String>,
    pub only: Only,
    /// The home point of sale, for bootstrap ranks (0049).
    pub home_pos: Option<PosId>,
    /// The party every search uses (0036): some sites price only one
    /// adult.
    pub passengers: Passengers,
    /// Families whose last check that loaded found no flights (0067).
    pub no_flights: HashSet<FamilyId>,
    /// Miles programs without a live login (logged out, or a session
    /// that ended): their award families wait for the user (0024, 0085).
    pub logged_out: HashSet<String>,
    /// Whether LATAM's round trips may run: only for a manual check of
    /// one family under `--latam-round-trips`, never for the scheduler
    /// (0090).
    pub latam_round_trips: bool,
}

/// Why a family is not a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Skip {
    Inactive,
    Paused,
    CoolingDown,
    Unresolved,
    NotSelected,
    /// An airline or miles site with no reader in the daemon: not
    /// probed yet (0050), or denying its browser (0052). Only TAP has
    /// one ([`sites::scripted`]).
    Unprobed,
    /// A family its site cannot run ([`sites::unsupported`]): TAP's
    /// round trips, LATAM's until its outbound click is verified (0090),
    /// or a party these sites do not price.
    Unsupported,
    /// A miles program that is not logged in, or whose session ended
    /// (0085).
    LoggedOut,
}

/// Whether `family` may be checked now, ignoring the budget and the
/// scheduler's intervals (`plan_tick` applies those).
pub fn eligibility(family: &FamilyRecord, cx: &Context) -> Result<(), Skip> {
    if !family.active {
        return Err(Skip::Inactive);
    }
    if !selected(family, cx) {
        return Err(Skip::NotSelected);
    }
    // A site that shares another's worker shares its login pause and
    // its cooldown (0084).
    let site = sites::worker_site(&family.site);
    if cx.paused.contains(site) {
        return Err(Skip::Paused);
    }
    if cx.cooling.contains(site) {
        return Err(Skip::CoolingDown);
    }
    if !matches!(family.key.source, Source::GoogleFlights)
        && !sites::scripted(&family.key.source)
    {
        return Err(Skip::Unprobed);
    }
    if sites::unsupported_unless(
        &family.key,
        &cx.passengers,
        cx.latam_round_trips,
    )
    .is_some()
    {
        return Err(Skip::Unsupported);
    }
    if let Source::Miles { program } = &family.key.source
        && cx.logged_out.contains(program)
    {
        return Err(Skip::LoggedOut);
    }
    // Scripted sites search by IATA code: they need no Freebase ID.
    if cx.unresolved.contains(&family.key.route)
        && family.key.kind != FamilyKind::Explore
        && !sites::scripted(&family.key.source)
    {
        return Err(Skip::Unresolved);
    }
    Ok(())
}

fn selected(family: &FamilyRecord, cx: &Context) -> bool {
    if let Some(id) = cx.only.family {
        return family.id == id;
    }
    let Some(code) = &cx.only.destination else {
        return true;
    };
    match cx.destinations.get(&family.key.route) {
        Some(Some(dest)) => dest.eq_ignore_ascii_case(code),
        // Explore sweeps have no destination: left out.
        Some(None) | None => false,
    }
}

/// Which never-checked families go first (DECISIONS.md 0049): home
/// point of sale before foreign ones; within each, economy legs and the
/// Explore sweep, then economy round trips, then the premium cabins.
/// Airline and miles sources come last, their legs before their round
/// trips (a LATAM round trip costs a search more, 0085).
pub fn bootstrap_rank(key: &FamilyKey, home_pos: Option<PosId>) -> u8 {
    if !matches!(key.source, Source::GoogleFlights) {
        return match key.kind {
            FamilyKind::RoundTrip { .. } => 7,
            _ => 6,
        };
    }
    let within = match (key.cabin, key.kind) {
        (Cabin::Economy, FamilyKind::Leg { .. } | FamilyKind::Explore) => 0,
        (Cabin::Economy, FamilyKind::RoundTrip { .. }) => 1,
        _ => 2,
    };
    if Some(key.pos) == home_pos {
        within
    } else {
        3 + within
    }
}

/// Core families (rank 0: the home point of sale's economy legs and
/// Explore sweeps on Google Flights) are checked at least daily
/// (DECISIONS.md 0056); the rest keep the global weekly maximum.
pub fn core_interval(
    key: &FamilyKey,
    home_pos: Option<PosId>,
) -> Option<chrono::TimeDelta> {
    (bootstrap_rank(key, home_pos) == 0).then(|| chrono::TimeDelta::hours(24))
}

/// How long a family whose last check found no flights waits before the
/// next one (DECISIONS.md 0067).
pub const NO_FLIGHTS_WAIT: chrono::TimeDelta = chrono::TimeDelta::days(7);

/// A family's own (minimum, maximum) intervals: core families are due
/// daily (0056), unless their last check found no flights, which makes
/// them wait a week and drops the daily cadence (0067).
pub fn intervals(
    family: &FamilyRecord,
    cx: &Context,
) -> (Option<chrono::TimeDelta>, Option<chrono::TimeDelta>) {
    if cx.no_flights.contains(&family.id) {
        (Some(NO_FLIGHTS_WAIT), None)
    } else {
        (None, core_interval(&family.key, cx.home_pos))
    }
}

/// The candidates for `plan_tick`, and how many families each reason
/// left out (for the log). A site's worker passes its own site's
/// families only (0062).
pub fn build<'a>(
    families: impl IntoIterator<Item = &'a FamilyRecord>,
    cx: &Context,
) -> (Vec<Candidate>, BTreeMap<Skip, usize>) {
    let mut out = Vec::new();
    let mut skipped = BTreeMap::new();
    for f in families {
        match eligibility(f, cx) {
            Ok(()) => {
                let (min_interval, max_interval) = intervals(f, cx);
                out.push(Candidate {
                    family: f.id,
                    site: sites::worker_site(&f.site).to_owned(),
                    weight: f.weight,
                    state: to_state(&f.state),
                    active: true,
                    bootstrap_rank: bootstrap_rank(&f.key, cx.home_pos),
                    max_interval,
                    min_interval,
                    // Route ids grow as destinations are added (0065).
                    added: f.key.route.0,
                });
            }
            Err(skip) => *skipped.entry(skip).or_insert(0) += 1,
        }
    }
    (out, skipped)
}

/// Whether `site` has a reader in the daemon: Google Flights and the
/// scripted airline sites (0050, 0051). Other sites only ever pause for
/// a login.
pub fn has_reader(site: &str) -> bool {
    match Source::from_site(site) {
        Some(Source::GoogleFlights) => true,
        Some(source) => sites::scripted(&source),
        None => false,
    }
}

/// Families of a route for a manual "check now" (0046): every eligible
/// one, whatever its interval.
pub fn of_route(
    families: &[FamilyRecord],
    route: RouteId,
    cx: &Context,
) -> Vec<FamilyId> {
    families
        .iter()
        .filter(|f| f.key.route == route && eligibility(f, cx).is_ok())
        .map(|f| f.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use tm_domain::Leg;

    use super::*;

    fn family(id: i64, route: i64, source: Source) -> FamilyRecord {
        let site = source.site();
        FamilyRecord {
            id: FamilyId(id),
            key: FamilyKey {
                route: RouteId(route),
                cabin: Cabin::Economy,
                source,
                pos: PosId(1),
                kind: FamilyKind::Leg { leg: Leg::Outbound },
            },
            active: true,
            weight: 1.0,
            site,
            state: FamilyStateRecord::default(),
        }
    }

    fn cx() -> Context {
        let mut cx = Context::default();
        cx.destinations.insert(RouteId(1), Some("LIS".into()));
        cx.destinations.insert(RouteId(2), Some("MIA".into()));
        cx.destinations.insert(RouteId(9), None);
        cx
    }

    /// Airline and miles sites without a reader are skipped (0052,
    /// 0059), even when their miles program is logged in.
    #[test]
    fn sites_without_a_reader_are_skipped() {
        let families = vec![
            family(1, 1, Source::GoogleFlights),
            family(2, 1, Source::Airline { code: "G3".into() }),
            miles(3, "smiles"),
        ];
        let (cands, skipped) = build(&families, &cx());
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1]);
        assert_eq!(skipped.get(&Skip::Unprobed), Some(&2));
        assert_eq!(skipped.len(), 1);
        assert_eq!(eligibility(&families[2], &cx()), Err(Skip::Unprobed));
    }

    /// The daemon's own readers (0051, 0052, 0059, 0085): TAP, LATAM and
    /// LATAM Pass run; every other airline and miles site stays off.
    #[test]
    fn only_tap_and_latam_have_a_reader() {
        let mut families = vec![
            family(1, 1, Source::GoogleFlights),
            family(2, 1, Source::Airline { code: "TP".into() }),
            family(3, 1, Source::Airline { code: "LA".into() }),
            miles(4, "latam_pass"),
        ];
        let mut id = 10;
        for code in ["AD", "G3", "IB", "AF", "AA", "UA", "EK", "QR", "TK"] {
            families.push(family(id, 1, Source::Airline { code: code.into() }));
            id += 1;
        }
        for program in ["smiles", "azul_fidelidade"] {
            families.push(miles(id, program));
            id += 1;
        }
        let mut cx = Context::default();
        let (cands, skipped) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
        assert_eq!(skipped.get(&Skip::Unprobed), Some(&11));
        assert_eq!(skipped.len(), 1);
        // LATAM's cash site runs on LATAM Pass's worker (0084).
        assert_eq!(cands[2].site, "miles:latam_pass");
        assert_eq!(cands[3].site, "miles:latam_pass");

        // A program without a live login waits; LATAM's cash does not.
        cx.logged_out.insert("latam_pass".into());
        let (cands, skipped) = build(&families[..4], &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(skipped.get(&Skip::LoggedOut), Some(&1));
        // A login window on LATAM Pass's profile pauses LATAM's cash too.
        cx.logged_out.clear();
        cx.paused.insert("miles:latam_pass".into());
        let (cands, skipped) = build(&families[..4], &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(skipped.get(&Skip::Paused), Some(&2));
        cx.paused.clear();

        // TAP's round trips, and legs for a party its calendar does not
        // price, are left out with their own reason.
        let mut rt = family(3, 1, Source::Airline { code: "TP".into() });
        rt.key.kind = FamilyKind::RoundTrip { days: 7 };
        let (cands, skipped) = build(&[rt], &cx);
        assert!(cands.is_empty());
        assert_eq!(skipped.get(&Skip::Unsupported), Some(&1));
        cx.passengers.adults = 2;
        let (cands, skipped) = build(&families[..2], &cx);
        assert_eq!(cands.len(), 1);
        assert_eq!(skipped.get(&Skip::Unsupported), Some(&1));

        // A city Google cannot be told about yet does not hold TAP.
        cx.passengers = Passengers::default();
        cx.unresolved.insert(RouteId(1));
        let (cands, skipped) = build(&families[..2], &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![2]);
        assert_eq!(skipped.get(&Skip::Unresolved), Some(&1));
    }

    #[test]
    fn bootstrap_ranks_put_home_economy_legs_first() {
        let home = Some(PosId(1));
        let key = |cabin, pos, kind| FamilyKey {
            route: RouteId(1),
            cabin,
            source: Source::GoogleFlights,
            pos: PosId(pos),
            kind,
        };
        let leg = FamilyKind::Leg { leg: Leg::Outbound };
        let rt = FamilyKind::RoundTrip { days: 7 };
        assert_eq!(bootstrap_rank(&key(Cabin::Economy, 1, leg), home), 0);
        assert_eq!(
            bootstrap_rank(&key(Cabin::Economy, 1, FamilyKind::Explore), home),
            0
        );
        assert_eq!(bootstrap_rank(&key(Cabin::Economy, 1, rt), home), 1);
        assert_eq!(bootstrap_rank(&key(Cabin::Business, 1, leg), home), 2);
        assert_eq!(bootstrap_rank(&key(Cabin::Economy, 2, leg), home), 3);
        assert_eq!(bootstrap_rank(&key(Cabin::Business, 2, rt), home), 5);
        let mut airline = key(Cabin::Economy, 1, leg);
        airline.source = Source::Airline { code: "LA".into() };
        assert_eq!(bootstrap_rank(&airline, home), 6);
        airline.kind = rt;
        assert_eq!(bootstrap_rank(&airline, home), 7);
        assert_eq!(
            core_interval(&key(Cabin::Economy, 1, leg), home),
            Some(chrono::TimeDelta::hours(24))
        );
        assert_eq!(core_interval(&key(Cabin::Business, 1, leg), home), None);
        assert_eq!(core_interval(&key(Cabin::Economy, 2, leg), home), None);
    }

    /// A family whose last check found no flights waits a week and is
    /// no longer due daily; the others keep their intervals (0067).
    #[test]
    fn no_flights_waits_a_week_without_the_daily_cadence() {
        let families = vec![
            family(1, 1, Source::GoogleFlights),
            family(2, 1, Source::GoogleFlights),
        ];
        let mut cx = cx();
        cx.home_pos = Some(PosId(1));
        cx.no_flights.insert(FamilyId(2));
        let (cands, _) = build(&families, &cx);
        assert_eq!(cands.len(), 2, "still a candidate");
        assert_eq!(cands[0].min_interval, None);
        assert_eq!(cands[0].max_interval, Some(chrono::TimeDelta::hours(24)));
        assert_eq!(cands[1].min_interval, Some(NO_FLIGHTS_WAIT));
        assert_eq!(cands[1].max_interval, None);
    }

    fn miles(id: i64, program: &str) -> FamilyRecord {
        family(
            id,
            1,
            Source::Miles {
                program: program.into(),
            },
        )
    }

    #[test]
    fn filters_inactive_cooling_and_paused() {
        let mut inactive = family(2, 1, Source::GoogleFlights);
        inactive.active = false;
        let families = vec![
            family(1, 1, Source::GoogleFlights),
            inactive,
            family(3, 1, Source::Airline { code: "TP".into() }),
        ];
        let mut cx = cx();
        let (cands, skipped) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1, 3]);
        assert_eq!(skipped.get(&Skip::Inactive), Some(&1));

        cx.cooling.insert("google_flights".into());
        let (cands, skipped) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![3]);
        assert_eq!(skipped.get(&Skip::CoolingDown), Some(&1));

        // A login pauses its own site only (0062).
        cx.paused.insert("airline:TP".into());
        let (cands, skipped) = build(&families, &cx);
        assert!(cands.is_empty());
        assert_eq!(skipped.get(&Skip::Paused), Some(&1));
        assert_eq!(skipped.get(&Skip::CoolingDown), Some(&1));
        cx.cooling.clear();
        let (cands, _) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn unresolved_routes_wait_but_sweeps_do_not() {
        let mut sweep = family(7, 9, Source::GoogleFlights);
        sweep.key.kind = FamilyKind::Explore;
        let families = vec![family(1, 1, Source::GoogleFlights), sweep];
        let mut cx = cx();
        cx.unresolved.insert(RouteId(1));
        cx.unresolved.insert(RouteId(9));
        let (cands, skipped) = build(&families, &cx);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].family, FamilyId(7));
        assert_eq!(skipped.get(&Skip::Unresolved), Some(&1));
    }

    #[test]
    fn debugging_restrictions() {
        let families = vec![
            family(1, 1, Source::GoogleFlights),
            family(2, 2, Source::GoogleFlights),
            family(3, 9, Source::GoogleFlights),
        ];
        let mut cx = cx();
        cx.only.family = Some(FamilyId(2));
        let (cands, _) = build(&families, &cx);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].family, FamilyId(2));

        cx.only = Only {
            family: None,
            destination: Some("lis".into()),
        };
        let (cands, _) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1]);
        assert_eq!(of_route(&families, RouteId(2), &cx), vec![]);
        cx.only = Only::default();
        assert_eq!(of_route(&families, RouteId(2), &cx), vec![FamilyId(2)]);
    }

    #[test]
    fn state_round_trips() {
        let mut base = FamilyStateRecord {
            rt_discount_ewma: 0.1,
            additive: true,
            ..FamilyStateRecord::default()
        };
        base.n_hints = 3;
        let mut s = to_state(&base);
        s.checks = 9;
        let back = to_record(&s, &base);
        assert_eq!(back.checks, 9);
        assert_eq!(back.n_hints, 3);
        assert_eq!(back.rt_discount_ewma, 0.1);
        assert!(back.additive);
        let d = to_discount(&base.route_trip());
        assert_eq!(from_discount(&d), base.route_trip());
    }
}
