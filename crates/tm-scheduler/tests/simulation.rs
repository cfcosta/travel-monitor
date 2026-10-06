//! A week of simulated ticks: the greedy scheduler against round-robin
//! with the same budget.
//!
//! Each family's price changes at the events of a Poisson process with
//! its own true rate (from once in 20 days to 4 times a day). At each
//! change the new price is a deal (25% under the usual fare) with
//! probability 0.3, otherwise a regular fare. A deal lasts until the next
//! change. A deal is *caught* when some check lands while it lasts.
//!
//! Both policies start knowing nothing, run hourly ticks with the same
//! page-load budget and the same 6-hour minimum interval, and see the same
//! price paths. The greedy policy learns change rates from its own checks
//! (`update_after_check`) and plans with `plan_tick` (10% random checks,
//! Thompson sampling).

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use rand::{Rng, SeedableRng, rngs::StdRng};
use tm_domain::{FamilyId, QueryReason};
use tm_scheduler::{
    Candidate,
    CheckSummary,
    FamilyState,
    Order,
    Params,
    TickBudget,
    plan_tick,
    update_after_check,
};

const FAMILIES: usize = 60;
const HOURS: i64 = 24 * 7;
const LOADS_PER_TICK: u32 = 3;
const RATES_PER_DAY: [f64; 6] = [0.05, 0.2, 0.5, 1.0, 2.0, 4.0];
const DEAL_SHARE: f64 = 0.3;
const BASE: i64 = 100_000;

struct Path {
    /// Change instants, hours; segment `k` is `[starts[k], starts[k+1])`.
    starts: Vec<f64>,
    prices: Vec<i64>,
    deal: Vec<bool>,
}

impl Path {
    fn simulate(rate_per_day: f64, rng: &mut StdRng) -> Self {
        let rate_per_hour = rate_per_day / 24.0;
        let mut starts = vec![f64::NEG_INFINITY];
        let mut prices = vec![BASE];
        let mut deal = vec![false];
        let mut t = 0.0;
        loop {
            t += -(1.0 - rng.random::<f64>()).ln() / rate_per_hour;
            if t > HOURS as f64 + 1.0 {
                break;
            }
            let is_deal = rng.random::<f64>() < DEAL_SHARE;
            let price = if is_deal {
                BASE * 3 / 4
            } else {
                (BASE as f64 * rng.random_range(0.95..1.25)) as i64
            };
            starts.push(t);
            prices.push(price);
            deal.push(is_deal);
        }
        Self {
            starts,
            prices,
            deal,
        }
    }

    fn segment_at(&self, hour: f64) -> usize {
        self.starts.partition_point(|&s| s <= hour) - 1
    }

    fn deals(&self) -> usize {
        self.deal.iter().filter(|&&d| d).count()
    }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0)
        .single()
        .expect("valid instant")
}

fn fresh_state() -> FamilyState {
    FamilyState {
        cost_loads: 1.0,
        ..FamilyState::default()
    }
}

/// Deals caught by a policy, given each family's check hours.
fn caught(paths: &[Path], checks: &[Vec<f64>]) -> usize {
    paths
        .iter()
        .zip(checks)
        .map(|(path, hours)| {
            let mut seen: Vec<usize> = hours
                .iter()
                .map(|&h| path.segment_at(h))
                .filter(|&k| path.deal[k])
                .collect();
            seen.dedup();
            seen.len()
        })
        .sum()
}

fn run_greedy(paths: &[Path], seed: u64) -> Vec<Vec<f64>> {
    // The research's value index (0028), not the default stalest-first
    // order (0065): this test is about the index.
    let params = Params {
        order: Order::Value,
        ..Params::default()
    };
    let mut rng = StdRng::seed_from_u64(seed);
    let mut states: Vec<FamilyState> =
        (0..paths.len()).map(|_| fresh_state()).collect();
    let mut checks = vec![Vec::new(); paths.len()];
    let budget = TickBudget {
        loads_available: LOADS_PER_TICK,
        per_site_remaining: BTreeMap::new(),
    };
    for hour in 0..HOURS {
        let now = t0() + TimeDelta::hours(hour);
        let candidates: Vec<Candidate> = states
            .iter()
            .enumerate()
            .map(|(i, s)| Candidate {
                family: FamilyId(i as i64),
                site: "google_flights".to_owned(),
                weight: 1.0,
                state: s.clone(),
                active: true,
                bootstrap_rank: 0,
                max_interval: None,
                added: i as i64,
            })
            .collect();
        for pick in plan_tick(&candidates, now, &budget, &params, &mut rng) {
            let i = pick.family.0 as usize;
            let path = &paths[i];
            let price = path.prices[path.segment_at(hour as f64)];
            checks[i].push(hour as f64);
            update_after_check(
                &mut states[i],
                &CheckSummary {
                    min_price_minor: Some(price),
                    any_material_change: false,
                    drop: false,
                    page_loads: 1,
                    reason: pick.reason,
                },
                now,
            );
        }
    }
    checks
}

