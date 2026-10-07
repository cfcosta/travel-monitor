//! Each site's page-load budget (DECISIONS.md 0039, 0060):
//! tm-scheduler's state machine, persisted in the store's
//! `site_budgets` row of the site and its blocks.
//!
//! Every site (`Source::site()`) has its own budget, its own count of
//! loads today, its own +10%-a-day ramp and its own halving and cooldown
//! on a block; spending or a block on one site never touches another.
//! Google Flights starts from the settings' `daily_budget` and
//! `max_daily_budget` (the `--budget` and `--max-budget` flags), a
//! miles program from [`SiteLimits::MILES`] or the app (0083), every
//! other site from [`Settings::default_limits`] or `--site-budget`,
//! applied again at each start (0088). The scheduler is unchanged: a site's limits become the `daily_budget`,
//! `max_daily_budget` and `min_daily_budget` of a copy of the
//! [`Params`] ([`site_params`]).
//!
//! The store keeps each site's count, budget in force and the start of
//! its block-free stretch (`last_ramp_at`); its cooldown is rebuilt from
//! its blocks when the site's worker starts. Each tick adds its share of
//! what is left today to a small bank, so a check that costs several
//! loads still fits when a tick's share is a fraction of a load.

use std::collections::{BTreeMap, HashSet, VecDeque};

use chrono::{DateTime, Local, NaiveDate, TimeDelta, Timelike, Utc};
use rand::Rng;
use tm_domain::{GOOGLE_FLIGHTS, Settings, SiteLimits, Source};
use tm_scheduler::{BudgetState, Params, TickBudget};
use tm_store::{SiteBudget, Store};

/// The most loads a bank may hold: a six-month round-trip graph (8
/// loads) plus a few confirmations.
pub const BANK_CAP: u32 = 40;

/// One site's budget, owned by that site's worker (0062).
#[derive(Debug, Clone)]
pub struct Budget {
    pub site: String,
    pub limits: SiteLimits,
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

/// The lowest a block halves a budget to (0039), or the site's start if
/// that is lower.
pub const MIN_DAILY_BUDGET: u32 = 50;

/// `base` with a site's limits as its budget: where it starts, the most
/// it ramps to, and the floor for halving ([`MIN_DAILY_BUDGET`]).
pub fn site_params(base: &Params, limits: SiteLimits) -> Params {
    let limits = limits.normalized();
    Params {
        daily_budget: limits.start,
        max_daily_budget: limits.max,
        min_daily_budget: MIN_DAILY_BUDGET.min(limits.start),
        ..base.clone()
    }
}

/// Applies `--budget` and `--max-budget` to the settings (the
/// home-manager module's values are authoritative): they are Google
/// Flights' start and maximum (0060). Returns the settings in force.
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

/// Writes the configured limits into the sites' rows before their
/// workers start: Google Flights' from the settings, then each
/// `--site-budget`, then every other stored site's defaults
/// ([`Settings::default_limits`], 0088), so a new default reaches a
/// site that already has a row. A miles program without a
/// `--site-budget` is left alone: its limits are the user's, set in the
/// app (0083). A changed start resets that site's budget in force; an
/// unchanged one keeps its ramp; today's count is kept either way
/// ([`Store::configure_site_budget`]).
pub async fn configure_sites(
    store: &Store,
    settings: &Settings,
    overrides: &[(String, SiteLimits)],
    now: DateTime<Utc>,
    today: NaiveDate,
) -> anyhow::Result<()> {
    store
        .configure_site_budget(
            GOOGLE_FLIGHTS,
            settings.google_flights_limits(),
            today,
            now,
        )
        .await?;
    for (site, limits) in overrides {
        if site == GOOGLE_FLIGHTS {
            continue;
        }
        store
            .configure_site_budget(site, *limits, today, now)
            .await?;
    }
    for row in store.site_budgets().await? {
        let site = row.site.as_str();
        let set_elsewhere = site == GOOGLE_FLIGHTS
            || overrides.iter().any(|(s, _)| s == site)
            || matches!(Source::from_site(site), Some(Source::Miles { .. }));
        if !set_elsewhere {
            store
                .configure_site_budget(
                    site,
                    settings.default_limits(site),
                    today,
                    now,
                )
                .await?;
        }
    }
    Ok(())
}

/// The sites in a block cooldown at `now`, replayed from the recent
/// blocks (0039). The coordinator uses it for manual checks; each
/// worker keeps its own site's in its [`Budget`].
pub async fn cooling_sites(
    store: &Store,
    params: &Params,
    now: DateTime<Utc>,
) -> anyhow::Result<HashSet<String>> {
    let horizon = params.backoff_reset + params.max_block_cooldown;
    let mut replay = BudgetState::new(params, now);
    for b in store.blocks_since(now - horizon).await? {
        replay.on_block(&b.site, b.at, params);
    }
    Ok(replay
        .sites
        .keys()
        .filter(|s| !replay.site_available(s, now))
        .cloned()
        .collect())
}

impl Budget {
    /// `site`'s budget as stored (or its defaults), with its cooldown
    /// replayed from its recent blocks. `base` gives everything but the
    /// limits, which come from the row.
    pub async fn restore(
        store: &Store,
        site: &str,
        base: &Params,
        now: DateTime<Utc>,
        today: NaiveDate,
    ) -> anyhow::Result<Self> {
        let row = store.site_budget_or_default(site, today).await?;
        let limits = SiteLimits {
            start: row.start_budget,
            max: row.max_daily_budget,
        }
        .normalized();
        let params = site_params(base, limits);
        let mut state = BudgetState::new(&params, now);
        state.daily_budget = row.daily_budget.clamp(
            params.min_daily_budget.max(1),
            params.max_daily_budget.max(1),
        );
        state.clean_since = row.last_ramp_at.unwrap_or(now).min(now);
        let horizon = params.backoff_reset + params.max_block_cooldown;
        let blocks: Vec<_> = store
            .blocks_since(now - horizon)
            .await?
            .into_iter()
            .filter(|b| b.site == site)
            .collect();
        let mut replay = BudgetState::new(&params, now);
        for b in &blocks {
            replay.on_block(&b.site, b.at, &params);
        }
        state.sites = replay.sites;
        if let Some(last) = blocks.last() {
            state.clean_since = state.clean_since.max(last.at);
        }
        Ok(Self {
            site: site.to_owned(),
            limits,
            state,
            loads_today: row.loads_on(today),
            day: today,
            bank: 0,
            recent: VecDeque::new(),
        })
    }

