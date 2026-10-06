//! The daily page-load budget (DECISIONS.md 0039).
//!
//! - Start at `daily_budget` (600 by default, DECISIONS.md 0049).
//! - Every full 24 hours without a block anywhere raises it by
//!   `budget_ramp` (10%, at least one load), up to `max_daily_budget`.
//! - A block (CAPTCHA, "unusual traffic", a block page) halves it, never
//!   below `min_daily_budget`, restarts the 24-hour clock, and puts the
//!   blocked site in a cooldown: `block_cooldown` (2 h) doubled for each
//!   further block within `backoff_reset` (72 h) of the previous one, up
//!   to `max_block_cooldown` (48 h).
//! - [`BudgetState::tick_allowance`] spreads what is left of today's
//!   budget over the ticks left, with random jitter so loads are not
//!   evenly spaced.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::Params;

/// Backoff state of one site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteBackoff {
    /// Consecutive blocks (each within `backoff_reset` of the previous).
    pub level: u32,
    pub last_block: DateTime<Utc>,
    /// No loads on this site before this instant.
    pub cooldown_until: DateTime<Utc>,
}

/// The budget state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetState {
    /// Page loads per day right now.
    pub daily_budget: u32,
    /// Start of the current block-free stretch, advanced by a day per
    /// ramp step.
    pub clean_since: DateTime<Utc>,
    pub sites: BTreeMap<String, SiteBackoff>,
}

impl BudgetState {
    pub fn new(params: &Params, now: DateTime<Utc>) -> Self {
        Self {
            daily_budget: params.daily_budget,
            clean_since: now,
            sites: BTreeMap::new(),
        }
    }

    /// Apply every ramp step due by `now`. Idempotent for the same `now`.
    pub fn advance(&mut self, now: DateTime<Utc>, params: &Params) {
        let (budget, since) = self.ramped(now, params);
        self.daily_budget = budget;
        self.clean_since = since;
    }

    /// The budget and stretch start after the ramp steps due by `now`,
    /// without changing `self`.
    fn ramped(
        &self,
        now: DateTime<Utc>,
        params: &Params,
    ) -> (u32, DateTime<Utc>) {
        let day = TimeDelta::days(1);
        let mut budget = self.daily_budget;
        let mut since = self.clean_since;
        let max = params.max_daily_budget.max(params.min_daily_budget);
        while now - since >= day {
            since += day;
            if budget >= max {
                // Already at the cap: just move the clock to now.
                let days = (now - since).num_days();
                since += TimeDelta::days(days);
                break;
            }
            budget = ramp(budget, params.budget_ramp).min(max);
        }
        (budget.min(max), since)
    }

    /// The budget in force at `now`, counting ramp steps not yet applied.
    pub fn budget_at(&self, now: DateTime<Utc>, params: &Params) -> u32 {
        self.ramped(now, params).0
    }

    /// Record a block on `site` at `now`.
    pub fn on_block(
        &mut self,
        site: &str,
        now: DateTime<Utc>,
        params: &Params,
    ) {
        self.advance(now, params);
        self.daily_budget =
            (self.daily_budget / 2).max(params.min_daily_budget);
        self.clean_since = now;
        let level = match self.sites.get(site) {
            Some(b) if now - b.last_block <= params.backoff_reset => {
                b.level.saturating_add(1)
            }
            _ => 1,
        };
        let cooldown = cooldown_for(level, params);
        self.sites.insert(
            site.to_owned(),
            SiteBackoff {
                level,
                last_block: now,
                cooldown_until: now + cooldown,
            },
        );
    }

    /// Whether `site` may be loaded at `now`.
    pub fn site_available(&self, site: &str, now: DateTime<Utc>) -> bool {
        self.sites.get(site).is_none_or(|b| now >= b.cooldown_until)
    }

    /// When `site`'s cooldown ends, if it is cooling down at `now`.
    pub fn cooldown_until(
        &self,
        site: &str,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        self.sites
            .get(site)
            .map(|b| b.cooldown_until)
            .filter(|&until| until > now)
    }

    /// Page loads this tick may use: what is left of today's budget
    /// divided by `ticks_left_today` (this one included), times a uniform
    /// jitter in `1 ± pacing_jitter`, rounded stochastically so the
    /// expected total is right even when the share is a fraction of a
    /// load. Never more than what is left.
    pub fn tick_allowance<R: Rng + ?Sized>(
        &self,
        now: DateTime<Utc>,
        loads_today: u32,
        ticks_left_today: u32,
        params: &Params,
        rng: &mut R,
    ) -> u32 {
        let left = self.budget_at(now, params).saturating_sub(loads_today);
        if left == 0 {
            return 0;
        }
        if ticks_left_today <= 1 {
            return left;
        }
        let j = params.pacing_jitter.clamp(0.0, 1.0);
        let factor = if j > 0.0 {
            rng.random_range(1.0 - j..=1.0 + j)
        } else {
            1.0
        };
        let share = f64::from(left) / f64::from(ticks_left_today) * factor;
        let whole = share.floor();
        let extra = if rng.random::<f64>() < share - whole {
            1.0
        } else {
            0.0
        };
        ((whole + extra) as u32).min(left)
    }
}

