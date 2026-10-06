//! Which families to query this tick (DECISIONS.md 0028, 0029).
//!
//! Order of picks:
//!
//! 1. **Overdue**: checked longer ago than its maximum interval (the
//!    candidate's own [`Candidate::max_interval`], else `max_interval`),
//!    most overdue first.
//!    first.
//! 2. **Bootstrap**: never checked, lowest [`Candidate::bootstrap_rank`]
//!    first (DECISIONS.md 0049), then the earliest added
//!    ([`Candidate::added`], 0065), then highest weight. Ranks are taken
//!    strictly in order: once a never-checked family does not fit, cheaper
//!    ones of the same rank may still go, but no later rank is
//!    bootstrapped this tick, and never-checked families do not compete
//!    in the scored slots.
//! 3. Every further slot is, with probability `random_share`, a uniformly
//!    random eligible family (**Random**), otherwise the next one in
//!    [`Params::order`]: by default the one whose data is oldest, the
//!    value index only breaking ties (0065); or the highest-scoring one
//!    (0028). Either way it is **Scheduled**, with its score and the
//!    runner-up's.
//!
//! Throughout, a family is eligible only when it is active and was not
//! checked within `min_interval`, and a pick must fit both the tick's
//! remaining page loads and its site's remaining loads, at the family's
//! expected cost ([`FamilyState::expected_loads`]). A family that does
//! not fit is skipped, so a cheaper one can still use the rest. Each
//! family is picked at most once per tick. Given the same inputs and the
//! same RNG state the result is identical.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rand::Rng;
use tm_domain::{FamilyId, QueryReason};

use crate::{Order, Params, score, state::FamilyState};

/// One family the daemon could query.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub family: FamilyId,
    /// The rate-limit bucket ([`tm_domain::Source::site`]).
    pub site: String,
    /// The user's weight `w` (route weight, default 1).
    pub weight: f64,
    pub state: FamilyState,
    /// Inactive families (removed routes) are never picked.
    pub active: bool,
    /// Bootstrap order for never-checked families: lower ranks are
    /// checked first (DECISIONS.md 0049). Ignored once checked.
    pub bootstrap_rank: u8,
    /// A tighter maximum interval for this family (core families are
    /// checked daily, DECISIONS.md 0056); `None` uses the global one.
    pub max_interval: Option<chrono::TimeDelta>,
    /// When the family's route was added, as an increasing number (the
    /// daemon uses the route id): never-checked families of earlier
    /// routes are filled in first (0065).
    pub added: i64,
}

/// What this tick may spend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickBudget {
    /// Page loads this tick may use in total.
    pub loads_available: u32,
    /// Page loads each site may still use. A site that is not in the map
    /// is limited only by `loads_available`; a site at 0 (for example in
    /// a block cooldown) gets nothing.
    pub per_site_remaining: BTreeMap<String, u32>,
}

/// One query to run, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub family: FamilyId,
    pub site: String,
    pub reason: QueryReason,
    /// The page loads this pick was charged against the budget.
    pub expected_loads: u32,
}

