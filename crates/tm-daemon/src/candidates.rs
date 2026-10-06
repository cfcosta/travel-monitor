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
    /// Jev is configured (airline and miles sources need it, 0041).
    pub has_jev: bool,
    /// Miles programs with a working session (0024).
    pub logged_in: HashSet<String>,
    /// Routes whose places Google cannot be told about yet (a city
    /// without its Freebase ID, 0041).
    pub unresolved: HashSet<RouteId>,
    /// Sites in a block cooldown (0039).
    pub cooling: HashSet<String>,
    /// The destination code of each route, for [`Only::destination`];
    /// sweeps map to `None`.
    pub destinations: HashMap<RouteId, Option<String>>,
    /// The browser is paused for a login.
    pub paused: bool,
    pub only: Only,
    /// The home point of sale, for bootstrap ranks (0049).
    pub home_pos: Option<PosId>,
    /// Airline and miles sites whose extraction has been built on a
    /// probe of the site (0050). Others are skipped. The daemon uses
    /// [`sites::verified_sites`].
    pub verified_sites: HashSet<String>,
    /// The party every search uses (0036): some sites price only one
    /// adult.
    pub passengers: Passengers,
}

/// Why a family is not a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Skip {
    Inactive,
    Paused,
    CoolingDown,
    NoJev,
    LoggedOut,
    Unresolved,
    NotSelected,
    /// An airline or miles site not probed yet (0050).
    Unprobed,
    /// A family its site cannot run ([`sites::unsupported`]): TAP's
    /// round trips, or a party TAP's calendar does not price.
    Unsupported,
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
    if cx.paused {
        return Err(Skip::Paused);
    }
    if cx.cooling.contains(&family.site) {
        return Err(Skip::CoolingDown);
    }
    if !matches!(family.key.source, Source::GoogleFlights)
        && !cx.verified_sites.contains(&family.site)
    {
        return Err(Skip::Unprobed);
    }
    if sites::unsupported(&family.key, &cx.passengers).is_some() {
        return Err(Skip::Unsupported);
    }
    match &family.key.source {
        Source::GoogleFlights => {}
        source @ Source::Airline { .. } if sites::scripted(source) => {}
        Source::Airline { .. } if !cx.has_jev => return Err(Skip::NoJev),
        Source::Airline { .. } => {}
        Source::Miles { .. } if !cx.has_jev => return Err(Skip::NoJev),
        Source::Miles { program } => {
            if !cx.logged_in.contains(program) {
                return Err(Skip::LoggedOut);
            }
        }
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
/// Airline and miles sources come last.
pub fn bootstrap_rank(key: &FamilyKey, home_pos: Option<PosId>) -> u8 {
    if !matches!(key.source, Source::GoogleFlights) {
        return 6;
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

/// The candidates for `plan_tick`, and how many families each reason
/// left out (for the log).
pub fn build(
    families: &[FamilyRecord],
    cx: &Context,
) -> (Vec<Candidate>, BTreeMap<Skip, usize>) {
    let mut out = Vec::new();
    let mut skipped = BTreeMap::new();
    for f in families {
        match eligibility(f, cx) {
            Ok(()) => out.push(Candidate {
                family: f.id,
                site: f.site.clone(),
                weight: f.weight,
                state: to_state(&f.state),
                active: true,
                bootstrap_rank: bootstrap_rank(&f.key, cx.home_pos),
                max_interval: core_interval(&f.key, cx.home_pos),
            }),
            Err(skip) => *skipped.entry(skip).or_insert(0) += 1,
        }
    }
    (out, skipped)
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
        let mut cx = Context {
            has_jev: true,
            ..Context::default()
        };
        cx.destinations.insert(RouteId(1), Some("LIS".into()));
        cx.destinations.insert(RouteId(2), Some("MIA".into()));
        cx.destinations.insert(RouteId(9), None);
        // The filter tests below are about logins and Jev, not probes.
        for site in ["airline:LA", "miles:smiles", "miles:latam_pass"] {
            cx.verified_sites.insert(site.into());
        }
        cx
    }

    #[test]
    fn unprobed_sites_are_skipped() {
        let families = vec![
            family(1, 1, Source::GoogleFlights),
            family(2, 1, Source::Airline { code: "TP".into() }),
        ];
        let mut cx = cx();
        cx.verified_sites.clear();
        let (cands, skipped) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1]);
        assert_eq!(skipped.get(&Skip::Unprobed), Some(&1));
    }

    /// The daemon's own list (0051, 0052): TAP runs, without Jev; every
    /// other airline and miles site stays off.
    #[test]
    fn only_tap_is_verified() {
        let mut families = vec![
            family(1, 1, Source::GoogleFlights),
            family(2, 1, Source::Airline { code: "TP".into() }),
        ];
        let mut id = 10;
        for code in ["LA", "AD", "G3", "IB", "AF", "AA", "UA", "EK", "QR", "TK"]
        {
            families.push(family(id, 1, Source::Airline { code: code.into() }));
            id += 1;
        }
        for program in ["smiles", "latam_pass", "azul_fidelidade"] {
            families.push(miles(id, program));
            id += 1;
        }
        let mut cx = Context {
            verified_sites: sites::verified_sites(),
            ..Context::default()
        };
        cx.logged_in.insert("smiles".into());
        assert!(!cx.has_jev);
        let (cands, skipped) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(skipped.get(&Skip::Unprobed), Some(&13));
        assert_eq!(skipped.len(), 1);

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
        assert_eq!(
            core_interval(&key(Cabin::Economy, 1, leg), home),
            Some(chrono::TimeDelta::hours(24))
        );
        assert_eq!(core_interval(&key(Cabin::Business, 1, leg), home), None);
        assert_eq!(core_interval(&key(Cabin::Economy, 2, leg), home), None);
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
    fn filters_inactive_cooling_jev_and_logins() {
        let mut inactive = family(2, 1, Source::GoogleFlights);
        inactive.active = false;
        let families = vec![
            family(1, 1, Source::GoogleFlights),
            inactive,
            family(3, 1, Source::Airline { code: "LA".into() }),
            miles(4, "smiles"),
            miles(5, "latam_pass"),
        ];
        let mut cx = cx();
        cx.logged_in.insert("smiles".into());
        let (cands, skipped) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![1, 3, 4]);
        assert_eq!(skipped.get(&Skip::Inactive), Some(&1));
        assert_eq!(skipped.get(&Skip::LoggedOut), Some(&1));

        cx.has_jev = false;
        let (cands, skipped) = build(&families, &cx);
        assert_eq!(cands.len(), 1);
        assert_eq!(skipped.get(&Skip::NoJev), Some(&3));

        cx.has_jev = true;
        cx.cooling.insert("google_flights".into());
        let (cands, _) = build(&families, &cx);
        let ids: Vec<i64> = cands.iter().map(|c| c.family.0).collect();
        assert_eq!(ids, vec![3, 4]);

        cx.paused = true;
        let (cands, skipped) = build(&families, &cx);
        assert!(cands.is_empty());
        assert_eq!(skipped.get(&Skip::Paused), Some(&4));
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
