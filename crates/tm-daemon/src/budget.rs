//! The page-load budget (DECISIONS.md 0039): tm-scheduler's state
//! machine, persisted in the store's budget row and its block log.
//!
//! The store keeps today's count, the daily budget and the start of the
//! current block-free stretch (`last_ramp_at`); site cooldowns are
//! rebuilt from the block log when the daemon starts. Each tick adds
//! its share of what is left today to a small bank, so a check that
//! costs several loads still fits when a tick's share is a fraction of
//! a load.

use std::collections::{BTreeMap, HashSet, VecDeque};

use chrono::{DateTime, Local, NaiveDate, TimeDelta, Timelike, Utc};
use rand::Rng;
use tm_domain::Settings;
use tm_scheduler::{BudgetState, Params, TickBudget};
use tm_store::Store;

/// The most loads a bank may hold: a six-month round-trip graph (8
/// loads) plus a few confirmations.
pub const BANK_CAP: u32 = 40;

#[derive(Debug, Clone)]
pub struct Budget {
    pub state: BudgetState,
    pub loads_today: u32,
    pub day: NaiveDate,
    /// Loads banked for the next picks. Negative after a check cost more
    /// than it was charged (a never-checked family's estimate is low):
    /// the debt is repaid from later shares instead of being forgiven,
    /// which let a day's spending run past its budget.
    bank: i64,
    recent: VecDeque<(DateTime<Utc>, u32)>,
}

/// The local calendar day: budgets are per day as the user lives it.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// Ticks left before local midnight, this one included.
pub fn ticks_left_today(tick: std::time::Duration) -> u32 {
    let now = Local::now();
    let elapsed = i64::from(now.num_seconds_from_midnight());
    let left = (86_400 - elapsed).max(1);
    let tick = i64::try_from(tick.as_secs().max(1)).unwrap_or(i64::MAX);
    u32::try_from((left + tick - 1) / tick)
        .unwrap_or(u32::MAX)
        .max(1)
}

/// Applies `--budget` and `--max-budget` to the settings (the
/// home-manager module's values are authoritative). Returns the
/// settings in force.
pub async fn apply_flags(
    store: &Store,
    budget: Option<u32>,
    max_budget: Option<u32>,
) -> anyhow::Result<Settings> {
    let mut settings = store.settings().await?;
    let before = settings.clone();
    if let Some(b) = budget {
        settings.daily_budget = b.max(1);
    }
    if let Some(m) = max_budget {
        settings.max_daily_budget = m.max(1);
    }
    if settings.max_daily_budget < settings.daily_budget {
        settings.max_daily_budget = settings.daily_budget;
    }
    if settings != before {
        store.set_settings(&settings).await?;
    }
    Ok(settings)
}

impl Budget {
    /// The budget as stored, with site cooldowns replayed from recent
    /// blocks. Without a stored row it starts at the settings' budget.
    pub async fn restore(
        store: &Store,
        params: &Params,
        now: DateTime<Utc>,
        today: NaiveDate,
    ) -> anyhow::Result<Self> {
        let mut state = BudgetState::new(params, now);
        let mut loads_today = 0;
        if let Some(stored) = store.budget().await? {
            state.daily_budget = stored.daily_budget.clamp(
                params.min_daily_budget.max(1),
                params.max_daily_budget.max(1),
            );
            state.clean_since = stored.last_ramp_at.unwrap_or(now).min(now);
            if stored.day == today {
                loads_today = stored.loads_today;
            }
        }
        let horizon = params.backoff_reset + params.max_block_cooldown;
        let blocks = store.blocks_since(now - horizon).await?;
        let mut replay = BudgetState::new(params, now);
        for b in &blocks {
            replay.on_block(&b.site, b.at, params);
        }
        state.sites = replay.sites;
        if let Some(last) = blocks.last() {
            state.clean_since = state.clean_since.max(last.at);
        }
        Ok(Self {
            state,
            loads_today,
            day: today,
            bank: 0,
            recent: VecDeque::new(),
        })
    }

