//! Daemon bookkeeping: the page-load budget and blocks (DECISIONS.md
//! 0039), exchange rates (0038) and the heartbeat the app shows
//! (0057).

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{Currency, FamilyId, QueryId};

use crate::{
    Store,
    convert::count,
    error::{Result, StoreError},
    records::{
        BlockRecord,
        BudgetState,
        CheckInProgress,
        ExchangeRate,
        Heartbeat,
    },
};

impl Store {
    /// The budget row, if the daemon has written one.
    pub async fn budget(&self) -> Result<Option<BudgetState>> {
        let row = sqlx::query!(
            r#"SELECT daily_budget, loads_today, day AS "day: NaiveDate",
                      last_ramp_at AS "last_ramp_at: DateTime<Utc>"
               FROM budget WHERE id = 1"#
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(BudgetState {
                daily_budget: count("daily_budget", r.daily_budget)?,
                loads_today: count("loads_today", r.loads_today)?,
                day: r.day,
                last_ramp_at: r.last_ramp_at,
            })
        })
        .transpose()
    }

    pub async fn save_budget(&self, state: &BudgetState) -> Result<()> {
        sqlx::query!(
            "INSERT INTO budget (id, daily_budget, loads_today, day, last_ramp_at)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET
                 daily_budget = excluded.daily_budget,
                 loads_today = excluded.loads_today,
                 day = excluded.day,
                 last_ramp_at = excluded.last_ramp_at",
            state.daily_budget,
            state.loads_today,
            state.day,
            state.last_ramp_at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Counts `loads` page loads on `today`, starting a new count when
    /// the day changed. Without a budget row yet, one is created with
    /// the settings' daily budget. Returns the new state.
    pub async fn add_loads(
        &self,
        loads: u32,
        today: NaiveDate,
    ) -> Result<BudgetState> {
        let mut tx = self.write().await?;
        let budget =
            i64::from(crate::config::settings_in(&mut tx).await?.daily_budget);
        sqlx::query!(
            "INSERT INTO budget (id, daily_budget, loads_today, day)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET
                 loads_today = CASE WHEN budget.day = excluded.day
                     THEN budget.loads_today + excluded.loads_today
                     ELSE excluded.loads_today END,
                 day = excluded.day",
            budget,
            loads,
            today
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.budget()
            .await?
            .ok_or_else(|| StoreError::not_found("budget", 1))
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

    /// Writes the heartbeat, replacing the last one. A check in
    /// progress is written with it; `None` clears it (DECISIONS.md 0057).
    pub async fn write_heartbeat(&self, beat: &Heartbeat) -> Result<()> {
        let check = beat.check.as_ref();
        let family = check.map(|c| c.family.0);
        let query = check.and_then(|c| c.query).map(|q| q.0);
        let reason = check
            .map(|c| serde_json::to_string(&c.reason))
            .transpose()?;
        let started_at = check.map(|c| c.started_at);
        sqlx::query!(
            "INSERT INTO heartbeat (id, pid, updated_at, state, next_check,
                 loads_today, budget, next_check_at, check_family_id,
                 check_query_id, check_reason_json, check_started_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (id) DO UPDATE SET
                 pid = excluded.pid,
                 updated_at = excluded.updated_at,
                 state = excluded.state,
                 next_check = excluded.next_check,
                 loads_today = excluded.loads_today,
                 budget = excluded.budget,
                 next_check_at = excluded.next_check_at,
                 check_family_id = excluded.check_family_id,
                 check_query_id = excluded.check_query_id,
                 check_reason_json = excluded.check_reason_json,
                 check_started_at = excluded.check_started_at",
            beat.pid,
            beat.updated_at,
            beat.state,
            beat.next_check,
            beat.loads_today,
            beat.budget,
            beat.next_check_at,
            family,
            query,
            reason,
            started_at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn heartbeat(&self) -> Result<Option<Heartbeat>> {
        let row = sqlx::query!(
            r#"SELECT pid, updated_at AS "updated_at: DateTime<Utc>", state,
                      next_check, loads_today, budget,
                      next_check_at AS "next_check_at: DateTime<Utc>",
                      check_family_id, check_query_id, check_reason_json,
                      check_started_at AS "check_started_at: DateTime<Utc>"
               FROM heartbeat WHERE id = 1"#
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
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
            Ok(Heartbeat {
                pid: count("pid", r.pid)?,
                updated_at: r.updated_at,
                state: r.state,
                next_check: r.next_check,
                next_check_at: r.next_check_at,
                check,
                loads_today: count("loads_today", r.loads_today)?,
                budget: count("budget", r.budget)?,
            })
        })
        .transpose()
    }
}
