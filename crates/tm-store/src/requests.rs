//! Work the app asks the daemon for (DECISIONS.md 0046). Requests
//! survive restarts of either program.

use chrono::{DateTime, Utc};
use tm_domain::RequestId;

use crate::{
    Store,
    convert::{RequestRow, request_status_name},
    error::{Result, StoreError},
    records::{RequestKind, RequestRecord, RequestStatus},
};

impl Store {
    /// Queues a request.
    pub async fn enqueue(&self, kind: &RequestKind) -> Result<RequestId> {
        let name = kind.name();
        let payload = serde_json::to_string(kind)?;
        let now = Utc::now();
        let done = sqlx::query!(
            "INSERT INTO requests (kind, payload, status, created_at)
             VALUES (?1, ?2, 'pending', ?3)",
            name,
            payload,
            now
        )
        .execute(&self.pool)
        .await?;
        Ok(RequestId(done.last_insert_rowid()))
    }

    /// Takes the oldest pending request, marking it claimed. One
    /// statement, so two claimers never get the same request.
    pub async fn claim_next(&self) -> Result<Option<RequestRecord>> {
        let now = Utc::now();
        let row = sqlx::query_as!(
            RequestRow,
            r#"UPDATE requests SET status = 'claimed', claimed_at = ?1
               WHERE id = (SELECT id FROM requests WHERE status = 'pending'
                           ORDER BY id LIMIT 1)
               RETURNING id AS "id!: i64", payload AS "payload!: String",
                         status AS "status!: String",
                         created_at AS "created_at!: DateTime<Utc>",
                         claimed_at AS "claimed_at: DateTime<Utc>",
                         finished_at AS "finished_at: DateTime<Utc>",
                         detail AS "detail: String""#,
            now
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(RequestRecord::try_from).transpose()
    }

    /// Marks a claimed (or pending) request done or failed.
    pub async fn complete(
        &self,
        id: RequestId,
        ok: bool,
        detail: Option<&str>,
    ) -> Result<()> {
        let status = request_status_name(if ok {
            RequestStatus::Done
        } else {
            RequestStatus::Failed
        });
        let now = Utc::now();
        let done = sqlx::query!(
            "UPDATE requests SET status = ?2, detail = ?3, finished_at = ?4
             WHERE id = ?1 AND status IN ('pending', 'claimed')",
            id.0,
            status,
            detail,
            now
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("open request", id));
        }
        Ok(())
    }

    /// Puts requests claimed by a daemon that stopped before finishing
    /// them back in the queue. The daemon calls this when it starts.
    pub async fn requeue_claimed(&self) -> Result<u64> {
        let done = sqlx::query!(
            "UPDATE requests SET status = 'pending', claimed_at = NULL
             WHERE status = 'claimed'"
        )
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    /// Requests not finished yet (pending or claimed), oldest first.
    pub async fn pending_requests(&self) -> Result<Vec<RequestRecord>> {
        let rows = sqlx::query_as!(
            RequestRow,
            r#"SELECT id, payload, status,
                      created_at AS "created_at: DateTime<Utc>",
                      claimed_at AS "claimed_at: DateTime<Utc>",
                      finished_at AS "finished_at: DateTime<Utc>",
                      detail
               FROM requests WHERE status IN ('pending', 'claimed')
               ORDER BY id"#
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(RequestRecord::try_from).collect()
    }

    pub async fn request(
        &self,
        id: RequestId,
    ) -> Result<Option<RequestRecord>> {
        let row = sqlx::query_as!(
            RequestRow,
            r#"SELECT id, payload, status,
                      created_at AS "created_at: DateTime<Utc>",
                      claimed_at AS "claimed_at: DateTime<Utc>",
                      finished_at AS "finished_at: DateTime<Utc>",
                      detail
               FROM requests WHERE id = ?1"#,
            id.0
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(RequestRecord::try_from).transpose()
    }
}