    pub async fn save(&self, store: &Store) -> anyhow::Result<()> {
        store
            .save_budget(&tm_store::BudgetState {
                daily_budget: self.state.daily_budget,
                loads_today: self.loads_today,
                day: self.day,
                last_ramp_at: Some(self.state.clean_since),
            })
            .await?;
        Ok(())
    }

    /// Applies due ramp steps and the day change. True when something
    /// changed that should be saved.
    pub fn advance(
        &mut self,
        now: DateTime<Utc>,
        today: NaiveDate,
        params: &Params,
    ) -> bool {
        let before =
            (self.state.daily_budget, self.state.clean_since, self.day);
        self.state.advance(now, params);
        let max = params.max_daily_budget.max(params.min_daily_budget);
        self.state.daily_budget = self.state.daily_budget.min(max);
        if today != self.day {
            self.day = today;
            self.loads_today = 0;
        }
        before != (self.state.daily_budget, self.state.clean_since, self.day)
    }

    /// The budget in force now.
    pub fn daily(&self, now: DateTime<Utc>, params: &Params) -> u32 {
        self.state.budget_at(now, params)
    }

    /// Sites in a cooldown at `now`.
    pub fn cooling(&self, now: DateTime<Utc>) -> HashSet<String> {
        self.state
            .sites
            .keys()
            .filter(|s| !self.state.site_available(s, now))
            .cloned()
            .collect()
    }

    /// This tick's budget: the bank plus this tick's share, every
    /// cooling site at zero.
    pub fn tick<R: Rng + ?Sized>(
        &mut self,
        now: DateTime<Utc>,
        ticks_left: u32,
        params: &Params,
        rng: &mut R,
    ) -> TickBudget {
        let share = self.state.tick_allowance(
            now,
            self.loads_today,
            ticks_left,
            params,
            rng,
        );
        let left = self.daily(now, params).saturating_sub(self.loads_today);
        let cap = if ticks_left <= 1 {
            left
        } else {
            BANK_CAP.min(left)
        };
        self.bank = (self.bank + i64::from(share)).min(i64::from(cap));
        let per_site_remaining: BTreeMap<String, u32> =
            self.cooling(now).into_iter().map(|s| (s, 0)).collect();
        TickBudget {
            loads_available: u32::try_from(self.bank.max(0)).unwrap_or(0),
            per_site_remaining,
        }
    }

    /// Counts loads spent now.
    pub async fn spend(
        &mut self,
        store: &Store,
        loads: u32,
        now: DateTime<Utc>,
        today: NaiveDate,
    ) -> anyhow::Result<()> {
        if loads == 0 {
            return Ok(());
        }
        self.bank -= i64::from(loads);
        self.recent.push_back((now, loads));
        while self
            .recent
            .front()
            .is_some_and(|(at, _)| now - *at > TimeDelta::hours(1))
        {
            self.recent.pop_front();
        }
        let stored = store.add_loads(loads, today).await?;
        self.day = stored.day;
        self.loads_today = stored.loads_today;
        Ok(())
    }

    /// Loads in the hour before `now` (recorded with each block, 0039).
    pub fn loads_last_hour(&self, now: DateTime<Utc>) -> u32 {
        self.recent
            .iter()
            .filter(|(at, _)| now - *at <= TimeDelta::hours(1))
            .map(|(_, n)| *n)
            .sum()
    }

