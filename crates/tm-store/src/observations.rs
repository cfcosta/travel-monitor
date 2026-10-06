//! Observations (append-only, DECISIONS.md 0020) and Explore hints
//! (0041).
//!
//! Every observation and hint carries the passenger party of its query
//! (0036). Reads return only the current party's series: changing the
//! passengers starts new series.

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{
    Cabin,
    ExploreHint,
    FamilyId,
    Observation,
    ObservationId,
    RouteId,
};

use crate::{
    Store,
    convert::{HintRow, ObservationRow, cabin_name, money_parts},
    error::{Result, StoreError},
};

impl Store {
    /// Inserts a batch of observations in one transaction. Each must
    /// name a logged query of the same family; `id` is ignored. Returns
    /// the new ids, in order.
    pub async fn insert_observations(
        &self,
        observations: &[Observation],
    ) -> Result<Vec<ObservationId>> {
        let mut tx = self.write().await?;
        let mut ids = Vec::with_capacity(observations.len());
        for o in observations {
            let (price_minor, currency) = money_parts(o.price.as_ref());
            let done = sqlx::query!(
                "INSERT INTO observations (query_id, family_id, observed_at,
                     departure, return_date, price_minor, currency, miles,
                     carrier, stops, duration_minutes, seller, passengers)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        q.passengers
                 FROM queries q WHERE q.id = ?1 AND q.family_id = ?2",
                o.query.0,
                o.family.0,
                o.observed_at,
                o.departure,
                o.return_date,
                price_minor,
                currency,
                o.miles,
                o.carrier,
                o.stops,
                o.duration_minutes,
                o.seller,
            )
            .execute(&mut *tx)
            .await?;
            if done.rows_affected() == 0 {
                return Err(StoreError::not_found(
                    "query of this family",
                    format!("{} (family {})", o.query, o.family),
                ));
            }
            ids.push(ObservationId(done.last_insert_rowid()));
        }
        tx.commit().await?;
        Ok(ids)
    }

    /// The cells of the family's latest successful query (outcome `ok`,
    /// with at least one observation), by departure date.
    pub async fn latest_cells(
        &self,
        family: FamilyId,
    ) -> Result<Vec<Observation>> {
        let passengers = self.settings().await?.passengers.key();
        let rows = sqlx::query_as!(
            ObservationRow,
            r#"SELECT id, query_id, family_id,
                      observed_at AS "observed_at: DateTime<Utc>",
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      price_minor, currency, miles, carrier, stops,
                      duration_minutes, seller
               FROM observations
               WHERE query_id = (
                   SELECT q.id FROM queries q
                   WHERE q.family_id = ?1 AND q.outcome = 'ok'
                     AND q.passengers = ?2
                     AND EXISTS (SELECT 1 FROM observations x
                                 WHERE x.query_id = q.id)
                   ORDER BY q.finished_at DESC, q.id DESC
                   LIMIT 1)
               ORDER BY departure, return_date"#,
            family.0,
            passengers
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Observation::try_from).collect()
    }

    /// A family's observations since `since`, oldest first.
    pub async fn history(
        &self,
        family: FamilyId,
        since: DateTime<Utc>,
    ) -> Result<Vec<Observation>> {
        let passengers = self.settings().await?.passengers.key();
        let rows = sqlx::query_as!(
            ObservationRow,
            r#"SELECT id, query_id, family_id,
                      observed_at AS "observed_at: DateTime<Utc>",
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      price_minor, currency, miles, carrier, stops,
                      duration_minutes, seller
               FROM observations
               WHERE family_id = ?1 AND observed_at >= ?2
                 AND passengers = ?3
               ORDER BY observed_at, id"#,
            family.0,
            since,
            passengers
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Observation::try_from).collect()
    }

    /// Observations of every family (any source, point of sale or kind)
    /// of a route and cabin since `since`, oldest first.
    pub async fn history_for_route(
        &self,
        route: RouteId,
        cabin: Cabin,
        since: DateTime<Utc>,
    ) -> Result<Vec<Observation>> {
        let passengers = self.settings().await?.passengers.key();
        let cabin = cabin_name(cabin);
        let rows = sqlx::query_as!(
            ObservationRow,
            r#"SELECT o.id, o.query_id, o.family_id,
                      o.observed_at AS "observed_at: DateTime<Utc>",
                      o.departure AS "departure: NaiveDate",
                      o.return_date AS "return_date: NaiveDate",
                      o.price_minor, o.currency, o.miles, o.carrier, o.stops,
                      o.duration_minutes, o.seller
               FROM observations o JOIN families f ON f.id = o.family_id
               WHERE f.route_id = ?1 AND f.cabin = ?2 AND o.observed_at >= ?3
                 AND o.passengers = ?4
               ORDER BY o.observed_at, o.id"#,
            route.0,
            cabin,
            since,
            passengers
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Observation::try_from).collect()
    }

    /// Inserts Explore hints in one transaction. Each must name a logged
    /// query.
    pub async fn insert_explore_hints(
        &self,
        hints: &[ExploreHint],
    ) -> Result<()> {
        let mut tx = self.write().await?;
        for h in hints {
            let cabin = cabin_name(h.cabin);
            let currency = h.price.currency.as_str();
            let done = sqlx::query!(
                "INSERT INTO explore_hints (query_id, origin_id, cabin, pos_id,
                     observed_at, destination_freebase_id, destination_airport,
                     price_minor, currency, departure, return_date, stops,
                     duration_minutes, carrier, passengers)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                        ?14, q.passengers
                 FROM queries q WHERE q.id = ?1",
                h.query.0,
                h.origin.0,
                cabin,
                h.pos.0,
                h.observed_at,
                h.destination_freebase_id,
                h.destination_airport,
                h.price.minor,
                currency,
                h.departure,
                h.return_date,
                h.stops,
                h.duration_minutes,
                h.carrier,
            )
            .execute(&mut *tx)
            .await?;
            if done.rows_affected() == 0 {
                return Err(StoreError::not_found("query", h.query));
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Explore hints observed since `since` for the current party,
    /// oldest first.
    pub async fn explore_hints_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<ExploreHint>> {
        let passengers = self.settings().await?.passengers.key();
        let rows = sqlx::query_as!(
            HintRow,
            r#"SELECT query_id, origin_id, cabin, pos_id,
                      observed_at AS "observed_at: DateTime<Utc>",
                      destination_freebase_id, destination_airport,
                      price_minor, currency,
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      stops, duration_minutes, carrier
               FROM explore_hints
               WHERE observed_at >= ?1 AND passengers = ?2
               ORDER BY observed_at, id"#,
            since,
            passengers
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(ExploreHint::try_from).collect()
    }
}
