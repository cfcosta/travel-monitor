//! Destinations the Explore sweep prices outside the watchlist, and
//! their drops (DECISIONS.md 0087).
//!
//! A hint is *watched* when its destination is a configured place, an
//! origin or a destination, active or removed (a removed destination
//! was a choice): its airport is that place's code, its Freebase ID is
//! that city's, or another hint put the same Freebase ID at a watched
//! airport (Explore filing MIA under "Miami Beach"). The others are the
//! raw material of `tm_scheduler::finds`.

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{Cabin, ExploreHint, Money, PlaceId};

use crate::{
    Store,
    convert::{HintRow, cabin_name, money, opt_count, parse_cabin},
    error::Result,
};

/// A drop of an unwatched destination's Explore price under its own
/// history: the daemon notifies once per drop.
#[derive(Debug, Clone, PartialEq)]
pub struct ExploreDrop {
    pub id: i64,
    pub origin: PlaceId,
    pub cabin: Cabin,
    pub destination_freebase_id: String,
    pub destination_airport: String,
    pub destination_name: Option<String>,
    pub price: Money,
    /// The median of the destination's older Explore prices.
    pub usual: Money,
    pub departure: Option<NaiveDate>,
    pub return_date: Option<NaiveDate>,
    pub carrier: Option<String>,
    pub stops: Option<u8>,
    pub found_at: DateTime<Utc>,
    pub notified_at: Option<DateTime<Utc>>,
}

impl ExploreDrop {
    /// `-0.25` for 25% under the usual price.
    pub fn change(&self) -> Option<f64> {
        (self.usual.minor > 0)
            .then(|| self.price.minor as f64 / self.usual.minor as f64 - 1.0)
    }
}

struct DropRow {
    id: i64,
    origin_id: i64,
    cabin: String,
    destination_freebase_id: String,
    destination_airport: String,
    destination_name: Option<String>,
    price_minor: i64,
    median_minor: i64,
    currency: String,
    departure: Option<NaiveDate>,
    return_date: Option<NaiveDate>,
    carrier: Option<String>,
    stops: Option<i64>,
    found_at: DateTime<Utc>,
    notified_at: Option<DateTime<Utc>>,
}

impl TryFrom<DropRow> for ExploreDrop {
    type Error = crate::StoreError;

    fn try_from(row: DropRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            origin: PlaceId(row.origin_id),
            cabin: parse_cabin(&row.cabin)?,
            destination_freebase_id: row.destination_freebase_id,
            destination_airport: row.destination_airport,
            destination_name: row.destination_name,
            price: money(row.price_minor, row.currency.clone()),
            usual: money(row.median_minor, row.currency),
            departure: row.departure,
            return_date: row.return_date,
            carrier: row.carrier,
            stops: opt_count("stops", row.stops)?,
            found_at: row.found_at,
            notified_at: row.notified_at,
        })
    }
}

