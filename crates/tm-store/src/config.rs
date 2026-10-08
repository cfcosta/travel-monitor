//! Configuration edited in the app (DECISIONS.md 0035): places, points
//! of sale, settings, airline and miles sources, and the derived routes.

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;
use tm_domain::{
    CountryCode,
    Currency,
    Place,
    PlaceId,
    PointOfSale,
    PosId,
    Route,
    RouteId,
    Settings,
};

use crate::{
    Store,
    convert::{
        PlaceRow,
        PosRow,
        RouteRow,
        SettingsRow,
        award_dates_name,
        count,
        login_state_name,
        parse_award_dates,
        parse_login_state,
        place_kind_parts,
    },
    error::{Result, StoreError},
    records::{
        AirlineSource,
        Destination,
        LoginState,
        MilesProgram,
        NewPlace,
        Origin,
        RouteTripState,
    },
};

fn validate_place(place: &NewPlace) -> Result<String> {
    let code = place.code.trim().to_ascii_uppercase();
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(StoreError::Invalid(format!(
            "place code {:?}",
            place.code
        )));
    }
    if place.country.as_str().len() != 2 {
        return Err(StoreError::Invalid(format!(
            "country code {:?}",
            place.country.as_str()
        )));
    }
    if !(-90.0..=90.0).contains(&place.lat)
        || !(-180.0..=180.0).contains(&place.lon)
    {
        return Err(StoreError::Invalid(format!(
            "coordinates {}, {}",
            place.lat, place.lon
        )));
    }
    Ok(code)
}

/// Inserts a place, or updates the one with the same code (keeping a
/// known Freebase ID when the new one is unknown). Returns its id.
pub(crate) async fn upsert_place(
    conn: &mut SqliteConnection,
    place: &NewPlace,
) -> Result<PlaceId> {
    let code = validate_place(place)?;
    let (kind, freebase_id) = place_kind_parts(&place.kind);
    let country = place.country.as_str();
    sqlx::query!(
        "INSERT INTO places (code, name, country, kind, freebase_id, lat, lon)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (code) DO UPDATE SET
             name = excluded.name,
             country = excluded.country,
             kind = excluded.kind,
             freebase_id = CASE WHEN excluded.kind = 'city'
                 THEN COALESCE(excluded.freebase_id, places.freebase_id)
                 ELSE NULL END,
             lat = excluded.lat,
             lon = excluded.lon",
        code,
        place.name,
        country,
        kind,
        freebase_id,
        place.lat,
        place.lon,
    )
    .execute(&mut *conn)
    .await?;
    let id = sqlx::query_scalar!("SELECT id FROM places WHERE code = ?1", code)
        .fetch_one(&mut *conn)
        .await?;
    Ok(PlaceId(id))
}

pub(crate) async fn add_origin_in(
    conn: &mut SqliteConnection,
    place: &NewPlace,
) -> Result<PlaceId> {
    let id = upsert_place(conn, place).await?;
    sqlx::query!(
        "INSERT INTO origins (place_id, active) VALUES (?1, 1)
         ON CONFLICT (place_id) DO UPDATE SET active = 1",
        id.0
    )
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

pub(crate) async fn add_destination_in(
    conn: &mut SqliteConnection,
    place: &NewPlace,
    weight: f64,
) -> Result<PlaceId> {
    check_weight(weight)?;
    let id = upsert_place(conn, place).await?;
    sqlx::query!(
        "INSERT INTO destinations (place_id, weight, active) VALUES (?1, ?2, 1)
         ON CONFLICT (place_id) DO UPDATE SET active = 1, weight = ?2",
        id.0,
        weight
    )
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

fn check_weight(weight: f64) -> Result<()> {
    if weight.is_finite() && weight >= 0.0 {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!("weight {weight}")))
    }
}

