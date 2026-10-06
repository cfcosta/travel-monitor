//! Daemon bookkeeping: each site's page-load budget and the blocks
//! (DECISIONS.md 0039, 0060), exchange rates (0038), and the heartbeat
//! and per-site status the app shows (0057, 0062).

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{Currency, FamilyId, QueryId};

use crate::{
    Store,
    convert::count,
    error::{Result, StoreError},
    records::{
        BlockRecord,
        CheckInProgress,
        ExchangeRate,
        Heartbeat,
        SiteBudget,
        SiteStatus,
    },
};

impl Store {
    /// Every stored site budget, Google Flights first, then by site
    /// (0060). Sites never charged a load have no row yet.
    pub async fn site_budgets(&self) -> Result<Vec<SiteBudget>> {
        let rows = sqlx::query!(
            r#"SELECT site AS "site!: String", start_budget, daily_budget,
                      max_daily_budget, loads_today, day AS "day: NaiveDate",
                      last_ramp_at AS "last_ramp_at: DateTime<Utc>"
               FROM site_budgets
               ORDER BY site <> 'google_flights', site"#
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(SiteBudget {
                    site: r.site,
                    start_budget: count("start_budget", r.start_budget)?,
                    daily_budget: count("daily_budget", r.daily_budget)?,
                    max_daily_budget: count(
                        "max_daily_budget",
                        r.max_daily_budget,
                    )?,
                    loads_today: count("loads_today", r.loads_today)?,
                    day: r.day,
                    last_ramp_at: r.last_ramp_at,
                })
            })
            .collect()
    }

    /// `site`'s stored budget, if it has one.
    pub async fn site_budget(&self, site: &str) -> Result<Option<SiteBudget>> {
        let mut conn = self.pool.acquire().await?;
        site_budget_in(&mut conn, site).await
    }

    /// `site`'s stored budget, or the one it starts from: Google
    /// Flights' from the settings, any other site's
    /// [`tm_domain::SiteLimits::OTHER`] (0060). Nothing is written.
    pub async fn site_budget_or_default(
        &self,
        site: &str,
        today: NaiveDate,
    ) -> Result<SiteBudget> {
        let mut conn = self.pool.acquire().await?;
        if let Some(b) = site_budget_in(&mut conn, site).await? {
            return Ok(b);
        }
        let settings = crate::config::settings_in(&mut conn).await?;
        Ok(SiteBudget::new(site, settings.default_limits(site), today))
    }

    /// Writes a site's whole budget row.
    pub async fn save_site_budget(&self, b: &SiteBudget) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        save_site_budget_in(&mut conn, b).await
    }

    /// Sets a site's limits (the command line's `--budget`,
    /// `--max-budget` and `--site-budget`, or the settings). A new start
    /// resets the budget in force to it and restarts the ramp's clock at
    /// `now`; an unchanged one keeps the ramped budget. The budget in
    /// force never exceeds the new maximum. A site without a row gets
    /// one. Returns the row as written.
    pub async fn configure_site_budget(
        &self,
        site: &str,
        limits: tm_domain::SiteLimits,
        today: NaiveDate,
        now: DateTime<Utc>,
    ) -> Result<SiteBudget> {
        let limits = limits.normalized();
        let mut tx = self.write().await?;
        let row = match site_budget_in(&mut tx, site).await? {
            None => SiteBudget::new(site, limits, today),
            Some(mut row) => {
                if row.start_budget != limits.start {
                    row.start_budget = limits.start;
                    row.daily_budget = limits.start;
                    row.last_ramp_at = Some(now);
                }
                row.max_daily_budget = limits.max;
                row.daily_budget = row.daily_budget.min(limits.max).max(1);
                row
            }
        };
        save_site_budget_in(&mut tx, &row).await?;
        tx.commit().await?;
        Ok(row)
    }

    /// Counts `loads` page loads of `site` on `today`, starting a new
    /// count when the day changed. A site without a row gets one from
    /// its defaults ([`Store::site_budget_or_default`]). Returns the new
    /// row.
    pub async fn add_site_loads(
        &self,
        site: &str,
        loads: u32,
        today: NaiveDate,
    ) -> Result<SiteBudget> {
        let mut tx = self.write().await?;
        let settings = crate::config::settings_in(&mut tx).await?;
        let fresh = SiteBudget::new(site, settings.default_limits(site), today);
        sqlx::query!(
            "INSERT INTO site_budgets (site, start_budget, daily_budget,
                 max_daily_budget, loads_today, day)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (site) DO UPDATE SET
                 loads_today = CASE WHEN site_budgets.day = excluded.day
                     THEN site_budgets.loads_today + excluded.loads_today
                     ELSE excluded.loads_today END,
                 day = excluded.day",
            fresh.site,
            fresh.start_budget,
            fresh.daily_budget,
            fresh.max_daily_budget,
            loads,
            today
        )
        .execute(&mut *tx)
        .await?;
        let row = site_budget_in(&mut tx, site)
            .await?
            .ok_or_else(|| StoreError::not_found("site budget", site))?;
        tx.commit().await?;
        Ok(row)
    }

    /// Records a block with the load rate that preceded it (0039).
    pub async fn record_block(
        &self,
        site: &str,
        at: DateTime<Utc>,
        detail: &str,
        loads_last_hour: u32,
    ) -> Result<i64> {
        let done = sqlx::query!(
            "INSERT INTO blocks (site, at, detail, loads_last_hour)
             VALUES (?1, ?2, ?3, ?4)",
            site,
            at,
            detail,
            loads_last_hour
        )
        .execute(&self.pool)
        .await?;
        Ok(done.last_insert_rowid())
    }

    /// Blocks since `since`, oldest first.
    pub async fn blocks_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<BlockRecord>> {
        let rows = sqlx::query!(
            r#"SELECT id, site, at AS "at: DateTime<Utc>", detail,
                      loads_last_hour
               FROM blocks WHERE at >= ?1 ORDER BY at, id"#,
            since
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(BlockRecord {
                    id: r.id,
                    site: r.site,
                    at: r.at,
                    detail: r.detail,
                    loads_last_hour: count(
                        "loads_last_hour",
                        r.loads_last_hour,
                    )?,
                })
            })
            .collect()
    }

    /// Records BRL per unit of `currency` at `at`.
    pub async fn set_exchange_rate(
        &self,
        currency: &Currency,
        brl_per_unit: f64,
        at: DateTime<Utc>,
    ) -> Result<()> {
        if !(brl_per_unit.is_finite() && brl_per_unit > 0.0) {
            return Err(StoreError::Invalid(format!("rate {brl_per_unit}")));
        }
        let code = currency.as_str().trim().to_ascii_uppercase();
        sqlx::query!(
            "INSERT INTO exchange_rates (currency, brl_per_unit, at)
             VALUES (?1, ?2, ?3)",
            code,
            brl_per_unit,
            at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The latest rate for a currency. BRL is always 1.
    pub async fn latest_exchange_rate(
        &self,
        currency: &Currency,
    ) -> Result<Option<ExchangeRate>> {
        let code = currency.as_str().trim().to_ascii_uppercase();
        let row = sqlx::query!(
            r#"SELECT brl_per_unit, at AS "at: DateTime<Utc>"
               FROM exchange_rates WHERE currency = ?1
               ORDER BY at DESC, id DESC LIMIT 1"#,
            code
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| ExchangeRate {
            currency: Currency(code.clone()),
            brl_per_unit: r.brl_per_unit,
            at: r.at,
        }))
    }

    /// The latest rate of every currency, by code.
    pub async fn latest_exchange_rates(&self) -> Result<Vec<ExchangeRate>> {
        let rows = sqlx::query!(
            r#"SELECT e.currency, e.brl_per_unit, e.at AS "at: DateTime<Utc>"
               FROM exchange_rates e
               WHERE e.id = (SELECT x.id FROM exchange_rates x
                             WHERE x.currency = e.currency
                             ORDER BY x.at DESC, x.id DESC LIMIT 1)
               ORDER BY e.currency"#
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| ExchangeRate {
                currency: Currency(r.currency),
                brl_per_unit: r.brl_per_unit,
                at: r.at,
            })
            .collect())
    }

    /// Writes the heartbeat, replacing the last one. The check columns
    /// a daemon before 0062 wrote are cleared: checks are per site now
    /// ([`Store::write_site_status`]).
    pub async fn write_heartbeat(&self, beat: &Heartbeat) -> Result<()> {
        sqlx::query!(
            "INSERT INTO heartbeat (id, pid, updated_at, state, next_check,
                 loads_today, budget, next_check_at, check_family_id,
                 check_query_id, check_reason_json, check_started_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, NULL)
             ON CONFLICT (id) DO UPDATE SET
                 pid = excluded.pid,
                 updated_at = excluded.updated_at,
                 state = excluded.state,
                 next_check = excluded.next_check,
                 loads_today = excluded.loads_today,
                 budget = excluded.budget,
                 next_check_at = excluded.next_check_at,
                 check_family_id = NULL,
                 check_query_id = NULL,
                 check_reason_json = NULL,
                 check_started_at = NULL",
            beat.pid,
            beat.updated_at,
            beat.state,
            beat.next_check,
            beat.loads_today,
            beat.budget,
            beat.next_check_at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn heartbeat(&self) -> Result<Option<Heartbeat>> {
        let row = sqlx::query!(
            r#"SELECT pid, updated_at AS "updated_at: DateTime<Utc>", state,
                      next_check, loads_today, budget,
                      next_check_at AS "next_check_at: DateTime<Utc>"
               FROM heartbeat WHERE id = 1"#
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(Heartbeat {
                pid: count("pid", r.pid)?,
                updated_at: r.updated_at,
                state: r.state,
                next_check: r.next_check,
                next_check_at: r.next_check_at,
                loads_today: count("loads_today", r.loads_today)?,
                budget: count("budget", r.budget)?,
            })
        })
        .transpose()
    }

    /// Writes what one site is doing, replacing its last status (0062).
    pub async fn write_site_status(&self, status: &SiteStatus) -> Result<()> {
        let check = status.check.as_ref();
        let family = check.map(|c| c.family.0);
        let query = check.and_then(|c| c.query).map(|q| q.0);
        let reason = check
            .map(|c| serde_json::to_string(&c.reason))
            .transpose()?;
        let started_at = check.map(|c| c.started_at);
        sqlx::query!(
            "INSERT INTO site_status (site, updated_at, state, next_check_at,
                 check_family_id, check_query_id, check_reason_json,
                 check_started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (site) DO UPDATE SET
                 updated_at = excluded.updated_at,
                 state = excluded.state,
                 next_check_at = excluded.next_check_at,
                 check_family_id = excluded.check_family_id,
                 check_query_id = excluded.check_query_id,
                 check_reason_json = excluded.check_reason_json,
                 check_started_at = excluded.check_started_at",
            status.site,
            status.updated_at,
            status.state,
            status.next_check_at,
            family,
            query,
            reason,
            started_at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every site's status, Google Flights first, then by site.
    pub async fn site_statuses(&self) -> Result<Vec<SiteStatus>> {
        let rows = sqlx::query!(
            r#"SELECT site AS "site!: String",
                      updated_at AS "updated_at: DateTime<Utc>", state,
                      next_check_at AS "next_check_at: DateTime<Utc>",
                      check_family_id, check_query_id, check_reason_json,
                      check_started_at AS "check_started_at: DateTime<Utc>"
               FROM site_status
               ORDER BY site <> 'google_flights', site"#
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let check = match (
                    r.check_family_id,
                    r.check_reason_json,
                    r.check_started_at,
                ) {
                    (Some(family), Some(reason), Some(started_at)) => {
                        Some(CheckInProgress {
                            family: FamilyId(family),
                            query: r.check_query_id.map(QueryId),
                            reason: serde_json::from_str(&reason)?,
                            started_at,
                        })
                    }
                    _ => None,
                };
                Ok(SiteStatus {
                    site: r.site,
                    updated_at: r.updated_at,
                    state: r.state,
                    next_check_at: r.next_check_at,
                    check,
                })
            })
            .collect()
    }

    /// Forgets every site's status (a previous run's), when the daemon
    /// starts.
    pub async fn clear_site_statuses(&self) -> Result<()> {
        sqlx::query!("DELETE FROM site_status")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

async fn site_budget_in(
    conn: &mut sqlx::SqliteConnection,
    site: &str,
) -> Result<Option<SiteBudget>> {
    let row = sqlx::query!(
        r#"SELECT site AS "site!: String", start_budget, daily_budget,
                  max_daily_budget, loads_today, day AS "day: NaiveDate",
                  last_ramp_at AS "last_ramp_at: DateTime<Utc>"
           FROM site_budgets WHERE site = ?1"#,
        site
    )
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|r| {
        Ok(SiteBudget {
            site: r.site,
            start_budget: count("start_budget", r.start_budget)?,
            daily_budget: count("daily_budget", r.daily_budget)?,
            max_daily_budget: count("max_daily_budget", r.max_daily_budget)?,
            loads_today: count("loads_today", r.loads_today)?,
            day: r.day,
            last_ramp_at: r.last_ramp_at,
        })
    })
    .transpose()
}

async fn save_site_budget_in(
    conn: &mut sqlx::SqliteConnection,
    b: &SiteBudget,
) -> Result<()> {
    let start = b.start_budget.max(1);
    let daily = b.daily_budget.max(1);
    let max = b.max_daily_budget.max(1);
    sqlx::query!(
        "INSERT INTO site_budgets (site, start_budget, daily_budget,
             max_daily_budget, loads_today, day, last_ramp_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (site) DO UPDATE SET
             start_budget = excluded.start_budget,
             daily_budget = excluded.daily_budget,
             max_daily_budget = excluded.max_daily_budget,
             loads_today = excluded.loads_today,
             day = excluded.day,
             last_ramp_at = excluded.last_ramp_at",
        b.site,
        start,
        daily,
        max,
        b.loads_today,
        b.day,
        b.last_ramp_at
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}