    /// The scheduler's parameters with this site's limits.
    pub fn params(&self, base: &Params) -> Params {
        site_params(base, self.limits)
    }

    /// The row this budget is stored as.
    pub fn row(&self) -> SiteBudget {
        SiteBudget {
            site: self.site.clone(),
            start_budget: self.limits.start,
            daily_budget: self.state.daily_budget,
            max_daily_budget: self.limits.max,
            loads_today: self.loads_today,
            day: self.day,
            last_ramp_at: Some(self.state.clean_since),
        }
    }

    pub async fn save(&self, store: &Store) -> anyhow::Result<()> {
        store.save_site_budget(&self.row()).await?;
        Ok(())
    }

    /// New limits from the configuration, as
    /// [`Store::configure_site_budget`] applies them: a changed start
    /// resets the budget in force and the ramp's clock, a lower maximum
    /// caps it. True when something changed.
    pub fn configure(
        &mut self,
        limits: SiteLimits,
        now: DateTime<Utc>,
    ) -> bool {
        let limits = limits.normalized();
        if limits == self.limits {
            return false;
        }
        if limits.start != self.limits.start {
            self.state.daily_budget = limits.start;
            self.state.clean_since = now;
        }
        self.limits = limits;
        self.state.daily_budget = self.state.daily_budget.min(limits.max);
        true
    }

    /// Applies due ramp steps and the day change. True when something
    /// changed that should be saved.
    pub fn advance(
        &mut self,
        now: DateTime<Utc>,
        today: NaiveDate,
        base: &Params,
    ) -> bool {
        let params = self.params(base);
        let before =
            (self.state.daily_budget, self.state.clean_since, self.day);
        self.state.advance(now, &params);
        let max = params.max_daily_budget.max(params.min_daily_budget);
        self.state.daily_budget = self.state.daily_budget.min(max);
        if today != self.day {
            self.day = today;
            self.loads_today = 0;
        }
        before != (self.state.daily_budget, self.state.clean_since, self.day)
    }

    /// The budget in force now.
    pub fn daily(&self, now: DateTime<Utc>, base: &Params) -> u32 {
        self.state.budget_at(now, &self.params(base))
    }

    /// When this site's cooldown ends, if it is cooling down at `now`.
    pub fn cooling_until(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.state.cooldown_until(&self.site, now)
    }

    /// Whether this site is in a block cooldown at `now`.
    pub fn is_cooling(&self, now: DateTime<Utc>) -> bool {
        !self.state.site_available(&self.site, now)
    }

    /// Whether today's budget is spent.
    pub fn is_spent(&self, now: DateTime<Utc>, base: &Params) -> bool {
        self.loads_today >= self.daily(now, base)
    }