pub(crate) async fn add_pos_in(
    conn: &mut SqliteConnection,
    country: &CountryCode,
    currency: &Currency,
    is_home: bool,
) -> Result<PosId> {
    let country = country.as_str().trim().to_ascii_uppercase();
    let currency = currency.as_str().trim().to_ascii_uppercase();
    if country.len() != 2 || currency.len() != 3 {
        return Err(StoreError::Invalid(format!(
            "point of sale {country} {currency}"
        )));
    }
    if is_home {
        sqlx::query!(
            "UPDATE points_of_sale SET is_home = 0
             WHERE is_home = 1 AND country != ?1",
            country
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query!(
        "INSERT INTO points_of_sale (country, currency, is_home, active)
         VALUES (?1, ?2, ?3, 1)
         ON CONFLICT (country) DO UPDATE SET
             currency = excluded.currency,
             is_home = excluded.is_home,
             active = 1",
        country,
        currency,
        is_home
    )
    .execute(&mut *conn)
    .await?;
    let id = sqlx::query_scalar!(
        "SELECT id FROM points_of_sale WHERE country = ?1",
        country
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(PosId(id))
}

pub(crate) async fn set_settings_in(
    conn: &mut SqliteConnection,
    s: &Settings,
) -> Result<()> {
    s.validate()?;
    let p = s.passengers;
    let t = s.trips;
    sqlx::query!(
        "INSERT INTO settings (
             id, adults, children, infants_in_seat, infants_on_lap,
             trip_min_days, trip_max_days, round_trip_days, horizon_days,
             iof, card_spread, daily_budget, max_daily_budget,
             max_interval_hours, min_interval_hours, random_share,
             deal_threshold, min_history_weeks, additive_checks_to_stop,
             language, unusual_rate, favorite_deal_threshold)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
         ON CONFLICT (id) DO UPDATE SET
             adults = excluded.adults,
             children = excluded.children,
             infants_in_seat = excluded.infants_in_seat,
             infants_on_lap = excluded.infants_on_lap,
             trip_min_days = excluded.trip_min_days,
             trip_max_days = excluded.trip_max_days,
             round_trip_days = excluded.round_trip_days,
             horizon_days = excluded.horizon_days,
             iof = excluded.iof,
             card_spread = excluded.card_spread,
             daily_budget = excluded.daily_budget,
             max_daily_budget = excluded.max_daily_budget,
             max_interval_hours = excluded.max_interval_hours,
             min_interval_hours = excluded.min_interval_hours,
             random_share = excluded.random_share,
             deal_threshold = excluded.deal_threshold,
             min_history_weeks = excluded.min_history_weeks,
             additive_checks_to_stop = excluded.additive_checks_to_stop,
             language = excluded.language,
             unusual_rate = excluded.unusual_rate,
             favorite_deal_threshold = excluded.favorite_deal_threshold",
        p.adults,
        p.children,
        p.infants_in_seat,
        p.infants_on_lap,
        t.min_days,
        t.max_days,
        t.round_trip_days,
        s.horizon_days,
        s.iof,
        s.card_spread,
        s.daily_budget,
        s.max_daily_budget,
        s.max_interval_hours,
        s.min_interval_hours,
        s.random_share,
        s.deal_threshold,
        s.min_history_weeks,
        s.additive_checks_to_stop,
        s.language,
        s.unusual_rate,
        s.favorite_deal_threshold,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn settings_in(
    conn: &mut SqliteConnection,
) -> Result<Settings> {
    let row = sqlx::query_as!(
        SettingsRow,
        "SELECT adults, children, infants_in_seat, infants_on_lap,
                trip_min_days, trip_max_days, round_trip_days, horizon_days,
                iof, card_spread, daily_budget, max_daily_budget,
                max_interval_hours, min_interval_hours, random_share,
                deal_threshold, min_history_weeks, additive_checks_to_stop,
                language, unusual_rate, favorite_deal_threshold
         FROM settings WHERE id = 1"
    )
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some(row) => row.try_into(),
        None => Ok(Settings::default()),
    }
}

pub(crate) async fn upsert_airline_in(
    conn: &mut SqliteConnection,
    code: &str,
    name: &str,
    active: bool,
) -> Result<()> {
    let code = code.trim().to_ascii_uppercase();
    if code.is_empty() {
        return Err(StoreError::Invalid("empty airline code".into()));
    }
    sqlx::query!(
        "INSERT INTO airline_sources (code, name, active) VALUES (?1, ?2, ?3)
         ON CONFLICT (code) DO UPDATE SET name = excluded.name,
             active = excluded.active",
        code,
        name,
        active
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn set_airline_destinations_in(
    conn: &mut SqliteConnection,
    code: &str,
    places: &[PlaceId],
) -> Result<()> {
    let code = code.trim().to_ascii_uppercase();
    let exists = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM airline_sources WHERE code = ?1"#,
        code
    )
    .fetch_one(&mut *conn)
    .await?;
    if exists == 0 {
        return Err(StoreError::not_found("airline", code));
    }
    sqlx::query!(
        "DELETE FROM airline_destinations WHERE airline_code = ?1",
        code
    )
    .execute(&mut *conn)
    .await?;
    for place in places {
        sqlx::query!(
            "INSERT OR IGNORE INTO airline_destinations (airline_code, place_id)
             VALUES (?1, ?2)",
            code,
            place.0
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub(crate) async fn upsert_miles_in(
    conn: &mut SqliteConnection,
    program: &str,
    name: &str,
    active: bool,
) -> Result<()> {
    let program = program.trim();
    if program.is_empty() {
        return Err(StoreError::Invalid("empty miles program".into()));
    }
    sqlx::query!(
        "INSERT INTO miles_programs (program, name, active) VALUES (?1, ?2, ?3)
         ON CONFLICT (program) DO UPDATE SET name = excluded.name,
             active = excluded.active",
        program,
        name,
        active
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Every active origin × active destination becomes an active route;
/// every other route goes inactive. Each origin has a sweep
/// pseudo-route, active while the origin is. Returns the number of
/// active (real) routes.
pub(crate) async fn sync_routes_in(
    conn: &mut SqliteConnection,
) -> Result<usize> {
    sqlx::query!(
        "INSERT INTO routes (origin_id, destination_id, sweep, active)
         SELECT o.place_id, d.place_id, 0, 1
         FROM origins o JOIN destinations d
         WHERE o.active = 1 AND d.active = 1 AND o.place_id != d.place_id
         ON CONFLICT (origin_id, destination_id) DO UPDATE SET active = 1"
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE routes SET active = 0
         WHERE sweep = 0 AND active = 1 AND NOT EXISTS (
             SELECT 1 FROM origins o, destinations d
             WHERE o.place_id = routes.origin_id
               AND d.place_id = routes.destination_id
               AND o.active = 1 AND d.active = 1)"
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "INSERT INTO routes (origin_id, destination_id, sweep, active)
         SELECT place_id, place_id, 1, active FROM origins WHERE true
         ON CONFLICT (origin_id, destination_id) DO UPDATE SET
             active = excluded.active"
    )
    .execute(&mut *conn)
    .await?;
    let n = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM routes
           WHERE sweep = 0 AND active = 1"#
    )
    .fetch_one(&mut *conn)
    .await?;
    count("routes", n)
}

impl Store {
    // -----------------------------------------------------------------
    // Places
    // -----------------------------------------------------------------

    /// Adds an origin (or reactivates it). A place with the same code is
    /// updated in place.
    pub async fn add_origin(&self, place: &NewPlace) -> Result<Place> {
        let mut tx = self.write().await?;
        let id = add_origin_in(&mut tx, place).await?;
        tx.commit().await?;
        self.place(id)
            .await?
            .ok_or_else(|| StoreError::not_found("place", id))
    }

    /// Adds a destination with a scheduler weight (or reactivates it and
    /// sets the weight).
    pub async fn add_destination(
        &self,
        place: &NewPlace,
        weight: f64,
    ) -> Result<Place> {
        let mut tx = self.write().await?;
        let id = add_destination_in(&mut tx, place, weight).await?;
        tx.commit().await?;
        self.place(id)
            .await?
            .ok_or_else(|| StoreError::not_found("place", id))
    }

    /// Updates a place's code, name, country, kind and coordinates.
    pub async fn update_place(&self, place: &Place) -> Result<()> {
        let new = NewPlace {
            code: place.code.clone(),
            name: place.name.clone(),
            country: place.country.clone(),
            kind: place.kind.clone(),
            lat: place.lat,
            lon: place.lon,
        };
        let code = validate_place(&new)?;
        let (kind, freebase_id) = place_kind_parts(&place.kind);
        let country = place.country.as_str();
        let done = sqlx::query!(
            "UPDATE places SET code = ?2, name = ?3, country = ?4, kind = ?5,
                 freebase_id = ?6, lat = ?7, lon = ?8
             WHERE id = ?1",
            place.id.0,
            code,
            place.name,
            country,
            kind,
            freebase_id,
            place.lat,
            place.lon,
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("place", place.id));
        }
        Ok(())
    }

    /// Stops using a place as an origin. Its routes go inactive at the
    /// next `sync_routes`, keeping their history.
    pub async fn remove_origin(&self, place: PlaceId) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE origins SET active = 0 WHERE place_id = ?1",
            place.0
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("origin", place));
        }
        Ok(())
    }

    /// Stops using a place as a destination (soft: see `remove_origin`).
    pub async fn remove_destination(&self, place: PlaceId) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE destinations SET active = 0 WHERE place_id = ?1",
            place.0
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("destination", place));
        }
        Ok(())
    }

    pub async fn set_destination_weight(
        &self,
        place: PlaceId,
        weight: f64,
    ) -> Result<()> {
        check_weight(weight)?;
        let done = sqlx::query!(
            "UPDATE destinations SET weight = ?2 WHERE place_id = ?1",
            place.0,
            weight
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("destination", place));
        }
        Ok(())
    }

    /// Stars or unstars a destination (DECISIONS.md 0094): every route
    /// to it becomes, or stops being, a favorite. A removed destination
    /// keeps its star for when it is added back.
    pub async fn set_destination_favorite(
        &self,
        place: PlaceId,
        favorite: bool,
    ) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE destinations SET favorite = ?2 WHERE place_id = ?1",
            place.0,
            favorite
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("destination", place));
        }
        Ok(())
    }

    /// Records a city's Freebase ID, learned with a `q=` search (0041).
    pub async fn set_freebase_id(
        &self,
        place: PlaceId,
        freebase_id: &str,
    ) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE places SET freebase_id = ?2 WHERE id = ?1 AND kind = 'city'",
            place.0,
            freebase_id
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("city", place));
        }
        Ok(())
    }

    pub async fn place(&self, id: PlaceId) -> Result<Option<Place>> {
        let row = sqlx::query_as!(
            PlaceRow,
            "SELECT id, code, name, country, kind, freebase_id, lat, lon
             FROM places WHERE id = ?1",
            id.0
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Place::try_from).transpose()
    }

    pub async fn place_by_code(&self, code: &str) -> Result<Option<Place>> {
        let code = code.trim().to_ascii_uppercase();
        let row = sqlx::query_as!(
            PlaceRow,
            "SELECT id, code, name, country, kind, freebase_id, lat, lon
             FROM places WHERE code = ?1",
            code
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Place::try_from).transpose()
    }

    /// Origins by code; `active_only` leaves removed ones out.
    pub async fn list_origins(&self, active_only: bool) -> Result<Vec<Origin>> {
        let rows = sqlx::query!(
            r#"SELECT p.id, p.code, p.name, p.country, p.kind, p.freebase_id,
                      p.lat, p.lon, o.active AS "active: bool"
               FROM origins o JOIN places p ON p.id = o.place_id
               WHERE (?1 = 0 OR o.active = 1)
               ORDER BY p.code"#,
            active_only
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let place = PlaceRow {
                    id: r.id,
                    code: r.code,
                    name: r.name,
                    country: r.country,
                    kind: r.kind,
                    freebase_id: r.freebase_id,
                    lat: r.lat,
                    lon: r.lon,
                };
                Ok(Origin {
                    place: place.try_into()?,
                    active: r.active,
                })
            })
            .collect()
    }

    /// Destinations by code; `active_only` leaves removed ones out.
    pub async fn list_destinations(
        &self,
        active_only: bool,
    ) -> Result<Vec<Destination>> {
        let rows = sqlx::query!(
            r#"SELECT p.id, p.code, p.name, p.country, p.kind, p.freebase_id,
                      p.lat, p.lon, d.weight, d.active AS "active: bool",
                      d.favorite AS "favorite: bool"
               FROM destinations d JOIN places p ON p.id = d.place_id
               WHERE (?1 = 0 OR d.active = 1)
               ORDER BY p.code"#,
            active_only
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let place = PlaceRow {
                    id: r.id,
                    code: r.code,
                    name: r.name,
                    country: r.country,
                    kind: r.kind,
                    freebase_id: r.freebase_id,
                    lat: r.lat,
                    lon: r.lon,
                };
                Ok(Destination {
                    place: place.try_into()?,
                    weight: r.weight,
                    active: r.active,
                    favorite: r.favorite,
                })
            })
            .collect()
    }

    // -----------------------------------------------------------------
    // Points of sale
    // -----------------------------------------------------------------

    /// Adds a point of sale (or reactivates and updates it). Making one
    /// the home market clears the flag on the others.
    pub async fn add_point_of_sale(
        &self,
        country: &CountryCode,
        currency: &Currency,
        is_home: bool,
    ) -> Result<PointOfSale> {
        let mut tx = self.write().await?;
        let id = add_pos_in(&mut tx, country, currency, is_home).await?;
        tx.commit().await?;
        self.point_of_sale(id)
            .await?
            .ok_or_else(|| StoreError::not_found("point of sale", id))
    }

    pub async fn set_point_of_sale_active(
        &self,
        id: PosId,
        active: bool,
    ) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE points_of_sale SET active = ?2 WHERE id = ?1",
            id.0,
            active
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("point of sale", id));
        }
        Ok(())
    }

    pub async fn point_of_sale(
        &self,
        id: PosId,
    ) -> Result<Option<PointOfSale>> {
        let row = sqlx::query_as!(
            PosRow,
            r#"SELECT id, country, currency, is_home AS "is_home: bool",
                      active AS "active: bool"
               FROM points_of_sale WHERE id = ?1"#,
            id.0
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(PointOfSale::from))
    }

    /// The home market (BR), whatever its active flag.
    pub async fn home_point_of_sale(&self) -> Result<Option<PointOfSale>> {
        let row = sqlx::query_as!(
            PosRow,
            r#"SELECT id, country, currency, is_home AS "is_home: bool",
                      active AS "active: bool"
               FROM points_of_sale WHERE is_home = 1"#
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(PointOfSale::from))
    }

    /// Points of sale, home first.
    pub async fn list_points_of_sale(
        &self,
        active_only: bool,
    ) -> Result<Vec<PointOfSale>> {
        let rows = sqlx::query_as!(
            PosRow,
            r#"SELECT id, country, currency, is_home AS "is_home: bool",
                      active AS "active: bool"
               FROM points_of_sale WHERE (?1 = 0 OR active = 1)
               ORDER BY is_home DESC, id"#,
            active_only
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(PointOfSale::from).collect())
    }

    // -----------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------

    /// The settings; the defaults until some are saved.
    pub async fn settings(&self) -> Result<Settings> {
        let mut conn = self.pool.acquire().await?;
        settings_in(&mut conn).await
    }

    /// Validates and saves the settings.
    pub async fn set_settings(&self, settings: &Settings) -> Result<()> {
        let mut tx = self.write().await?;
        set_settings_in(&mut tx, settings).await?;
        tx.commit().await?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Airline sources
    // -----------------------------------------------------------------

    /// Adds or updates an airline source (by IATA code).
    pub async fn upsert_airline_source(
        &self,
        code: &str,
        name: &str,
        active: bool,
    ) -> Result<()> {
        let mut tx = self.write().await?;
        upsert_airline_in(&mut tx, code, name, active).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_airline_active(
        &self,
        code: &str,
        active: bool,
    ) -> Result<()> {
        let code = code.trim().to_ascii_uppercase();
        let done = sqlx::query!(
            "UPDATE airline_sources SET active = ?2 WHERE code = ?1",
            code,
            active
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("airline", code));
        }
        Ok(())
    }

    /// Replaces the destinations an airline's site is searched for.
    pub async fn set_airline_destinations(
        &self,
        code: &str,
        places: &[PlaceId],
    ) -> Result<()> {
        let mut tx = self.write().await?;
        set_airline_destinations_in(&mut tx, code, places).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Airline sources by code, with their destinations.
    pub async fn list_airline_sources(&self) -> Result<Vec<AirlineSource>> {
        let sources = sqlx::query!(
            r#"SELECT code, name, active AS "active: bool"
               FROM airline_sources ORDER BY code"#
        )
        .fetch_all(&self.pool)
        .await?;
        let served = sqlx::query!(
            r#"SELECT airline_code AS "airline_code!", place_id AS "place_id!"
             FROM airline_destinations
             ORDER BY airline_code, place_id"#
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(sources
            .into_iter()
            .map(|s| AirlineSource {
                destinations: served
                    .iter()
                    .filter(|d| d.airline_code == s.code)
                    .map(|d| PlaceId(d.place_id))
                    .collect(),
                code: s.code,
                name: s.name,
                active: s.active,
            })
            .collect())
    }

    // -----------------------------------------------------------------
    // Miles programs
    // -----------------------------------------------------------------

    pub async fn upsert_miles_program(
        &self,
        program: &str,
        name: &str,
        active: bool,
    ) -> Result<()> {
        let mut tx = self.write().await?;
        upsert_miles_in(&mut tx, program, name, active).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Records a program's login state and when it was checked (0024).
    pub async fn set_miles_login(
        &self,
        program: &str,
        state: LoginState,
        checked_at: DateTime<Utc>,
    ) -> Result<()> {
        let state = login_state_name(state);
        let done = sqlx::query!(
            "UPDATE miles_programs SET login_state = ?2, last_checked = ?3
             WHERE program = ?1",
            program,
            state,
            checked_at
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("miles program", program));
        }
        Ok(())
    }

    pub async fn list_miles_programs(&self) -> Result<Vec<MilesProgram>> {
        let rows = sqlx::query!(
            r#"SELECT program, name, login_state,
                      last_checked AS "last_checked: DateTime<Utc>",
                      active AS "active: bool", value_minor, buy_minor, dates,
                      session_until AS "session_until: DateTime<Utc>"
               FROM miles_programs ORDER BY program"#
        )
        .fetch_all(&self.pool)
        .await?;
        let brl = |minor: Option<i64>| {
            minor.map(|m| tm_domain::Money::new(m, tm_domain::Currency::brl()))
        };
        rows.into_iter()
            .map(|r| {
                Ok(MilesProgram {
                    login_state: parse_login_state(&r.login_state)?,
                    program: r.program,
                    name: r.name,
                    last_checked: r.last_checked,
                    active: r.active,
                    value_per_1000: brl(r.value_minor),
                    buy_per_1000: brl(r.buy_minor),
                    dates: parse_award_dates(&r.dates)?,
                    session_until: r.session_until,
                })
            })
            .collect()
    }

    /// Records when the program's login session ends, as read from its
    /// profile's cookies; `None` when unknown (0083).
    pub async fn set_miles_session_until(
        &self,
        program: &str,
        until: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE miles_programs SET session_until = ?2 WHERE program = ?1",
            program,
            until
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("miles program", program));
        }
        Ok(())
    }

    /// Saves what the user set for a miles program (0083): whether it is
    /// active, the value of 1,000 miles, the price the program sells
    /// 1,000 at (BRL, positive; `None` unsets) and which dates it is
    /// asked for. Only a change of `active` bumps the configuration
    /// version, since only it changes the families.
    pub async fn save_miles_settings(
        &self,
        program: &str,
        settings: &tm_domain::ProgramSettings,
    ) -> Result<()> {
        let per_1000 = |m: &Option<tm_domain::Money>, what: &str| match m {
            Some(m) if m.currency.as_str() != "BRL" || m.minor <= 0 => {
                Err(StoreError::Invalid(format!(
                    "{what} must be a positive amount in BRL"
                )))
            }
            Some(m) => Ok(Some(m.minor)),
            None => Ok(None),
        };
        let value = per_1000(&settings.value_per_1000, "the value of a mile")?;
        let buy = per_1000(&settings.buy_per_1000, "the price of miles")?;
        let dates = award_dates_name(settings.dates);
        let mut tx = self.write().await?;
        let done = sqlx::query!(
            "UPDATE miles_programs SET value_minor = ?2, buy_minor = ?3,
                 dates = ?4
             WHERE program = ?1",
            program,
            value,
            buy,
            dates
        )
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("miles program", program));
        }
        sqlx::query!(
            "UPDATE miles_programs SET active = ?2
             WHERE program = ?1 AND active != ?2",
            program,
            settings.active
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Routes
    // -----------------------------------------------------------------

    /// Derives routes from the configured origins and destinations (see
    /// [`sync_routes_in`]). Returns the number of active routes.
    pub async fn sync_routes(&self) -> Result<usize> {
        let mut tx = self.write().await?;
        let n = sync_routes_in(&mut tx).await?;
        tx.commit().await?;
        Ok(n)
    }

    /// Real routes (not Explore sweeps), by origin and destination code.
    pub async fn list_routes(&self, active_only: bool) -> Result<Vec<Route>> {
        let rows = sqlx::query_as!(
            RouteRow,
            r#"SELECT r.id, r.active AS "active: bool",
                      COALESCE(d.weight, 1.0) AS "weight!: f64",
                      COALESCE(d.favorite, 0) AS "favorite!: bool",
                      o.id AS o_id, o.code AS o_code, o.name AS o_name,
                      o.country AS o_country, o.kind AS o_kind,
                      o.freebase_id AS o_freebase_id, o.lat AS o_lat,
                      o.lon AS o_lon,
                      p.id AS d_id, p.code AS d_code, p.name AS d_name,
                      p.country AS d_country, p.kind AS d_kind,
                      p.freebase_id AS d_freebase_id, p.lat AS d_lat,
                      p.lon AS d_lon
               FROM routes r
               JOIN places o ON o.id = r.origin_id
               JOIN places p ON p.id = r.destination_id
               LEFT JOIN destinations d ON d.place_id = r.destination_id
               WHERE r.sweep = 0 AND (?1 = 0 OR r.active = 1)
               ORDER BY o.code, p.code"#,
            active_only
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Route::try_from).collect()
    }

    /// The Explore sweep pseudo-routes (destination = origin, weight 1).
    pub async fn sweep_routes(&self, active_only: bool) -> Result<Vec<Route>> {
        let rows = sqlx::query_as!(
            RouteRow,
            r#"SELECT r.id, r.active AS "active: bool",
                      1.0 AS "weight!: f64",
                      0 AS "favorite!: bool",
                      o.id AS o_id, o.code AS o_code, o.name AS o_name,
                      o.country AS o_country, o.kind AS o_kind,
                      o.freebase_id AS o_freebase_id, o.lat AS o_lat,
                      o.lon AS o_lon,
                      o.id AS d_id, o.code AS d_code, o.name AS d_name,
                      o.country AS d_country, o.kind AS d_kind,
                      o.freebase_id AS d_freebase_id, o.lat AS d_lat,
                      o.lon AS d_lon
               FROM routes r JOIN places o ON o.id = r.origin_id
               WHERE r.sweep = 1 AND (?1 = 0 OR r.active = 1)
               ORDER BY o.code"#,
            active_only
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Route::try_from).collect()
    }

    /// A route or a sweep pseudo-route.
    pub async fn route(&self, id: RouteId) -> Result<Option<Route>> {
        let row = sqlx::query_as!(
            RouteRow,
            r#"SELECT r.id, r.active AS "active: bool",
                      CASE WHEN r.sweep = 1 THEN 1.0
                           ELSE COALESCE(d.weight, 1.0) END AS "weight!: f64",
                      CASE WHEN r.sweep = 1 THEN 0
                           ELSE COALESCE(d.favorite, 0) END AS "favorite!: bool",
                      o.id AS o_id, o.code AS o_code, o.name AS o_name,
                      o.country AS o_country, o.kind AS o_kind,
                      o.freebase_id AS o_freebase_id, o.lat AS o_lat,
                      o.lon AS o_lon,
                      p.id AS d_id, p.code AS d_code, p.name AS d_name,
                      p.country AS d_country, p.kind AS d_kind,
                      p.freebase_id AS d_freebase_id, p.lat AS d_lat,
                      p.lon AS d_lon
               FROM routes r
               JOIN places o ON o.id = r.origin_id
               JOIN places p ON p.id = r.destination_id
               LEFT JOIN destinations d ON d.place_id = r.destination_id
               WHERE r.id = ?1"#,
            id.0
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Route::try_from).transpose()
    }

    /// The route between two codes, if it exists (active or not).
    pub async fn route_between(
        &self,
        origin: &str,
        destination: &str,
    ) -> Result<Option<RouteId>> {
        let origin = origin.trim().to_ascii_uppercase();
        let destination = destination.trim().to_ascii_uppercase();
        let id = sqlx::query_scalar!(
            "SELECT r.id FROM routes r
             JOIN places o ON o.id = r.origin_id
             JOIN places p ON p.id = r.destination_id
             WHERE o.code = ?1 AND p.code = ?2",
            origin,
            destination
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(id.map(RouteId))
    }

    /// The Explore pseudo-route of an origin.
    pub async fn sweep_route(
        &self,
        origin: PlaceId,
    ) -> Result<Option<RouteId>> {
        let id = sqlx::query_scalar!(
            "SELECT id FROM routes WHERE sweep = 1 AND origin_id = ?1",
            origin.0
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(id.map(RouteId))
    }

    pub async fn route_trip_state(
        &self,
        route: RouteId,
    ) -> Result<Option<RouteTripState>> {
        let row = sqlx::query!(
            r#"SELECT rt_discount_ewma, rt_discount_n, additive_streak,
                      additive AS "additive: bool"
               FROM routes WHERE id = ?1"#,
            route.0
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(RouteTripState {
                rt_discount_ewma: r.rt_discount_ewma,
                rt_discount_n: count("rt_discount_n", r.rt_discount_n)?,
                additive_streak: count("additive_streak", r.additive_streak)?,
                additive: r.additive,
            })
        })
        .transpose()
    }

    /// Saves a route's round-trip discount state (0042). Flipping
    /// `additive` takes effect on families at the next `sync_families`.
    pub async fn save_route_trip_state(
        &self,
        route: RouteId,
        state: &RouteTripState,
    ) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE routes SET rt_discount_ewma = ?2, rt_discount_n = ?3,
                 additive_streak = ?4, additive = ?5
             WHERE id = ?1",
            route.0,
            state.rt_discount_ewma,
            state.rt_discount_n,
            state.additive_streak,
            state.additive
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("route", route));
        }
        Ok(())
    }
}
