//! "Found by Explore" (DECISIONS.md 0087): destinations the Explore
//! sweep priced that nobody watches, as the routes overview lists them,
//! and the place a click on "Watch" adds.

use chrono::{DateTime, NaiveDate, Utc};
use tm_domain::{Cabin, Money, PlaceId, PlaceKind, country_code, metro_code};
use tm_scheduler::finds::ExploreFind;

use super::NewPlace;

/// One find in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundRow {
    /// Explore's destination.
    pub freebase_id: String,
    pub cabin: Cabin,
    /// The airport Explore names most often for it.
    pub code: String,
    /// Explore's name, or the code when it gave none.
    pub name: String,
    /// The country in Explore's words.
    pub country: Option<String>,
    /// The cheapest price of the last day, and its trip.
    pub price: Money,
    pub departure: Option<NaiveDate>,
    pub return_date: Option<NaiveDate>,
    pub carrier: Option<String>,
    pub stops: Option<u8>,
    pub seen_at: DateTime<Utc>,
    /// Sweeps that priced it.
    pub sweeps: usize,
    /// Its own Explore median, once it has enough history, and the
    /// price against it.
    pub usual: Option<Money>,
    pub change: Option<f64>,
    /// What "Watch" adds; `None` when Explore did not say enough (the
    /// country or the coordinates).
    pub watch: Option<NewPlace>,
}

impl FoundRow {
    /// `Garuda Indonesia, 1 stop`.
    pub fn airline_label(&self) -> Option<String> {
        let carrier = self.carrier.as_deref()?;
        Some(match self.stops {
            Some(0) => format!("{carrier} nonstop"),
            Some(n) => format!("{carrier}, {}", crate::format::stops(n)),
            None => carrier.to_owned(),
        })
    }

    /// A drop under its own Explore history (the daemon's rule).
    pub fn is_drop(&self, threshold: f64) -> bool {
        self.change.is_some_and(|c| c <= -threshold + 1e-9)
    }
}

/// The place "Watch" adds for a find: a city, by its Freebase ID, when
/// Explore priced it at more than one airport or its airport belongs to
/// a metropolitan area Google groups (SFB → ORL); otherwise the airport.
/// `None` without a known country or coordinates.
pub fn watch_place(find: &ExploreFind) -> Option<NewPlace> {
    let country = country_code(find.country.as_deref()?)?;
    let (lat, lon) = find.coordinates?;
    let airport = find.main_airport().to_ascii_uppercase();
    if airport.len() != 3 {
        return None;
    }
    let metro = find.airports.iter().find_map(|a| metro_code(a));
    let city = find.airports.len() > 1 || metro.is_some();
    let name = find.name.clone().unwrap_or_else(|| airport.clone());
    Some(if city && !find.freebase_id.is_empty() {
        NewPlace {
            code: metro.map_or(airport, str::to_owned),
            name,
            country,
            kind: PlaceKind::City {
                freebase_id: Some(find.freebase_id.clone()),
            },
            lat,
            lon,
        }
    } else {
        NewPlace {
            code: airport,
            name,
            country,
            kind: PlaceKind::Airport,
            lat,
            lon,
        }
    })
}

/// The rows for `origin`, in the finds' order (drops first, biggest
/// first, then by price).
pub fn found_rows(
    finds: &[ExploreFind],
    origin: Option<PlaceId>,
) -> Vec<FoundRow> {
    finds
        .iter()
        .filter(|f| origin.is_none_or(|o| o == f.origin))
        .map(|f| FoundRow {
            freebase_id: f.freebase_id.clone(),
            cabin: f.cabin,
            code: f.main_airport().to_owned(),
            name: f
                .name
                .clone()
                .unwrap_or_else(|| f.main_airport().to_owned()),
            country: f.country.clone(),
            price: f.cheapest.price.clone(),
            departure: f.cheapest.departure,
            return_date: f.cheapest.return_date,
            carrier: f.cheapest.carrier.clone(),
            stops: f.cheapest.stops,
            seen_at: f.cheapest.observed_at,
            sweeps: f.sweeps,
            usual: f.usual.clone(),
            change: f.change(),
            watch: watch_place(f),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};
    use tm_domain::{ExploreHint, PosId, QueryId};
    use tm_scheduler::finds::{FindParams, explore_finds};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0)
            .single()
            .expect("valid time")
    }

    fn hint(query: i64, airport: &str, country: Option<&str>) -> ExploreHint {
        ExploreHint {
            query: QueryId(query),
            origin: PlaceId(1),
            cabin: Cabin::Economy,
            pos: PosId(1),
            observed_at: now() - TimeDelta::hours(query),
            destination_freebase_id: "/m/x".into(),
            destination_airport: airport.into(),
            price: Money::brl_whole(5_000),
            departure: None,
            return_date: None,
            stops: Some(0),
            duration_minutes: None,
            carrier: Some("Garuda".into()),
            destination_name: Some("Somewhere".into()),
            destination_country: country.map(str::to_owned),
            destination_coordinates: Some((-6.2, 106.8)),
        }
    }

    fn find(hints: &[ExploreHint]) -> ExploreFind {
        explore_finds(hints, now(), &FindParams::default())
            .into_iter()
            .next()
            .expect("a find")
    }

    #[test]
    fn one_airport_is_watched_as_an_airport() {
        let place = watch_place(&find(&[hint(1, "PER", Some("Australia"))]))
            .expect("a place");
        assert_eq!(place.code, "PER");
        assert_eq!(place.kind, PlaceKind::Airport);
        assert_eq!(place.country.as_str(), "AU");
        assert_eq!(place.name, "Somewhere");
        assert_eq!((place.lat, place.lon), (-6.2, 106.8));
    }

    #[test]
    fn several_airports_or_a_known_metro_make_a_city() {
        let two = [
            hint(1, "AAA", Some("Japan")),
            hint(2, "BBB", Some("Japan")),
            hint(3, "AAA", Some("Japan")),
        ];
        let place = watch_place(&find(&two)).expect("a place");
        assert_eq!(place.code, "AAA");
        assert_eq!(
            place.kind,
            PlaceKind::City {
                freebase_id: Some("/m/x".into())
            }
        );
        let metro = watch_place(&find(&[hint(1, "KIX", Some("Japan"))]))
            .expect("a place");
        assert_eq!(metro.code, "OSA");
        assert!(matches!(metro.kind, PlaceKind::City { .. }));
    }

    #[test]
    fn an_unknown_country_cannot_be_watched_in_one_click() {
        assert_eq!(watch_place(&find(&[hint(1, "CGK", None)])), None);
        assert_eq!(watch_place(&find(&[hint(1, "CGK", Some("Narnia"))])), None);
    }

    #[test]
    fn rows_follow_the_origin() {
        let finds = [find(&[hint(1, "CGK", Some("Indonesia"))])];
        let rows = found_rows(&finds, Some(PlaceId(1)));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code, "CGK");
        assert_eq!(rows[0].airline_label().as_deref(), Some("Garuda nonstop"));
        assert!(found_rows(&finds, Some(PlaceId(2))).is_empty());
    }
}
