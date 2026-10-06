//! Query families and their scheduler state (DECISIONS.md 0028, 0033,
//! 0042).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use tm_domain::{
    Cabin,
    FamilyId,
    FamilyKey,
    FamilyKind,
    Leg,
    PointOfSale,
    PosId,
    Route,
    RouteId,
    Settings,
    Source,
};

use crate::{
    Store,
    convert::{FamilyRow, count, family_columns},
    error::{Result, StoreError},
    records::{
        AirlineSource,
        FamilyRecord,
        FamilyStateRecord,
        FamilySync,
        MilesProgram,
    },
};

/// Everything the set of families is derived from.
pub(crate) struct FamilyInputs<'a> {
    /// Active real routes.
    pub routes: &'a [Route],
    /// Routes whose round trip has been shown additive (0042).
    pub additive: &'a HashSet<RouteId>,
    /// Active sweep pseudo-routes.
    pub sweeps: &'a [Route],
    /// Active points of sale.
    pub points_of_sale: &'a [PointOfSale],
    /// The home point of sale, used by airline and miles sources.
    pub home: Option<PosId>,
    /// Active airline sources.
    pub airlines: &'a [AirlineSource],
    /// Active miles programs.
    pub miles: &'a [MilesProgram],
    pub settings: &'a Settings,
}

/// The families the configuration implies:
///
/// - for every active route × its cabins (`Route::cabins`, 0026):
///   - sources: Google Flights under every active point of sale; each
///     active airline that serves the destination, and each active miles
///     program, under the home point of sale only;
///   - kinds: both legs, plus the round trip of
///     `settings.trips.round_trip_days` unless the route is additive;
/// - for every active origin's sweep pseudo-route: Explore in economy
///   and business under every active point of sale (0025).
pub(crate) fn desired_families(inputs: &FamilyInputs<'_>) -> Vec<FamilyKey> {
    let mut keys = Vec::new();
    let legs = [
        FamilyKind::Leg { leg: Leg::Outbound },
        FamilyKind::Leg { leg: Leg::Return },
    ];
    let round_trip = FamilyKind::RoundTrip {
        days: inputs.settings.trips.round_trip_days,
    };
    for route in inputs.routes {
        let mut kinds = legs.to_vec();
        if !inputs.additive.contains(&route.id) {
            kinds.push(round_trip);
        }
        let mut sources: Vec<(Source, PosId)> = inputs
            .points_of_sale
            .iter()
            .map(|pos| (Source::GoogleFlights, pos.id))
            .collect();
        if let Some(home) = inputs.home {
            for airline in inputs.airlines {
                if airline.destinations.contains(&route.destination.id) {
                    let code = airline.code.clone();
                    sources.push((Source::Airline { code }, home));
                }
            }
            for program in inputs.miles {
                let program = program.program.clone();
                sources.push((Source::Miles { program }, home));
            }
        }
        for &cabin in route.cabins() {
            for (source, pos) in &sources {
                for &kind in &kinds {
                    keys.push(FamilyKey {
                        route: route.id,
                        cabin,
                        source: source.clone(),
                        pos: *pos,
                        kind,
                    });
                }
            }
        }
    }
    for sweep in inputs.sweeps {
        for cabin in [Cabin::Economy, Cabin::Business] {
            for pos in inputs.points_of_sale {
                keys.push(FamilyKey {
                    route: sweep.id,
                    cabin,
                    source: Source::GoogleFlights,
                    pos: pos.id,
                    kind: FamilyKind::Explore,
                });
            }
        }
    }
    keys
}

