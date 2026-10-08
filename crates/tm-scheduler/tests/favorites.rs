//! Favorites are checked about twice as often, and the rest still get
//! their turn (DECISIONS.md 0094).
//!
//! Sixty families share one site; ten of them belong to favorite
//! destinations. Every family has been checked once, at staggered
//! times, so the run starts in the steady state rather than in
//! bootstrap. Each hour's tick plans with the default parameters
//! (stalest first, 10% random checks, Thompson sampling) and every check
//! costs one load. The budget is the limit: with `N` families, `F`
//! favorites and `B` checks an hour, stalest-first with a boost of 2
//! settles at a cycle of `T = (N + F) / B` hours for the others and
//! `T / 2` for the favorites.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use rand::{SeedableRng, rngs::StdRng};
use tm_domain::FamilyId;
use tm_scheduler::{Candidate, FamilyState, Params, TickBudget, plan_tick};

const FAMILIES: usize = 60;
const FAVORITES: usize = 10;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 8, 0, 0, 0)
        .single()
        .expect("valid instant")
}

/// The hours at which each family was checked, after `hours` hourly
/// ticks of `per_tick` loads, with `max_interval` as every family's own
/// maximum (`None`: the global week).
fn run(
    hours: i64,
    per_tick: u32,
    max_interval: Option<TimeDelta>,
    seed: u64,
) -> Vec<Vec<i64>> {
    let params = Params::default();
    let mut rng = StdRng::seed_from_u64(seed);
    // Checked once in the 70 hours before the start, spread evenly.
    let mut last: Vec<DateTime<Utc>> = (0..FAMILIES)
        .map(|i| {
            t0() - TimeDelta::minutes(70 * 60 * i as i64 / FAMILIES as i64)
        })
        .collect();
    let mut checks = vec![Vec::new(); FAMILIES];
    let budget = TickBudget {
        loads_available: per_tick,
        per_site_remaining: BTreeMap::new(),
    };
    for hour in 0..hours {
        let now = t0() + TimeDelta::hours(hour);
        let candidates: Vec<Candidate> = (0..FAMILIES)
            .map(|i| Candidate {
                family: FamilyId(i as i64),
                site: "google_flights".to_owned(),
                weight: 1.0,
                state: FamilyState {
                    last_check: Some(last[i]),
                    checks: 1,
                    cost_loads: 1.0,
                    ..FamilyState::default()
                },
                active: true,
                bootstrap_rank: 0,
                max_interval,
                min_interval: None,
                added: i as i64,
                favorite: i < FAVORITES,
            })
            .collect();
        let picks = plan_tick(&candidates, now, &budget, &params, &mut rng);
        assert!(picks.len() <= per_tick as usize, "over the budget");
        for pick in picks {
            let i = pick.family.0 as usize;
            last[i] = now;
            checks[i].push(hour);
        }
    }
    checks
}

fn mean(counts: impl Iterator<Item = usize>) -> f64 {
    let v: Vec<usize> = counts.collect();
    v.iter().sum::<usize>() as f64 / v.len() as f64
}

/// The longest wait between two checks of a family, in hours, from its
/// first check at or after `from` to the end.
fn longest_gap(hours: &[i64], from: i64, end: i64) -> i64 {
    let mut prev = None;
    let mut longest = 0;
    for &h in hours.iter().filter(|&&h| h >= from).chain([&end]) {
        if let Some(p) = prev {
            longest = longest.max(h - p);
        }
        prev = Some(h);
    }
    longest
}

#[test]
fn favorites_get_about_twice_the_checks() {
    let hours = 24 * 28;
    let (mut fav, mut other) = (0.0, 0.0);
    let mut fewest_other = usize::MAX;
    let mut longest_other = 0;
    let seeds = 4;
    for seed in 0..seeds {
        let checks = run(hours, 1, None, seed);
        fav += mean(checks[..FAVORITES].iter().map(Vec::len));
        other += mean(checks[FAVORITES..].iter().map(Vec::len));
        fewest_other = fewest_other
            .min(checks[FAVORITES..].iter().map(Vec::len).min().unwrap_or(0));
        longest_other = longest_other.max(
            checks[FAVORITES..]
                .iter()
                .map(|c| longest_gap(c, 0, hours))
                .max()
                .unwrap_or(0),
        );
    }
    let (fav, other) = (fav / seeds as f64, other / seeds as f64);
    let ratio = fav / other;
    eprintln!(
        "4 weeks, 1 check an hour: a favorite {fav:.1} checks, another \
         family {other:.1} ({ratio:.2}×); fewest {fewest_other}, longest \
         wait {longest_other} h"
    );
    // The ideal is 2×; the 10% random checks, spread evenly over every
    // family, pull it a little under (measured: 1.87×).
    assert!((1.7..=2.2).contains(&ratio), "ratio {ratio}");
    // Nobody starves: (60 + 10) families on 1 check an hour is a
    // 70-hour cycle for the others, 9.6 checks in four weeks, and none
    // waits past the weekly maximum.
    assert!(other > 8.5, "others {other}");
    assert!(fewest_other >= 7, "fewest {fewest_other}");
    assert!(longest_other <= 24 * 7, "longest wait {longest_other} h");
}

/// Core families are due daily (0056); a favorite's maximum is halved
/// to 12 hours. With a budget that covers both, every other family is
/// still checked at least daily.
#[test]
fn daily_families_stay_daily_beside_favorites() {
    let hours = 24 * 14;
    // Daily need: 50 others once and 10 favorites twice, 70 a day, about
    // 2.9 an hour: 3 an hour covers it.
    let checks = run(hours, 3, Some(TimeDelta::hours(24)), 11);
    let fav = mean(checks[..FAVORITES].iter().map(Vec::len));
    let other = mean(checks[FAVORITES..].iter().map(Vec::len));
    // After the first two days, which clear the start's backlog (most
    // families start more than a day old).
    let fav_wait = checks[..FAVORITES]
        .iter()
        .map(|c| longest_gap(c, 48, hours))
        .max()
        .unwrap_or(0);
    let other_wait = checks[FAVORITES..]
        .iter()
        .map(|c| longest_gap(c, 48, hours))
        .max()
        .unwrap_or(0);
    eprintln!(
        "2 weeks, 3 checks an hour, daily maximum: a favorite {fav:.1} \
         checks (longest wait {fav_wait} h), another family {other:.1} \
         (longest wait {other_wait} h)"
    );
    assert!(fav / other > 1.7, "{fav} vs {other}");
    // An hourly tick can land a check up to an hour late.
    assert!(fav_wait <= 13, "favorite waited {fav_wait} h");
    assert!(other_wait <= 25, "another family waited {other_wait} h");
}
