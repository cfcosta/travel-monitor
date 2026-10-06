//! The query log: why each query ran, its score, and how it ended
//! (DECISIONS.md 0028, 0031).

use chrono::{DateTime, Utc};
use tm_domain::{FamilyId, QueryId, QueryOutcome, QueryReason};

use crate::{
    Store,
    config::settings_in,
    convert::{QueryLogRow, outcome_parts},
    error::{Result, StoreError},
    records::QueryLogEntry,
};

impl Store {
    /// Logs a query as started now. Records the reason's label, score and
    /// runner-up as columns and the whole reason as JSON, and the
    /// passenger party searched (0036), which its observations inherit.
    pub async fn start_query(
        &self,
        family: FamilyId,
        reason: &QueryReason,
        scheduled_at: DateTime<Utc>,
    ) -> Result<QueryId> {
        let label = reason.label();
        let (score, runner_up) = match reason {
            QueryReason::Scheduled { score, runner_up } => {
                (Some(*score), *runner_up)
            }
            _ => (None, None),
        };
        let json = serde_json::to_string(reason)?;
        let started_at = Utc::now();
        let mut tx = self.write().await?;
        let passengers = settings_in(&mut tx).await?.passengers.key();
        let done = sqlx::query!(
            "INSERT INTO queries (family_id, reason, score, runner_up,
                 reason_json, passengers, scheduled_at, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            family.0,
            label,
            score,
            runner_up,
            json,
            passengers,
            scheduled_at,
            started_at,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(QueryId(done.last_insert_rowid()))
    }

    /// Records how a query ended. A query is finished once.
    pub async fn finish_query(
        &self,
        id: QueryId,
        outcome: &QueryOutcome,
        page_loads: u32,
        jev_calls: u32,
        finished_at: DateTime<Utc>,
    ) -> Result<()> {
        let (label, detail) = outcome_parts(outcome);
        let json = serde_json::to_string(outcome)?;
        let done = sqlx::query!(
            "UPDATE queries SET outcome = ?2, outcome_detail = ?3,
                 outcome_json = ?4, page_loads = ?5, jev_calls = ?6,
                 finished_at = ?7
             WHERE id = ?1 AND finished_at IS NULL",
            id.0,
            label,
            detail,
            json,
            page_loads,
            jev_calls,
            finished_at,
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("unfinished query", id));
        }
        Ok(())
    }

    /// The latest queries, newest first, with their family and route.
    pub async fn recent_queries(
        &self,
        limit: u32,
    ) -> Result<Vec<QueryLogEntry>> {
        let rows = sqlx::query_as!(
            QueryLogRow,
            r#"SELECT q.id, q.family_id, f.route_id, f.cabin, f.source,
                      f.source_code, f.pos_id, f.kind, f.leg, f.days, f.site,
                      o.code AS origin_code, p.code AS destination_code,
                      r.sweep AS "sweep: bool",
                      q.reason_json, q.score, q.runner_up,
                      q.scheduled_at AS "scheduled_at: DateTime<Utc>",
                      q.started_at AS "started_at: DateTime<Utc>",
                      q.finished_at AS "finished_at: DateTime<Utc>",
                      q.outcome_json, q.page_loads, q.jev_calls,
                      (SELECT COUNT(*) FROM observations ob
                       WHERE ob.query_id = q.id) AS "cells!: i64"
               FROM queries q
               JOIN families f ON f.id = q.family_id
               JOIN routes r ON r.id = f.route_id
               JOIN places o ON o.id = r.origin_id
               JOIN places p ON p.id = r.destination_id
               ORDER BY q.started_at DESC, q.id DESC
               LIMIT ?1"#,
            limit
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(QueryLogEntry::try_from).collect()
    }
}