impl Store {
    /// Syncs routes, then makes exactly the families the configuration
    /// implies active (see [`desired_families`]), creating missing ones
    /// with the default priors. Every other family, such as those of
    /// removed routes, points of sale or sources, or round trips of an
    /// additive route, is kept with its state but goes inactive.
    pub async fn sync_families(
        &self,
        settings: &Settings,
    ) -> Result<FamilySync> {
        self.sync_routes().await?;
        let routes = self.list_routes(true).await?;
        let sweeps = self.sweep_routes(true).await?;
        let points_of_sale = self.list_points_of_sale(true).await?;
        let home = self.home_point_of_sale().await?.map(|pos| pos.id);
        let airlines: Vec<AirlineSource> = self
            .list_airline_sources()
            .await?
            .into_iter()
            .filter(|a| a.active)
            .collect();
        let miles: Vec<MilesProgram> = self
            .list_miles_programs()
            .await?
            .into_iter()
            .filter(|m| m.active)
            .collect();
        let additive: HashSet<RouteId> =
            sqlx::query_scalar!("SELECT id FROM routes WHERE additive = 1")
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(RouteId)
                .collect();
        let keys = desired_families(&FamilyInputs {
            routes: &routes,
            additive: &additive,
            sweeps: &sweeps,
            points_of_sale: &points_of_sale,
            home,
            airlines: &airlines,
            miles: &miles,
            settings,
        });

        let prior = FamilyStateRecord::default();
        let mut tx = self.write().await?;
        let before = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM families"#
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query!("UPDATE families SET active = 0 WHERE active = 1")
            .execute(&mut *tx)
            .await?;
        for key in &keys {
            let c = family_columns(key);
            sqlx::query!(
                "INSERT INTO families (
                     key, route_id, cabin, source, source_code, pos_id, kind,
                     leg, days, site, active, chg_a, chg_b, deal_a, deal_b,
                     prec_a, prec_b, n_hints, checks, cost_loads)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?12,
                         ?13, ?14, ?15, ?16, 0, 0, ?17)
                 ON CONFLICT (key) DO UPDATE SET active = 1",
                c.key,
                key.route.0,
                c.cabin,
                c.source,
                c.source_code,
                key.pos.0,
                c.kind,
                c.leg,
                c.days,
                c.site,
                prior.chg_a,
                prior.chg_b,
                prior.deal_a,
                prior.deal_b,
                prior.prec_a,
                prior.prec_b,
                prior.cost_loads,
            )
            .execute(&mut *tx)
            .await?;
        }
        let after = sqlx::query!(
            r#"SELECT COUNT(*) AS "total!: i64",
                      COALESCE(SUM(active), 0) AS "active!: i64"
               FROM families"#
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(FamilySync {
            active: count("families", after.active)?,
            created: count("families", after.total - before)?,
        })
    }

    /// Families with their state, by id.
    pub async fn list_families(
        &self,
        active_only: bool,
    ) -> Result<Vec<FamilyRecord>> {
        let rows = sqlx::query_as!(
            FamilyRow,
            r#"SELECT f.id, f.route_id, f.cabin, f.source, f.source_code,
                      f.pos_id, f.kind, f.leg, f.days, f.site,
                      f.active AS "active: bool",
                      CASE WHEN r.sweep = 1 THEN 1.0
                           ELSE COALESCE(d.weight, 1.0) END AS "weight!: f64",
                      f.chg_a, f.chg_b, f.deal_a, f.deal_b, f.prec_a, f.prec_b,
                      f.n_hints,
                      f.last_check AS "last_check: DateTime<Utc>",
                      f.last_min_price_minor, f.checks, f.cost_loads,
                      r.rt_discount_ewma, r.rt_discount_n, r.additive_streak,
                      r.additive AS "additive: bool"
               FROM families f
               JOIN routes r ON r.id = f.route_id
               LEFT JOIN destinations d ON d.place_id = r.destination_id
               WHERE (?1 = 0 OR f.active = 1)
               ORDER BY f.id"#,
            active_only
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(FamilyRecord::try_from).collect()
    }

    pub async fn family(&self, id: FamilyId) -> Result<Option<FamilyRecord>> {
        let row = sqlx::query_as!(
            FamilyRow,
            r#"SELECT f.id, f.route_id, f.cabin, f.source, f.source_code,
                      f.pos_id, f.kind, f.leg, f.days, f.site,
                      f.active AS "active: bool",
                      CASE WHEN r.sweep = 1 THEN 1.0
                           ELSE COALESCE(d.weight, 1.0) END AS "weight!: f64",
                      f.chg_a, f.chg_b, f.deal_a, f.deal_b, f.prec_a, f.prec_b,
                      f.n_hints,
                      f.last_check AS "last_check: DateTime<Utc>",
                      f.last_min_price_minor, f.checks, f.cost_loads,
                      r.rt_discount_ewma, r.rt_discount_n, r.additive_streak,
                      r.additive AS "additive: bool"
               FROM families f
               JOIN routes r ON r.id = f.route_id
               LEFT JOIN destinations d ON d.place_id = r.destination_id
               WHERE f.id = ?1"#,
            id.0
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(FamilyRecord::try_from).transpose()
    }

    /// The id of the family with this key, if it exists.
    pub async fn family_id(&self, key: &FamilyKey) -> Result<Option<FamilyId>> {
        let text = family_columns(key).key;
        let id =
            sqlx::query_scalar!("SELECT id FROM families WHERE key = ?1", text)
                .fetch_optional(&self.pool)
                .await?;
        Ok(id.map(FamilyId))
    }

    /// Saves a family's own scheduler state. The route-level round-trip
    /// fields of `state` are ignored: see
    /// [`Store::save_route_trip_state`].
    pub async fn save_family_state(
        &self,
        family: FamilyId,
        state: &FamilyStateRecord,
    ) -> Result<()> {
        let done = sqlx::query!(
            "UPDATE families SET chg_a = ?2, chg_b = ?3, deal_a = ?4,
                 deal_b = ?5, prec_a = ?6, prec_b = ?7, n_hints = ?8,
                 last_check = ?9, last_min_price_minor = ?10, checks = ?11,
                 cost_loads = ?12
             WHERE id = ?1",
            family.0,
            state.chg_a,
            state.chg_b,
            state.deal_a,
            state.deal_b,
            state.prec_a,
            state.prec_b,
            state.n_hints,
            state.last_check,
            state.last_min_price_minor,
            state.checks,
            state.cost_loads,
        )
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::not_found("family", family));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tm_domain::{CountryCode, Currency, Place, PlaceId, PlaceKind};

    use super::*;

    fn place(id: i64, code: &str, country: &str) -> Place {
        Place {
            id: PlaceId(id),
            code: code.into(),
            name: code.into(),
            country: CountryCode::new(country),
            kind: PlaceKind::Airport,
            lat: 0.0,
            lon: 0.0,
        }
    }

    fn route(id: i64, from: &Place, to: &Place) -> Route {
        Route {
            id: RouteId(id),
            origin: from.clone(),
            destination: to.clone(),
            weight: 1.0,
            active: true,
        }
    }

    fn pos(id: i64, country: &str, home: bool) -> PointOfSale {
        PointOfSale {
            id: PosId(id),
            country: CountryCode::new(country),
            currency: Currency::brl(),
            is_home: home,
            active: true,
        }
    }

    #[test]
    fn derives_the_family_grid() {
        let sao = place(1, "SAO", "BR");
        let ssa = place(2, "SSA", "BR");
        let lis = place(3, "LIS", "PT");
        let routes = [route(10, &sao, &ssa), route(11, &sao, &lis)];
        let sweeps = [route(12, &sao, &sao)];
        let poses = [pos(1, "BR", true), pos(2, "US", false)];
        let airlines = [AirlineSource {
            code: "TP".into(),
            name: "TAP".into(),
            active: true,
            destinations: vec![lis.id],
        }];
        let miles = [MilesProgram {
            program: "smiles".into(),
            name: "Smiles".into(),
            login_state: crate::LoginState::LoggedOut,
            last_checked: None,
            active: true,
        }];
        let settings = Settings::default();
        let mut additive = HashSet::new();
        additive.insert(RouteId(10));
        let keys = desired_families(&FamilyInputs {
            routes: &routes,
            additive: &additive,
            sweeps: &sweeps,
            points_of_sale: &poses,
            home: Some(PosId(1)),
            airlines: &airlines,
            miles: &miles,
            settings: &settings,
        });
        // SSA: domestic (1 cabin) × (2 GF + smiles) × 2 legs (additive).
        let ssa_n = keys.iter().filter(|k| k.route == RouteId(10)).count();
        assert_eq!(ssa_n, 3 * 2);
        // LIS: 3 cabins × (2 GF + TAP + smiles) × 3 kinds.
        let lis_n = keys.iter().filter(|k| k.route == RouteId(11)).count();
        assert_eq!(lis_n, 3 * 4 * 3);
        // Explore: 2 cabins × 2 points of sale.
        let explore: Vec<_> = keys
            .iter()
            .filter(|k| k.kind == FamilyKind::Explore)
            .collect();
        assert_eq!(explore.len(), 4);
        assert!(explore.iter().all(|k| k.route == RouteId(12)));
        // Airline and miles families use the home point of sale.
        assert!(
            keys.iter()
                .filter(|k| k.source != Source::GoogleFlights)
                .all(|k| k.pos == PosId(1))
        );
        let unique: HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
    }
}
