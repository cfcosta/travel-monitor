//! Deals: candidate → confirmed / rejected / expired, seller quotes, and
//! the app's `seen` flag (DECISIONS.md 0030, 0031, 0034, 0040).

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::SqliteConnection;
use tm_domain::{Deal, DealId, DealStatus, Money, SellerQuote};

use crate::{
    Store,
    convert::{
        DealRow,
        SellerQuoteRow,
        cabin_name,
        deal_reason_name,
        deal_status_name,
        money_parts,
    },
    error::{Result, StoreError},
    records::DealFilter,
};

async fn insert_quotes(
    conn: &mut SqliteConnection,
    deal: DealId,
    quotes: &[SellerQuote],
) -> Result<()> {
    for q in quotes {
        let currency = q.price.currency.as_str();
        let (quoted_minor, quoted_currency) = money_parts(q.quoted.as_ref());
        sqlx::query!(
            "INSERT INTO seller_quotes (deal_id, pos_id, seller, is_airline,
                 price_minor, currency, quoted_minor, quoted_currency)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            deal.0,
            q.pos.0,
            q.seller,
            q.is_airline,
            q.price.minor,
            currency,
            quoted_minor,
            quoted_currency,
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

impl Store {
    /// Records a deal (usually a candidate) with its seller quotes.
    /// `id` and `seen` are ignored: a new deal is unseen.
    pub async fn insert_deal(&self, deal: &Deal) -> Result<DealId> {
        let cabin = cabin_name(deal.cabin);
        let reason = deal_reason_name(deal.reason);
        let status = deal_status_name(deal.status);
        let currency = deal.price.currency.as_str();
        let effective_currency = deal.effective_brl.currency.as_str();
        let (baseline_minor, baseline_currency) =
            money_parts(deal.baseline.as_ref());
        let mut tx = self.write().await?;
        let done = sqlx::query!(
            "INSERT INTO deals (route_id, cabin, pos_id, departure, return_date,
                 price_minor, currency, effective_minor, effective_currency,
                 baseline_minor, baseline_currency, reason, change, estimated,
                 status, found_at, confirmed_at, seen, carrier, seller, url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?16, ?17, 0, ?18, ?19, ?20)",
            deal.route.0,
            cabin,
            deal.pos.0,
            deal.departure,
            deal.return_date,
            deal.price.minor,
            currency,
            deal.effective_brl.minor,
            effective_currency,
            baseline_minor,
            baseline_currency,
            reason,
            deal.change,
            deal.estimated,
            status,
            deal.found_at,
            deal.confirmed_at,
            deal.carrier,
            deal.seller,
            deal.url,
        )
        .execute(&mut *tx)
        .await?;
        let id = DealId(done.last_insert_rowid());
        insert_quotes(&mut tx, id, &deal.sellers).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Moves a deal to `status` at `at`, which is recorded as
    /// `confirmed_at`, `rejected_at` or `expired_at`.
    pub async fn set_deal_status(
        &self,
        id: DealId,
        status: DealStatus,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let name = deal_status_name(status);
        let done = sqlx::query!(
            "UPDATE deals SET status = ?2,
                 confirmed_at = CASE WHEN ?2 = 'confirmed' THEN ?3
                                     ELSE confirmed_at END,
                 rejected_at = CASE WHEN ?2 = 'rejected' THEN ?3
                                    ELSE rejected_at END,
                 expired_at = CASE WHEN ?2 = 'expired' THEN ?3
                                   ELSE expired_at END
             WHERE id = ?1",
            id.0,
            name,
            at
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("deal", id));
        }
        Ok(())
    }

    /// Records what a confirmation found (0031, 0034): the price the
    /// second query saw, its effective BRL cost, the carrier, the
    /// cheapest seller and the URL that shows it. `None` keeps the
    /// stored carrier, seller or URL.
    pub async fn update_deal_quote(
        &self,
        id: DealId,
        price: &Money,
        effective_brl: &Money,
        carrier: Option<&str>,
        seller: Option<&str>,
        url: Option<&str>,
    ) -> Result<()> {
        let currency = price.currency.as_str();
        let effective_currency = effective_brl.currency.as_str();
        let done = sqlx::query!(
            "UPDATE deals SET price_minor = ?2, currency = ?3,
                 effective_minor = ?4, effective_currency = ?5,
                 carrier = COALESCE(?6, carrier),
                 seller = COALESCE(?7, seller),
                 url = COALESCE(?8, url)
             WHERE id = ?1",
            id.0,
            price.minor,
            currency,
            effective_brl.minor,
            effective_currency,
            carrier,
            seller,
            url
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("deal", id));
        }
        Ok(())
    }

    /// Adds seller quotes from a booking page (0034).
    pub async fn attach_seller_quotes(
        &self,
        id: DealId,
        quotes: &[SellerQuote],
    ) -> Result<()> {
        let mut tx = self.write().await?;
        insert_quotes(&mut tx, id, quotes).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The app has shown the deal (0040).
    pub async fn mark_deal_seen(&self, id: DealId) -> Result<()> {
        let done =
            sqlx::query!("UPDATE deals SET seen = 1 WHERE id = ?1", id.0)
                .execute(&self.pool)
                .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("deal", id));
        }
        Ok(())
    }

    pub async fn deal(&self, id: DealId) -> Result<Option<Deal>> {
        let row = sqlx::query_as!(
            DealRow,
            r#"SELECT id, route_id, cabin, pos_id,
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      price_minor, currency, effective_minor,
                      effective_currency, baseline_minor, baseline_currency,
                      reason, change, estimated AS "estimated: bool", status,
                      found_at AS "found_at: DateTime<Utc>",
                      confirmed_at AS "confirmed_at: DateTime<Utc>",
                      seen AS "seen: bool", carrier, seller, url
               FROM deals WHERE id = ?1"#,
            id.0
        )
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(self.with_sellers(vec![row]).await?.pop()),
            None => Ok(None),
        }
    }

    /// Deals, newest first.
    pub async fn list_deals(&self, filter: &DealFilter) -> Result<Vec<Deal>> {
        let status = filter.status.map(deal_status_name);
        let limit = filter.limit.map_or(-1, i64::from);
        let rows = sqlx::query_as!(
            DealRow,
            r#"SELECT id, route_id, cabin, pos_id,
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      price_minor, currency, effective_minor,
                      effective_currency, baseline_minor, baseline_currency,
                      reason, change, estimated AS "estimated: bool", status,
                      found_at AS "found_at: DateTime<Utc>",
                      confirmed_at AS "confirmed_at: DateTime<Utc>",
                      seen AS "seen: bool", carrier, seller, url
               FROM deals
               WHERE (?1 IS NULL OR status = ?1) AND (?2 = 0 OR seen = 0)
               ORDER BY found_at DESC, id DESC
               LIMIT ?3"#,
            status,
            filter.unseen_only,
            limit
        )
        .fetch_all(&self.pool)
        .await?;
        self.with_sellers(rows).await
    }

    /// Confirmed deals the app has not shown yet, oldest first, for its
    /// notifications (0040).
    pub async fn unseen_confirmed(&self) -> Result<Vec<Deal>> {
        let mut deals = self
            .list_deals(&DealFilter {
                status: Some(DealStatus::Confirmed),
                unseen_only: true,
                limit: None,
            })
            .await?;
        deals.reverse();
        Ok(deals)
    }

    async fn with_sellers(&self, rows: Vec<DealRow>) -> Result<Vec<Deal>> {
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let ids = serde_json::to_string(&ids)?;
        let quotes = sqlx::query_as!(
            SellerQuoteRow,
            r#"SELECT deal_id, pos_id, seller, is_airline AS "is_airline: bool",
                      price_minor, currency, quoted_minor, quoted_currency
               FROM seller_quotes
               WHERE deal_id IN (SELECT value FROM json_each(?1))
               ORDER BY id"#,
            ids
        )
        .fetch_all(&self.pool)
        .await?;
        let mut by_deal: HashMap<i64, Vec<SellerQuote>> = HashMap::new();
        for q in quotes {
            let deal = q.deal_id;
            by_deal.entry(deal).or_default().push(q.try_into()?);
        }
        rows.into_iter()
            .map(|row| {
                let sellers = by_deal.remove(&row.id).unwrap_or_default();
                row.into_deal(sellers)
            })
            .collect()
    }
}