    /// This tick's budget: the bank plus this tick's share, zero while
    /// the site cools down.
    pub fn tick<R: Rng + ?Sized>(
        &mut self,
        now: DateTime<Utc>,
        ticks_left: u32,
        base: &Params,
        rng: &mut R,
    ) -> TickBudget {
        let params = self.params(base);
        let share = self.state.tick_allowance(
            now,
            self.loads_today,
            ticks_left,
            &params,
            rng,
        );
        let left = self.daily(now, base).saturating_sub(self.loads_today);
        let cap = if ticks_left <= 1 {
            left
        } else {
            BANK_CAP.min(left)
        };
        self.bank = (self.bank + i64::from(share)).min(i64::from(cap));
        let per_site_remaining: BTreeMap<String, u32> = if self.is_cooling(now)
        {
            [(self.site.clone(), 0)].into()
        } else {
            BTreeMap::new()
        };
        TickBudget {
            loads_available: u32::try_from(self.bank.max(0)).unwrap_or(0),
            per_site_remaining,
        }
    }

    /// Counts loads spent now, on this site only.
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
        let stored = store.add_site_loads(&self.site, loads, today).await?;
        self.day = stored.day;
        self.loads_today = stored.loads_today;
        Ok(())
    }

    /// This site's loads in the hour before `now` (recorded with each
    /// block, 0039).
    pub fn loads_last_hour(&self, now: DateTime<Utc>) -> u32 {
        self.recent
            .iter()
            .filter(|(at, _)| now - *at <= TimeDelta::hours(1))
            .map(|(_, n)| *n)
            .sum()
    }

    /// Records a block of this site: the event with the preceding load
    /// rate, a lower budget and the cooldown. Other sites are untouched.
    pub async fn on_block(
        &mut self,
        store: &Store,
        detail: &str,
        now: DateTime<Utc>,
        base: &Params,
    ) -> anyhow::Result<()> {
        store
            .record_block(&self.site, now, detail, self.loads_last_hour(now))
            .await?;
        let params = self.params(base);
        self.state.on_block(&self.site, now, &params);
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
        configure_sites(&store, &settings, &[], at(6, 0), day)
            .await
            .expect("configures");
        let mut budget =
            Budget::restore(&store, GOOGLE_FLIGHTS, &params, at(6, 0), day)
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
            .on_block(&store, "unusual traffic page", at(6, 3), &params)
            .await
            .expect("records");
        assert_eq!(budget.state.daily_budget, 60);
        assert!(budget.is_cooling(at(6, 4)));
        let tick = budget.tick(at(6, 4), 1, &params, &mut rng);
        assert_eq!(tick.per_site_remaining.get(GOOGLE_FLIGHTS), Some(&0));
        assert!(
            cooling_sites(&store, &params, at(6, 4))
                .await
                .expect("reads")
                .contains(GOOGLE_FLIGHTS)
        );

        // A restart keeps the lower budget, the count and the cooldown.
        let again =
            Budget::restore(&store, GOOGLE_FLIGHTS, &params, at(6, 4), day)
                .await
                .expect("restores");
        assert_eq!(again.state.daily_budget, 60);
        assert_eq!(again.loads_today, 8);
        assert!(again.is_cooling(at(6, 4)));
        assert!(!again.is_cooling(at(7, 12)));
        // A new day starts a new count.
        let mut next = again.clone();
        assert!(next.advance(at(7, 12), at(7, 12).date_naive(), &params));
        assert_eq!(next.loads_today, 0);
        assert!(ticks_left_today(std::time::Duration::from_secs(60)) >= 1);
    }