    /// Records a block: the event with the preceding load rate, a lower
    /// budget and the site's cooldown.
    pub async fn on_block(
        &mut self,
        store: &Store,
        site: &str,
        detail: &str,
        now: DateTime<Utc>,
        params: &Params,
    ) -> anyhow::Result<()> {
        store
            .record_block(site, now, detail, self.loads_last_hour(now))
            .await?;
        self.state.on_block(site, now, params);
        self.bank = 0;
        self.save(store).await
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "#[tokio::test] expands to block_on"
)]
mod tests {
    use chrono::TimeZone;
    use rand::{SeedableRng, rngs::StdRng};

    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0)
            .single()
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn flags_budget_bank_and_blocks() {
        let store = Store::open_in_memory().await.expect("opens");
        store.seed_defaults().await.expect("seeds");
        let settings = apply_flags(&store, Some(120), Some(100))
            .await
            .expect("applies");
        assert_eq!(settings.daily_budget, 120);
        assert_eq!(settings.max_daily_budget, 120);
        assert_eq!(store.settings().await.expect("reads").daily_budget, 120);
        let params = Params::from_settings(&settings);
        let day = at(6, 0).date_naive();
        let mut budget = Budget::restore(&store, &params, at(6, 0), day)
            .await
            .expect("restores");
        assert_eq!(budget.state.daily_budget, 120);
        let mut rng = StdRng::seed_from_u64(7);
        // One tick left: the whole day's budget.
        let tick = budget.tick(at(6, 0), 1, &params, &mut rng);
        assert_eq!(tick.loads_available, 120);
        budget
            .spend(&store, 8, at(6, 1), day)
            .await
            .expect("spends");
        assert_eq!(budget.loads_today, 8);
        assert_eq!(budget.loads_last_hour(at(6, 1)), 8);
        // Many ticks left: a small share, banked up to the cap.
        let mut b2 = budget.clone();
        b2.bank = 0;
        let t = b2.tick(at(6, 2), 1000, &params, &mut rng);
        assert!(t.loads_available <= 1);
        for _ in 0..10_000 {
            b2.tick(at(6, 2), 1000, &params, &mut rng);
        }
        assert_eq!(
            b2.tick(at(6, 2), 1000, &params, &mut rng).loads_available,
            BANK_CAP
        );

        budget
            .on_block(
                &store,
                "google_flights",
                "unusual traffic page",
                at(6, 3),
                &params,
            )
            .await
            .expect("records");
        assert_eq!(budget.state.daily_budget, 60);
        assert!(budget.cooling(at(6, 4)).contains("google_flights"));
        let tick = budget.tick(at(6, 4), 1, &params, &mut rng);
        assert_eq!(tick.per_site_remaining.get("google_flights"), Some(&0));

        // A restart keeps the lower budget, the count and the cooldown.
        let again = Budget::restore(&store, &params, at(6, 4), day)
            .await
            .expect("restores");
        assert_eq!(again.state.daily_budget, 60);
        assert_eq!(again.loads_today, 8);
        assert!(again.cooling(at(6, 4)).contains("google_flights"));
        assert!(again.cooling(at(7, 12)).is_empty());
        // A new day starts a new count.
        let mut next = again.clone();
        assert!(next.advance(at(7, 12), at(7, 12).date_naive(), &params));
        assert_eq!(next.loads_today, 0);
        assert!(ticks_left_today(std::time::Duration::from_secs(60)) >= 1);
    }

    #[tokio::test]
    async fn a_check_that_costs_more_than_charged_is_repaid() {
        let store = Store::open_in_memory().await.expect("opens");
        store.seed_defaults().await.expect("seeds");
        let settings = apply_flags(&store, Some(600), Some(600))
            .await
            .expect("applies");
        let params = Params::from_settings(&settings);
        let day = at(6, 0).date_naive();
        let mut budget = Budget::restore(&store, &params, at(6, 0), day)
            .await
            .expect("restores");
        // A never-checked family was charged 3 loads but used 8.
        budget.bank = 3;
        budget
            .spend(&store, 8, at(6, 1), day)
            .await
            .expect("spends");
        assert_eq!(budget.bank, -5);
        // The next ticks pay the 5 back before anything is available.
        let mut rng = StdRng::seed_from_u64(1);
        let mut seen = 0;
        while budget.bank < 0 {
            let t = budget.tick(at(6, 2), 600, &params, &mut rng);
            assert_eq!(t.loads_available, 0);
            seen += 1;
            assert!(seen < 10_000, "the debt is never repaid");
        }
    }
}