/// Choose this tick's queries. See the module documentation for the rules.
pub fn plan_tick<R: Rng + ?Sized>(
    candidates: &[Candidate],
    now: DateTime<Utc>,
    budget: &TickBudget,
    params: &Params,
    rng: &mut R,
) -> Vec<Pick> {
    let mut ledger = Ledger {
        loads: budget.loads_available,
        sites: budget.per_site_remaining.clone(),
    };
    let min_days = params.min_interval.num_seconds() as f64 / 86_400.0;
    let max_days = params.max_interval.num_seconds() as f64 / 86_400.0;

    let mut overdue = Vec::new();
    let mut bootstrap = Vec::new();
    let mut pool = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        if !c.active {
            continue;
        }
        match c.state.last_check {
            None => bootstrap.push(i),
            Some(last) if last > now => {} // clock skew: just checked
            Some(_) => {
                let tau = c.state.elapsed_days(now).unwrap_or(0.0);
                if tau < min_days {
                    continue;
                }
                let max_days = c
                    .max_interval
                    .map_or(max_days, |m| m.num_seconds() as f64 / 86_400.0);
                if tau >= max_days {
                    overdue.push((i, tau));
                } else {
                    pool.push(i);
                }
            }
        }
    }

    let mut picks = Vec::new();
    let mut taken = vec![false; candidates.len()];

    overdue.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then(candidates[a.0].family.cmp(&candidates[b.0].family))
    });
    for (i, _) in &overdue {
        ledger.try_pick(
            &candidates[*i],
            QueryReason::Overdue,
            &mut picks,
            &mut taken[*i],
        );
    }

    bootstrap.sort_by(|&a, &b| {
        let (ca, cb) = (&candidates[a], &candidates[b]);
        ca.bootstrap_rank
            .cmp(&cb.bootstrap_rank)
            .then(ca.added.cmp(&cb.added))
            .then(finite(cb.weight).total_cmp(&finite(ca.weight)))
            .then(ca.family.cmp(&cb.family))
    });
    // A family that does not fit lets cheaper ones of the same rank
    // through, but no later rank: those wait for a later tick.
    let mut blocked_rank = None;
    for &i in &bootstrap {
        let rank = candidates[i].bootstrap_rank;
        if blocked_rank.is_some_and(|blocked| rank > blocked) {
            break;
        }
        let before = picks.len();
        ledger.try_pick(
            &candidates[i],
            QueryReason::Bootstrap,
            &mut picks,
            &mut taken[i],
        );
        if picks.len() == before && blocked_rank.is_none() {
            blocked_rank = Some(rank);
        }
    }

    // Overdue families that did not fit stay eligible for the scored
    // and random slots. Never-checked ones wait for their turn, so a
    // late rank cannot overtake an earlier one through a score.
    pool.extend(overdue.iter().map(|(i, _)| *i).filter(|&i| !taken[i]));
    // Score in a fixed order so Thompson samples are reproducible.
    pool.sort_by_key(|&i| (candidates[i].family, i));
    let mut scored: Vec<(usize, f64)> = pool
        .into_iter()
        .map(|i| {
            let c = &candidates[i];
            (i, score::score(&c.state, c.weight, now, params, rng))
        })
        .collect();
    let by_score = |a: &(usize, f64), b: &(usize, f64)| {
        b.1.total_cmp(&a.1)
            .then(candidates[a.0].family.cmp(&candidates[b.0].family))
    };
    match params.order {
        Order::Value => scored.sort_by(by_score),
        // Oldest check first (never-checked ones are not in the pool).
        Order::Stalest => scored.sort_by(|a, b| {
            let last = |i: usize| candidates[i].state.last_check;
            last(a.0).cmp(&last(b.0)).then_with(|| by_score(a, b))
        }),
    }

    let share = if params.random_share.is_finite() {
        params.random_share.clamp(0.0, 1.0)
    } else {
        0.0
    };
    loop {
        let fitting: Vec<usize> = (0..scored.len())
            .filter(|&k| ledger.fits(&candidates[scored[k].0]))
            .collect();
        let Some(&best) = fitting.first() else {
            break;
        };
        let random = share > 0.0 && rng.random::<f64>() < share;
        let k = if random {
            fitting[rng.random_range(0..fitting.len())]
        } else {
            best
        };
        let (i, s) = scored.remove(k);
        let reason = if random {
            QueryReason::Random
        } else {
            QueryReason::Scheduled {
                score: s,
                runner_up: scored.first().map(|r| r.1),
            }
        };
        ledger.try_pick(&candidates[i], reason, &mut picks, &mut taken[i]);
    }
    picks
}

fn finite(x: f64) -> f64 {
    if x.is_finite() { x } else { 0.0 }
}