fn run_round_robin(paths: &[Path]) -> Vec<Vec<f64>> {
    let min_interval = Params::default().min_interval.num_hours() as f64;
    let mut last: Vec<Option<f64>> = vec![None; paths.len()];
    let mut checks = vec![Vec::new(); paths.len()];
    for hour in 0..HOURS {
        let now = hour as f64;
        let mut order: Vec<usize> = (0..paths.len())
            .filter(|&i| last[i].is_none_or(|l| now - l >= min_interval))
            .collect();
        order.sort_by(|&a, &b| {
            let key = |i: usize| last[i].unwrap_or(f64::NEG_INFINITY);
            key(a).total_cmp(&key(b)).then(a.cmp(&b))
        });
        for &i in order.iter().take(LOADS_PER_TICK as usize) {
            last[i] = Some(now);
            checks[i].push(now);
        }
    }
    checks
}

#[test]
fn greedy_catches_more_drops_than_round_robin() {
    let mut totals = (0usize, 0usize, 0usize); // deals, greedy, round-robin
    let mut by_rate: BTreeMap<String, (usize, usize, usize, usize)> =
        BTreeMap::new();
    for seed in 0..8u64 {
        let mut rng = StdRng::seed_from_u64(1000 + seed);
        let rates: Vec<f64> = (0..FAMILIES)
            .map(|i| RATES_PER_DAY[i % RATES_PER_DAY.len()])
            .collect();
        let paths: Vec<Path> =
            rates.iter().map(|&r| Path::simulate(r, &mut rng)).collect();
        let greedy = run_greedy(&paths, seed);
        let rr = run_round_robin(&paths);

        // Same budget: neither spends more than the ticks allow.
        let spent = |c: &[Vec<f64>]| c.iter().map(Vec::len).sum::<usize>();
        assert!(spent(&greedy) <= (HOURS as usize) * LOADS_PER_TICK as usize);
        assert!(spent(&rr) <= (HOURS as usize) * LOADS_PER_TICK as usize);

        let deals: usize = paths.iter().map(Path::deals).sum();
        totals.0 += deals;
        totals.1 += caught(&paths, &greedy);
        totals.2 += caught(&paths, &rr);
        for (i, rate) in rates.iter().enumerate() {
            let e = by_rate.entry(format!("{rate:>4}/day")).or_default();
            e.0 += paths[i].deals();
            e.1 += caught(&paths[i..=i], &greedy[i..=i]);
            e.2 += caught(&paths[i..=i], &rr[i..=i]);
            e.3 += greedy[i].len();
        }
    }
    eprintln!(
        "deals {}, greedy caught {} ({:.1}%), round-robin caught {} ({:.1}%)",
        totals.0,
        totals.1,
        100.0 * totals.1 as f64 / totals.0 as f64,
        totals.2,
        100.0 * totals.2 as f64 / totals.0 as f64,
    );
    for (rate, (deals, g, rr, checks)) in &by_rate {
        eprintln!(
            "  {rate}: deals {deals:>4}, greedy {g:>4} ({checks:>4} checks), round-robin {rr:>4}"
        );
    }
    // Measured: 596 vs 561 (+6%) in this first week, mostly spent learning
    // from about 8 checks per family; with the true rates known the same
    // index catches 728 (+30%), and over four weeks the learner's lead
    // grows to +14%.
    assert!(
        totals.1 > totals.2,
        "greedy {} vs round-robin {}",
        totals.1,
        totals.2
    );
}

/// Every pick's reason is one the plan documents, and random picks are
/// about the configured share once bootstrap is over.
#[test]
fn reasons_over_a_week() {
    let mut rng = StdRng::seed_from_u64(77);
    let paths: Vec<Path> = (0..FAMILIES)
        .map(|i| {
            Path::simulate(RATES_PER_DAY[i % RATES_PER_DAY.len()], &mut rng)
        })
        .collect();
    // The research's value index (0028), not the default stalest-first
    // order (0065): this test is about the index.
    let params = Params {
        order: Order::Value,
        ..Params::default()
    };
    let mut states: Vec<FamilyState> =
        (0..FAMILIES).map(|_| fresh_state()).collect();
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let budget = TickBudget {
        loads_available: LOADS_PER_TICK,
        per_site_remaining: BTreeMap::new(),
    };
    for hour in 0..HOURS {
        let now = t0() + TimeDelta::hours(hour);
        let candidates: Vec<Candidate> = states
            .iter()
            .enumerate()
            .map(|(i, s)| Candidate {
                family: FamilyId(i as i64),
                site: "google_flights".to_owned(),
                weight: 1.0,
                state: s.clone(),
                active: true,
                bootstrap_rank: 0,
                max_interval: None,
                added: i as i64,
            })
            .collect();
        for pick in plan_tick(&candidates, now, &budget, &params, &mut rng) {
            *counts.entry(pick.reason.label()).or_default() += 1;
            let i = pick.family.0 as usize;
            let price = paths[i].prices[paths[i].segment_at(hour as f64)];
            update_after_check(
                &mut states[i],
                &CheckSummary {
                    min_price_minor: Some(price),
                    any_material_change: false,
                    drop: false,
                    page_loads: 1,
                    reason: pick.reason.clone(),
                },
                now,
            );
            assert!(!matches!(
                pick.reason,
                QueryReason::Hint | QueryReason::FanOut | QueryReason::Manual
            ));
        }
    }
    eprintln!("reasons: {counts:?}");
    assert_eq!(counts.get("bootstrap").copied(), Some(FAMILIES));
    let random = counts.get("random").copied().unwrap_or(0) as f64;
    let scheduled = counts.get("scheduled").copied().unwrap_or(0) as f64;
    let share = random / (random + scheduled);
    assert!((0.05..0.16).contains(&share), "random share {share}");
}