fn ramp(budget: u32, step: f64) -> u32 {
    // The epsilon keeps 300 × 1.1 = 330.000…01 from rounding up to 331.
    let raised = (f64::from(budget) * (1.0 + step.max(0.0)) - 1e-9).ceil();
    let raised = if raised.is_finite() {
        raised.min(f64::from(u32::MAX)) as u32
    } else {
        budget
    };
    raised.max(budget.saturating_add(1))
}

fn cooldown_for(level: u32, params: &Params) -> TimeDelta {
    let doublings = level.saturating_sub(1).min(30);
    let base = params.block_cooldown.num_seconds().max(0);
    let secs = base.saturating_mul(1i64 << doublings);
    TimeDelta::seconds(secs.min(params.max_block_cooldown.num_seconds().max(0)))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use hegel::{TestCase, generators as gs};
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    /// These tests were written around a 300-load start (0039); the
    /// default is now 600 (0049).
    fn params_300() -> Params {
        Params {
            daily_budget: 300,
            ..Params::default()
        }
    }

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, 0, 0, 0)
            .single()
            .expect("valid instant")
    }

    #[test]
    fn ramps_ten_percent_a_day_to_the_cap() {
        let p = params_300(); // 300 → max 2000
        let mut b = BudgetState::new(&p, t0());
        b.advance(t0() + TimeDelta::hours(23), &p);
        assert_eq!(b.daily_budget, 300);
        b.advance(t0() + TimeDelta::hours(24), &p);
        assert_eq!(b.daily_budget, 330);
        b.advance(t0() + TimeDelta::days(3), &p);
        assert_eq!(b.daily_budget, 400); // 330 → 363 → 400
        b.advance(t0() + TimeDelta::days(100), &p);
        assert_eq!(b.daily_budget, 2000);
        assert!(b.clean_since > t0() + TimeDelta::days(99));
        // Idempotent.
        let before = b.clone();
        b.advance(t0() + TimeDelta::days(100), &p);
        assert_eq!(b, before);
    }

    #[test]
    fn block_halves_and_backs_off() {
        let p = params_300();
        let mut b = BudgetState::new(&p, t0());
        let t = t0() + TimeDelta::days(2); // 363
        b.on_block("google_flights", t, &p);
        assert_eq!(b.daily_budget, 181);
        assert_eq!(b.clean_since, t);
        assert!(
            !b.site_available("google_flights", t + TimeDelta::minutes(119))
        );
        assert!(b.site_available("google_flights", t + TimeDelta::hours(2)));
        assert!(b.site_available("airline:LA", t));
        assert_eq!(
            b.cooldown_until("google_flights", t),
            Some(t + TimeDelta::hours(2))
        );

        let t2 = t + TimeDelta::hours(3);
        b.on_block("google_flights", t2, &p);
        assert_eq!(b.sites["google_flights"].level, 2);
        assert_eq!(
            b.cooldown_until("google_flights", t2),
            Some(t2 + TimeDelta::hours(4))
        );
        for i in 0..10 {
            b.on_block("google_flights", t2 + TimeDelta::hours(i), &p);
        }
        let last = t2 + TimeDelta::hours(9);
        assert_eq!(
            b.cooldown_until("google_flights", last),
            Some(last + TimeDelta::hours(48))
        );
        assert_eq!(b.daily_budget, 50); // floor
        // Long after: the backoff starts over.
        let later = last + TimeDelta::days(10);
        b.on_block("google_flights", later, &p);
        assert_eq!(b.sites["google_flights"].level, 1);
    }

    #[test]
    fn allowance_paces_the_rest_of_the_day() {
        let p = params_300();
        let b = BudgetState::new(&p, t0());
        let mut rng = StdRng::seed_from_u64(4);
        assert_eq!(b.tick_allowance(t0(), 300, 10, &p, &mut rng), 0);
        assert_eq!(b.tick_allowance(t0(), 290, 1, &p, &mut rng), 10);
        // 300 loads over 1440 one-minute ticks: ~0.21 per tick.
        let total: u32 = (0..1440)
            .map(|_| b.tick_allowance(t0(), 0, 1440, &p, &mut rng))
            .sum();
        assert!((250..=350).contains(&total), "{total}");
    }

    #[hegel::test(test_cases = 200)]
    fn allowance_never_exceeds_what_is_left(tc: TestCase) {
        let p = params_300();
        let b = BudgetState::new(&p, t0());
        let used = tc.draw(gs::integers::<u32>().max_value(400));
        let ticks = tc.draw(gs::integers::<u32>().max_value(2000));
        let seed = tc.draw(gs::integers::<u64>());
        let a = b.tick_allowance(
            t0(),
            used,
            ticks,
            &p,
            &mut StdRng::seed_from_u64(seed),
        );
        assert!(a <= 300u32.saturating_sub(used));
    }

    #[hegel::test(test_cases = 100)]
    fn budget_stays_within_bounds(tc: TestCase) {
        let p = params_300();
        let mut b = BudgetState::new(&p, t0());
        let steps = tc.draw(
            gs::vecs(gs::tuples!(
                gs::integers::<i64>().min_value(0).max_value(24 * 5),
                gs::booleans()
            ))
            .max_size(30),
        );
        let mut t = t0();
        for (hours, block) in steps {
            t += TimeDelta::hours(hours);
            if block {
                b.on_block("google_flights", t, &p);
            } else {
                b.advance(t, &p);
            }
            assert!(b.daily_budget >= p.min_daily_budget);
            assert!(b.daily_budget <= p.max_daily_budget);
            assert!(b.clean_since <= t);
        }
    }
}