impl Store {
    /// Explore hints since `since`, for the current party, of
    /// destinations that are not configured places (see the module),
    /// oldest first.
    pub async fn unwatched_explore_hints(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<ExploreHint>> {
        let passengers = self.settings().await?.passengers.key();
        let rows = sqlx::query_as!(
            HintRow,
            r#"WITH watched AS (
                   SELECT code, freebase_id FROM places
                   WHERE id IN (SELECT place_id FROM destinations
                                UNION SELECT place_id FROM origins)
               )
               SELECT query_id, origin_id, cabin, pos_id,
                      observed_at AS "observed_at: DateTime<Utc>",
                      destination_freebase_id, destination_airport,
                      price_minor, currency,
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      stops, duration_minutes, carrier, destination_name,
                      destination_country, destination_lat, destination_lon
               FROM explore_hints h
               WHERE observed_at >= ?1 AND passengers = ?2
                 AND destination_airport NOT IN (SELECT code FROM watched)
                 AND destination_freebase_id NOT IN (
                     SELECT freebase_id FROM watched
                     WHERE freebase_id IS NOT NULL
                     UNION
                     SELECT w.destination_freebase_id FROM explore_hints w
                     WHERE w.observed_at >= ?1
                       AND w.destination_airport IN (SELECT code FROM watched))
               ORDER BY observed_at, id"#,
            since,
            passengers
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(ExploreHint::try_from).collect()
    }

    /// When the latest Explore hint was observed: finds only change
    /// when a sweep lands (or the watchlist does).
    pub async fn latest_explore_hint_at(
        &self,
    ) -> Result<Option<DateTime<Utc>>> {
        let at = sqlx::query_scalar!(
            r#"SELECT MAX(observed_at) AS "at: DateTime<Utc>"
               FROM explore_hints"#
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(at)
    }

    /// Opens a drop for `hint`'s origin, cabin and destination, unless
    /// one is open already. Returns the new drop's id.
    pub async fn start_explore_drop(
        &self,
        hint: &ExploreHint,
        usual: &Money,
        at: DateTime<Utc>,
    ) -> Result<Option<i64>> {
        let cabin = cabin_name(hint.cabin);
        let currency = hint.price.currency.as_str();
        let id = sqlx::query_scalar!(
            r#"INSERT INTO explore_drops (origin_id, cabin,
                   destination_freebase_id, destination_airport,
                   destination_name, price_minor, median_minor, currency,
                   departure, return_date, carrier, stops, found_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
               ON CONFLICT (origin_id, cabin, destination_freebase_id)
               WHERE ended_at IS NULL DO NOTHING
               RETURNING id AS "id!: i64""#,
            hint.origin.0,
            cabin,
            hint.destination_freebase_id,
            hint.destination_airport,
            hint.destination_name,
            hint.price.minor,
            usual.minor,
            currency,
            hint.departure,
            hint.return_date,
            hint.carrier,
            hint.stops,
            at,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(id)
    }

    /// Ends a drop: the price came back, or the destination left the
    /// sweep or joined the watchlist.
    pub async fn end_explore_drop(
        &self,
        id: i64,
        at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query!(
            "UPDATE explore_drops SET ended_at = ?2
             WHERE id = ?1 AND ended_at IS NULL",
            id,
            at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The open drops from `origin`, oldest first.
    pub async fn open_explore_drops(
        &self,
        origin: PlaceId,
    ) -> Result<Vec<ExploreDrop>> {
        let rows = sqlx::query_as!(
            DropRow,
            r#"SELECT id, origin_id, cabin, destination_freebase_id,
                      destination_airport, destination_name, price_minor,
                      median_minor, currency,
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      carrier, stops,
                      found_at AS "found_at: DateTime<Utc>",
                      notified_at AS "notified_at: DateTime<Utc>"
               FROM explore_drops
               WHERE origin_id = ?1 AND ended_at IS NULL
               ORDER BY found_at, id"#,
            origin.0
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(ExploreDrop::try_from).collect()
    }

    /// Open drops not announced yet, oldest first.
    pub async fn unnotified_explore_drops(&self) -> Result<Vec<ExploreDrop>> {
        let rows = sqlx::query_as!(
            DropRow,
            r#"SELECT id, origin_id, cabin, destination_freebase_id,
                      destination_airport, destination_name, price_minor,
                      median_minor, currency,
                      departure AS "departure: NaiveDate",
                      return_date AS "return_date: NaiveDate",
                      carrier, stops,
                      found_at AS "found_at: DateTime<Utc>",
                      notified_at AS "notified_at: DateTime<Utc>"
               FROM explore_drops
               WHERE ended_at IS NULL AND notified_at IS NULL
               ORDER BY found_at, id"#
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(ExploreDrop::try_from).collect()
    }

    /// The notification server took the drop's notice.
    pub async fn mark_explore_drop_notified(
        &self,
        id: i64,
        at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query!(
            "UPDATE explore_drops SET notified_at = ?2 WHERE id = ?1",
            id,
            at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