    /// Each site has its own budget, count, ramp and cooldown (0060): a
    /// block on Google Flights leaves TAP's untouched.
    #[tokio::test]
    async fn sites_keep_their_own_budgets() {
        let store = Store::open_in_memory().await.expect("opens");
        store.seed_defaults().await.expect("seeds");
        let settings = apply_flags(&store, Some(4800), Some(12000))
            .await
            .expect("applies");
        let params = Params::from_settings(&settings);
        let day = at(6, 0).date_naive();
        configure_sites(
            &store,
            &settings,
            &[("miles:smiles".into(), SiteLimits { start: 40, max: 80 })],
            at(6, 0),
            day,
        )
        .await
        .expect("configures");
        let mut google =
            Budget::restore(&store, GOOGLE_FLIGHTS, &params, at(6, 0), day)
                .await
                .expect("restores");
        let mut tap =
            Budget::restore(&store, "airline:TP", &params, at(6, 0), day)
                .await
                .expect("restores");
        let smiles =
            Budget::restore(&store, "miles:smiles", &params, at(6, 0), day)
                .await
                .expect("restores");
        assert_eq!(google.daily(at(6, 0), &params), 4800);
        assert_eq!(tap.daily(at(6, 0), &params), 1200);
        assert_eq!(tap.limits, SiteLimits::OTHER);
        assert_eq!(smiles.daily(at(6, 0), &params), 40);

        google
            .spend(&store, 10, at(6, 1), day)
            .await
            .expect("spends");
        tap.spend(&store, 3, at(6, 1), day).await.expect("spends");
        assert_eq!((google.loads_today, tap.loads_today), (10, 3));
        google
            .on_block(&store, "captcha", at(6, 2), &params)
            .await
            .expect("records");
        assert_eq!(google.daily(at(6, 2), &params), 2400);
        assert!(google.is_cooling(at(6, 3)));
        assert!(!tap.is_cooling(at(6, 3)));
        assert_eq!(tap.daily(at(6, 3), &params), 1200);
        let mut rng = StdRng::seed_from_u64(3);
        assert!(
            tap.tick(at(6, 3), 1, &params, &mut rng)
                .per_site_remaining
                .is_empty()
        );

        // TAP ramps on its own clock: +10% after a clean day, to its cap.
        let tap_again =
            Budget::restore(&store, "airline:TP", &params, at(6, 3), day)
                .await
                .expect("restores");
        assert!(!tap_again.is_cooling(at(6, 3)));
        assert_eq!(tap_again.loads_today, 3);
        let mut later = tap_again.clone();
        later.advance(at(9, 0), at(9, 0).date_naive(), &params);
        assert!(later.daily(at(9, 0), &params) > 1200);
        later.advance(at(30, 0), at(30, 0).date_naive(), &params);
        assert_eq!(later.daily(at(30, 0), &params), 4000);

        // New limits from the settings reset a changed start only.
        assert!(!google.configure(settings.google_flights_limits(), at(6, 4)));
        assert!(google.configure(
            SiteLimits {
                start: 6000,
                max: 12000
            },
            at(6, 4)
        ));
        assert_eq!(google.state.daily_budget, 6000);
        assert!(google.configure(
            SiteLimits {
                start: 6000,
                max: 5000
            },
            at(6, 4)
        ));
        assert_eq!(
            google.limits,
            SiteLimits {
                start: 6000,
                max: 6000
            }
        );
    }

    /// New defaults reach a site that already has a row, keeping
    /// today's count (0088); a `--site-budget` still wins, and a miles
    /// program keeps the limits the app set (0083).
    #[tokio::test]
    async fn a_start_brings_stored_sites_to_their_defaults() {
        let store = Store::open_in_memory().await.expect("opens");
        store.seed_defaults().await.expect("seeds");
        let settings = store.settings().await.expect("reads");
        let day = at(7, 0).date_naive();
        let old = |site: &str, start, daily, max| SiteBudget {
            site: site.into(),
            start_budget: start,
            daily_budget: daily,
            max_daily_budget: max,
            loads_today: 241,
            day,
            last_ramp_at: Some(at(6, 0)),
        };
        for row in [
            old("airline:TP", 300, 330, 1000),
            old("airline:AD", 300, 300, 1000),
            old("airline:LA", 300, 330, 1000),
            old("miles:latam_pass", 60, 60, 120),
        ] {
            store.save_site_budget(&row).await.expect("saves");
        }
        let overrides = [(
            "airline:AD".to_owned(),
            SiteLimits {
                start: 300,
                max: 1000,
            },
        )];
        configure_sites(&store, &settings, &overrides, at(7, 19), day)
            .await
            .expect("configures");
        let row = |site: &'static str| {
            let store = store.clone();
            async move {
                store
                    .site_budget(site)
                    .await
                    .expect("reads")
                    .expect("a row")
            }
        };
        let tap = row("airline:TP").await;
        assert_eq!(
            (tap.start_budget, tap.daily_budget, tap.max_daily_budget),
            (1200, 1200, 4000)
        );
        assert_eq!((tap.loads_today, tap.day), (241, day));
        assert_eq!(tap.last_ramp_at, Some(at(7, 19)), "the ramp restarts");
        assert_eq!(row("airline:AD").await, old("airline:AD", 300, 300, 1000));
        assert_eq!(row("airline:LA").await, old("airline:LA", 300, 330, 1000));
        assert_eq!(
            row("miles:latam_pass").await,
            old("miles:latam_pass", 60, 60, 120)
        );

        // A second start with the same limits keeps TAP's ramp.
        configure_sites(&store, &settings, &overrides, at(9, 0), day)
            .await
            .expect("configures");
        assert_eq!(row("airline:TP").await, tap);
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
        configure_sites(&store, &settings, &[], at(6, 0), day)
            .await
            .expect("configures");
        let mut budget =
            Budget::restore(&store, GOOGLE_FLIGHTS, &params, at(6, 0), day)
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