struct Ledger {
    loads: u32,
    sites: BTreeMap<String, u32>,
}

impl Ledger {
    fn fits(&self, c: &Candidate) -> bool {
        let cost = c.state.expected_loads();
        cost <= self.loads
            && self.sites.get(&c.site).is_none_or(|&left| cost <= left)
    }

    fn try_pick(
        &mut self,
        c: &Candidate,
        reason: QueryReason,
        picks: &mut Vec<Pick>,
        taken: &mut bool,
    ) {
        if *taken || !self.fits(c) {
            return;
        }
        let cost = c.state.expected_loads();
        self.loads -= cost;
        if let Some(left) = self.sites.get_mut(&c.site) {
            *left -= cost;
        }
        *taken = true;
        picks.push(Pick {
            family: c.family,
            site: c.site.clone(),
            reason,
            expected_loads: cost,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::{TimeDelta, TimeZone};
    use hegel::{TestCase, generators as gs};
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0)
            .single()
            .expect("valid instant")
    }

    fn cand(id: i64, site: &str, hours_ago: Option<i64>) -> Candidate {
        Candidate {
            family: FamilyId(id),
            site: site.to_owned(),
            weight: 1.0,
            state: FamilyState {
                last_check: hours_ago.map(|h| now() - TimeDelta::hours(h)),
                cost_loads: 1.0,
                ..FamilyState::default()
            },
            active: true,
            bootstrap_rank: 0,
            max_interval: None,
            added: id,
        }
    }

    #[test]
    fn stalest_data_goes_first_and_older_routes_fill_in_first() {
        // Two checked families: the one checked longer ago goes first,
        // whatever their scores.
        let fresh = cand(1, "gf", Some(10));
        let stale = cand(2, "gf", Some(40));
        let picks = plan_tick(
            &[fresh, stale],
            now(),
            &budget(1),
            &no_random(),
            &mut StdRng::seed_from_u64(4),
        );
        assert_eq!(picks[0].family.0, 2);
        // Never-checked ones of the same rank: the earliest added first.
        let mut newer = cand(3, "gf", None);
        newer.added = 50;
        let mut older = cand(4, "gf", None);
        older.added = 7;
        let picks = plan_tick(
            &[newer, older],
            now(),
            &budget(1),
            &no_random(),
            &mut StdRng::seed_from_u64(4),
        );
        assert_eq!(picks[0].family.0, 4);
        assert_eq!(picks[0].reason, QueryReason::Bootstrap);
    }

    #[test]
    fn a_family_interval_overrides_the_global_one() {
        // Checked 30 hours ago: within the weekly cap, past a daily one.
        let weekly = cand(1, "gf", Some(30));
        let mut daily = cand(2, "gf", Some(30));
        daily.max_interval = Some(TimeDelta::hours(24));
        let picks = plan_tick(
            &[weekly, daily],
            now(),
            &budget(1),
            &no_random(),
            &mut StdRng::seed_from_u64(3),
        );
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].family.0, 2);
        assert_eq!(picks[0].reason, QueryReason::Overdue);
    }

    #[test]
    fn bootstrap_follows_rank_strictly() {
        // Rank 0 costs 3, rank 1 costs 1: with 4 loads, two rank-0
        // families cannot both fit, and the cheap rank-1 one must not
        // jump ahead of the waiting rank-0 one.
        let mut a = cand(1, "gf", None);
        a.state.cost_loads = 3.0;
        let mut b = cand(2, "gf", None);
        b.state.cost_loads = 3.0;
        let mut c = cand(3, "gf", None);
        c.bootstrap_rank = 1;
        let cands = vec![c, b, a];
        let picks = plan_tick(
            &cands,
            now(),
            &budget(4),
            &no_random(),
            &mut StdRng::seed_from_u64(1),
        );
        let ids: Vec<i64> = picks.iter().map(|p| p.family.0).collect();
        assert_eq!(ids, vec![1]);
    }

    fn budget(loads: u32) -> TickBudget {
        TickBudget {
            loads_available: loads,
            per_site_remaining: BTreeMap::new(),
        }
    }

    fn no_random() -> Params {
        Params {
            random_share: 0.0,
            thompson: false,
            ..Params::default()
        }
    }

    #[test]
    fn overdue_then_bootstrap_then_scored() {
        let cs = vec![
            cand(1, "gf", Some(24)),
            cand(2, "gf", None),
            cand(3, "gf", Some(24 * 8)),
            cand(4, "gf", Some(24 * 3)),
        ];
        let picks = plan_tick(
            &cs,
            now(),
            &budget(10),
            &no_random(),
            &mut StdRng::seed_from_u64(0),
        );
        let ids: Vec<i64> = picks.iter().map(|p| p.family.0).collect();
        assert_eq!(ids, vec![3, 2, 4, 1]);
        assert_eq!(picks[0].reason, QueryReason::Overdue);
        assert_eq!(picks[1].reason, QueryReason::Bootstrap);
        match &picks[2].reason {
            QueryReason::Scheduled { score, runner_up } => {
                assert!(*score > runner_up.unwrap_or(0.0));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            picks[3].reason,
            QueryReason::Scheduled {
                runner_up: None,
                ..
            }
        ));
    }

    #[test]
    fn skips_recent_and_inactive() {
        let mut inactive = cand(2, "gf", None);
        inactive.active = false;
        let cs = vec![cand(1, "gf", Some(1)), inactive];
        let picks = plan_tick(
            &cs,
            now(),
            &budget(10),
            &no_random(),
            &mut StdRng::seed_from_u64(0),
        );
        assert!(picks.is_empty());
    }

    #[test]
    fn site_limit_and_cheaper_family_fills_the_rest() {
        let mut big = cand(1, "gf", None);
        big.state.cost_loads = 5.0;
        let small = cand(2, "gf", None);
        let other = cand(3, "airline:LA", None);
        let b = TickBudget {
            loads_available: 4,
            per_site_remaining: [("airline:LA".to_owned(), 0)].into(),
        };
        let picks = plan_tick(
            &[big, small, other],
            now(),
            &b,
            &no_random(),
            &mut StdRng::seed_from_u64(0),
        );
        let ids: Vec<i64> = picks.iter().map(|p| p.family.0).collect();
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn random_share_one_is_all_random() {
        let cs: Vec<_> = (0..20).map(|i| cand(i, "gf", Some(24))).collect();
        let p = Params {
            random_share: 1.0,
            ..Params::default()
        };
        let picks = plan_tick(
            &cs,
            now(),
            &budget(10),
            &p,
            &mut StdRng::seed_from_u64(3),
        );
        assert_eq!(picks.len(), 10);
        assert!(picks.iter().all(|p| p.reason == QueryReason::Random));
    }

    #[test]
    fn about_ten_percent_random() {
        let cs: Vec<_> = (0..2000).map(|i| cand(i, "gf", Some(24))).collect();
        let picks = plan_tick(
            &cs,
            now(),
            &budget(1000),
            &Params::default(),
            &mut StdRng::seed_from_u64(9),
        );
        let random = picks
            .iter()
            .filter(|p| p.reason == QueryReason::Random)
            .count();
        assert_eq!(picks.len(), 1000);
        assert!((60..=140).contains(&random), "{random}");
    }

    #[test]
    fn future_last_check_is_not_picked() {
        let mut c = cand(1, "gf", Some(0));
        c.state.last_check = Some(now() + TimeDelta::days(30));
        let picks = plan_tick(
            &[c],
            now(),
            &budget(10),
            &no_random(),
            &mut StdRng::seed_from_u64(0),
        );
        assert!(picks.is_empty());
    }

    fn draw_candidates(tc: &TestCase) -> Vec<Candidate> {
        let n = tc.draw(gs::integers::<usize>().max_value(40));
        let sites = ["gf", "airline:LA", "miles:smiles"];
        (0..n)
            .map(|i| {
                let hours = tc.draw(gs::optional(
                    gs::integers::<i64>().min_value(-48).max_value(24 * 20),
                ));
                let site = tc.draw(gs::integers::<usize>().max_value(2));
                let cost =
                    tc.draw(gs::floats::<f64>().min_value(0.0).max_value(8.0));
                let weight =
                    tc.draw(gs::floats::<f64>().min_value(0.0).max_value(5.0));
                let mut c = cand(i as i64, sites[site], hours);
                c.state.cost_loads = cost;
                c.weight = weight;
                c.state.n_hints = tc.draw(gs::integers::<u32>().max_value(3));
                c.active = tc.draw(gs::booleans());
                c
            })
            .collect()
    }

    #[hegel::test(test_cases = 300)]
    fn never_exceeds_budgets_or_picks_recent(tc: TestCase) {
        let cs = draw_candidates(&tc);
        let mut sites = BTreeMap::new();
        for site in ["gf", "airline:LA"] {
            if tc.draw(gs::booleans()) {
                sites.insert(
                    site.to_owned(),
                    tc.draw(gs::integers::<u32>().max_value(15)),
                );
            }
        }
        let b = TickBudget {
            loads_available: tc.draw(gs::integers::<u32>().max_value(40)),
            per_site_remaining: sites,
        };
        let p = Params {
            random_share: tc
                .draw(gs::floats::<f64>().min_value(0.0).max_value(1.0)),
            thompson: tc.draw(gs::booleans()),
            ..Params::default()
        };
        let seed = tc.draw(gs::integers::<u64>());
        let picks =
            plan_tick(&cs, now(), &b, &p, &mut StdRng::seed_from_u64(seed));

        let total: u32 = picks.iter().map(|p| p.expected_loads).sum();
        assert!(total <= b.loads_available);
        for (site, limit) in &b.per_site_remaining {
            let used: u32 = picks
                .iter()
                .filter(|p| &p.site == site)
                .map(|p| p.expected_loads)
                .sum();
            assert!(used <= *limit, "{site}: {used} > {limit}");
        }
        let mut seen = BTreeSet::new();
        for pick in &picks {
            assert!(seen.insert(pick.family), "picked twice");
            let c = cs
                .iter()
                .find(|c| c.family == pick.family)
                .expect("picked from candidates");
            assert!(c.active);
            assert_eq!(pick.expected_loads, c.state.expected_loads());
            if let Some(last) = c.state.last_check {
                assert!(
                    now() - last >= p.min_interval,
                    "picked within min interval"
                );
            }
        }
        // Deterministic given the RNG.
        let again =
            plan_tick(&cs, now(), &b, &p, &mut StdRng::seed_from_u64(seed));
        assert_eq!(picks, again);
    }

    #[hegel::test(test_cases = 200)]
    fn spends_until_nothing_fits(tc: TestCase) {
        let cs = draw_candidates(&tc);
        let b = budget(tc.draw(gs::integers::<u32>().max_value(40)));
        let picks = plan_tick(
            &cs,
            now(),
            &b,
            &no_random(),
            &mut StdRng::seed_from_u64(1),
        );
        let left = b.loads_available
            - picks.iter().map(|p| p.expected_loads).sum::<u32>();
        let picked: BTreeSet<_> = picks.iter().map(|p| p.family).collect();
        for c in &cs {
            let eligible = c.active
                && c.state.last_check.is_none_or(|l| {
                    l <= now() && now() - l >= Params::default().min_interval
                });
            if eligible && !picked.contains(&c.family) {
                assert!(
                    c.state.expected_loads() > left,
                    "{:?} would fit",
                    c.family
                );
            }
        }
    }
}
