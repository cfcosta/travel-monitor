//! What one check of a family loads (DECISIONS.md 0041, 0042, 0051,
//! 0085): pure functions from a family, its route, its point of sale and
//! the settings to a job for a [`crate::sources::Sources`].

use chrono::{DateTime, Days, NaiveDate, Utc};
use tm_domain::{
    Cabin,
    FamilyKey,
    FamilyKind,
    Leg,
    Place,
    PointOfSale,
    Route,
    Settings,
    Source,
};
use tm_gflights::{
    Locale,
    PlaceRef,
    Search,
    plan::graph_pages,
    url::{explore_url, resolve_url, search_url},
};

use crate::{
    sites::{self, latam, latam::dates::Strip, tap},
    sources::{CalendarJob, ExploreJob, LatamJob, ResolveJob, TapJob},
};

/// What a family's check loads.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Calendar(CalendarJob),
    Explore(ExploreJob),
    Tap(TapJob),
    Latam(LatamJob),
}

/// The dates one LATAM check reads (0085, 0090), chosen by the worker
/// from the database ([`crate::sites::latam::dates`]): one results page,
/// and the strip days it may click when the strip is switched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatamDates {
    pub departure: NaiveDate,
    /// A round trip's return; a leg's is ignored.
    pub return_date: Option<NaiveDate>,
    /// The strip days the check may click (one-way searches only;
    /// [`Strip::none`] with the strip off).
    pub strip: Strip,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// A city whose Freebase ID is not known yet.
    #[error("{0} has no Freebase ID yet")]
    Unresolved(String),
    /// The cabin has no Explore phrasing (premium economy).
    #[error("Explore cannot search {0:?}")]
    NoExploreCabin(Cabin),
    /// An airline or miles site the daemon has no reader for (0052,
    /// 0059).
    #[error("no reader for {0}")]
    NoReader(String),
    /// Explore on a real route, or a leg on a sweep.
    #[error("the family's kind does not fit its source")]
    Mismatch,
    /// A scripted site that cannot run this family (a TAP round trip,
    /// a party TAP's calendar does not price).
    #[error("{0}")]
    Unsupported(&'static str),
}

/// Knobs that are not settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobOptions {
    /// Cap on price-graph pages (debugging).
    pub max_graph_pages: Option<u32>,
    /// Replace `https://www.google.com` in URLs (tests).
    pub origin_override: Option<String>,
    /// The date a TAP search without a calendar prices (the daemon
    /// picks the cheapest known Google Flights date, 0028 fan-out, 0053).
    pub cheapest_departure: Option<NaiveDate>,
    /// The check's time, for the freshness of cached airline prices;
    /// `None` is midnight UTC of `today`.
    pub now: Option<DateTime<Utc>>,
    /// Replace TAP's `https://booking.flytap.com` (tests).
    pub tap_base: Option<String>,
    /// Replace LATAM's `https://www.latamairlines.com` (tests).
    pub latam_base: Option<String>,
    /// What a LATAM check reads; without it, the date a TAP search
    /// without a calendar would price, and no strip.
    pub latam: Option<LatamDates>,
}

/// The `hl`/`gl`/`curr` of a point of sale.
pub fn locale(settings: &Settings, pos: &PointOfSale) -> Locale {
    Locale::new(
        &settings.language,
        pos.country.clone(),
        pos.currency.clone(),
    )
}

/// Points generated URLs somewhere else (tests serve a fake Google).
pub fn rebase(url: &str, origin: Option<&str>) -> String {
    match origin {
        Some(origin) => url.replacen(
            "https://www.google.com",
            origin.trim_end_matches('/'),
            1,
        ),
        None => url.to_owned(),
    }
}

fn place_ref(place: &Place) -> Result<PlaceRef, JobError> {
    PlaceRef::from_place(place)
        .ok_or_else(|| JobError::Unresolved(place.code.clone()))
}

/// The first departure a check searches: tomorrow. The price graph's
/// first page starts a few days before it and runs about 60 days on.
pub fn first_departure(today: NaiveDate) -> NaiveDate {
    today + Days::new(1)
}

/// The last departure a check needs (0009: six months).
pub fn horizon_end(today: NaiveDate, settings: &Settings) -> NaiveDate {
    today + Days::new(u64::from(settings.horizon_days))
}

/// Builds the job for one check of `key`.
pub fn build(
    key: &FamilyKey,
    route: &Route,
    pos: &PointOfSale,
    settings: &Settings,
    today: NaiveDate,
    options: &JobOptions,
) -> Result<Job, JobError> {
    let locale = locale(settings, pos);
    let origin_override = options.origin_override.as_deref();
    match (&key.source, key.kind) {
        (Source::GoogleFlights, FamilyKind::Explore) => {
            let url = explore_url(&route.origin.code, key.cabin, &locale)
                .map_err(|_| JobError::NoExploreCabin(key.cabin))?;
            Ok(Job::Explore(ExploreJob {
                pos: pos.id,
                url: rebase(&url, origin_override),
            }))
        }
        (Source::GoogleFlights, kind) => {
            let origin = place_ref(&route.origin)?;
            let destination = place_ref(&route.destination)?;
            let date = first_departure(today);
            let passengers = settings.passengers;
            let search = match kind {
                FamilyKind::Leg { leg: Leg::Outbound } => Search::one_way(
                    origin,
                    destination,
                    date,
                    key.cabin,
                    passengers,
                ),
                FamilyKind::Leg { leg: Leg::Return } => Search::one_way(
                    destination,
                    origin,
                    date,
                    key.cabin,
                    passengers,
                ),
                FamilyKind::RoundTrip { days } => Search::round_trip(
                    origin,
                    destination,
                    date,
                    date + Days::new(u64::from(days)),
                    key.cabin,
                    passengers,
                ),
                FamilyKind::Explore => return Err(JobError::Mismatch),
            };
            let pages = graph_pages(u32::from(settings.horizon_days)).max(1);
            let max_pages = options
                .max_graph_pages
                .map_or(pages, |cap| cap.clamp(1, pages));
            Ok(Job::Calendar(CalendarJob {
                pos: pos.id,
                url: rebase(&search_url(&search, &locale), origin_override),
                search,
                horizon_end: horizon_end(today, settings),
                max_pages,
                display: pos.currency.clone(),
            }))
        }
        (_, FamilyKind::Explore) => Err(JobError::Mismatch),
        (Source::Airline { code }, kind) if code == tap::CODE => {
            tap_job(key, kind, route, pos, settings, today, options)
                .map(Job::Tap)
        }
        (Source::Airline { code }, kind) if code == latam::CODE => {
            latam_job(key, kind, route, pos, settings, today, options)
                .map(Job::Latam)
        }
        (Source::Miles { program }, kind) if program == latam::PROGRAM => {
            latam_job(key, kind, route, pos, settings, today, options)
                .map(Job::Latam)
        }
        (source, _) => Err(JobError::NoReader(source.site())),
    }
}

/// A TAP leg (0051): the BR deep link for one direction under the
/// family's (home) point of sale.
fn tap_job(
    key: &FamilyKey,
    kind: FamilyKind,
    route: &Route,
    pos: &PointOfSale,
    settings: &Settings,
    today: NaiveDate,
    options: &JobOptions,
) -> Result<TapJob, JobError> {
    if let Some(why) = sites::unsupported(key, &settings.passengers) {
        return Err(JobError::Unsupported(why));
    }
    let (from, to) = match kind {
        FamilyKind::Leg { leg: Leg::Outbound } => {
            (&route.origin, &route.destination)
        }
        FamilyKind::Leg { leg: Leg::Return } => {
            (&route.destination, &route.origin)
        }
        FamilyKind::RoundTrip { .. } | FamilyKind::Explore => {
            return Err(JobError::Mismatch);
        }
    };
    let first = first_departure(today);
    // Economy reads the calendar from the first departure on; the
    // other cabins price one date, the cheapest Google showed.
    let departure = if key.cabin == Cabin::Economy {
        first
    } else {
        options
            .cheapest_departure
            .filter(|d| *d > today)
            .unwrap_or(today + Days::new(45))
    };
    let origin = tap::airport(&from.code).to_owned();
    let destination = tap::airport(&to.code).to_owned();
    let link = tap::Deeplink {
        market: pos.country.as_str().to_owned(),
        language: tap::language(&settings.language),
        origin: origin.clone(),
        destination: destination.clone(),
        departure,
        return_date: None,
        passengers: settings.passengers,
    };
    let base = options.tap_base.as_deref().unwrap_or(tap::BASE);
    let url = link.url(base).ok_or(JobError::Unsupported(
        "TAP's deep link cannot say this party",
    ))?;
    let end = horizon_end(today, settings);
    let now = options.now.unwrap_or_else(|| {
        today.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc()
    });
    Ok(TapJob {
        pos: pos.id,
        url,
        origin,
        destination,
        cabin: key.cabin,
        departure,
        first,
        horizon_end: end,
        max_months: tap::months_needed(first, end),
        fresh_since: tap::fresh_since(now),
    })
}

/// A LATAM search (0085): the Brazilian results page, cash or award, for
/// one leg or the round trip, under the family's (home) point of sale.
/// The page is always searched in economy, which returns every cabin
/// (latam.md §2); the job keeps the family's.
fn latam_job(
    key: &FamilyKey,
    kind: FamilyKind,
    route: &Route,
    pos: &PointOfSale,
    settings: &Settings,
    today: NaiveDate,
    options: &JobOptions,
) -> Result<LatamJob, JobError> {
    if let Some(why) = sites::unsupported(key, &settings.passengers) {
        return Err(JobError::Unsupported(why));
    }
    let (from, to, days) = match kind {
        FamilyKind::Leg { leg: Leg::Outbound } => {
            (&route.origin, &route.destination, None)
        }
        FamilyKind::Leg { leg: Leg::Return } => {
            (&route.destination, &route.origin, None)
        }
        FamilyKind::RoundTrip { days } => {
            (&route.origin, &route.destination, Some(days))
        }
        FamilyKind::Explore => return Err(JobError::Mismatch),
    };
    let first = first_departure(today);
    let dates = options.latam.clone().unwrap_or_else(|| LatamDates {
        departure: options
            .cheapest_departure
            .filter(|d| *d > today)
            .unwrap_or(today + Days::new(45)),
        return_date: None,
        strip: Strip::none(),
    });
    let departure = dates.departure.max(first);
    let return_date = days.map(|days| {
        dates
            .return_date
            .filter(|r| *r > departure)
            .unwrap_or(departure + Days::new(u64::from(days)))
    });
    let origin = latam::airport(&from.code).to_owned();
    let destination = latam::airport(&to.code).to_owned();
    let award = matches!(key.source, Source::Miles { .. });
    let search = latam::Search {
        origin: origin.clone(),
        destination: destination.clone(),
        outbound: departure,
        inbound: return_date,
        passengers: settings.passengers,
        cabin: Cabin::Economy,
        redemption: award,
    };
    let base = options.latam_base.as_deref().unwrap_or(latam::BASE);
    let url = search.url(base).ok_or(JobError::Unsupported(
        "LATAM's results URL cannot say this party",
    ))?;
    Ok(LatamJob {
        pos: pos.id,
        url,
        base: base.trim_end_matches('/').to_owned(),
        origin,
        destination,
        cabin: key.cabin,
        departure,
        return_date,
        award,
        seller: key.source.site(),
        strip: if return_date.is_some() {
            Strip::none()
        } else {
            dates.strip
        },
    })
}

/// The `q=` search that resolves a city (0041).
pub fn resolve_job(
    place: &Place,
    settings: &Settings,
    pos: &PointOfSale,
    options: &JobOptions,
) -> ResolveJob {
    let url = resolve_url(&place.name, &locale(settings, pos));
    ResolveJob {
        pos: pos.id,
        url: rebase(&url, options.origin_override.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use tm_domain::{
        CountryCode,
        Currency,
        PlaceId,
        PlaceKind,
        PosId,
        RouteId,
    };

    use super::*;

    fn place(code: &str, kind: PlaceKind) -> Place {
        Place {
            id: PlaceId(1),
            code: code.into(),
            name: code.into(),
            country: CountryCode::new("BR"),
            kind,
            lat: 0.0,
            lon: 0.0,
        }
    }

    fn route(dest: PlaceKind) -> Route {
        Route {
            id: RouteId(1),
            origin: place(
                "SAO",
                PlaceKind::City {
                    freebase_id: Some("/m/022pfm".into()),
                },
            ),
            destination: place("LIS", dest),
            weight: 1.0,
            active: true,
        }
    }

    fn pos() -> PointOfSale {
        PointOfSale {
            id: PosId(2),
            country: CountryCode::new("US"),
            currency: Currency::brl(),
            is_home: false,
            active: true,
        }
    }

    fn key(source: Source, kind: FamilyKind) -> FamilyKey {
        FamilyKey {
            route: RouteId(1),
            cabin: Cabin::Business,
            source,
            pos: PosId(2),
            kind,
        }
    }

    fn day(m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, d).unwrap_or_default()
    }

    #[test]
    fn legs_and_round_trips_are_price_graphs() {
        let settings = Settings::default();
        let today = day(10, 6);
        let job = build(
            &key(Source::GoogleFlights, FamilyKind::Leg { leg: Leg::Return }),
            &route(PlaceKind::Airport),
            &pos(),
            &settings,
            today,
            &JobOptions::default(),
        );
        let Ok(Job::Calendar(job)) = job else {
            panic!("a calendar job: {job:?}");
        };
        assert_eq!(job.search.legs.len(), 1);
        assert_eq!(job.search.legs[0].from, PlaceRef::Airport("LIS".into()));
        assert_eq!(job.search.legs[0].to, PlaceRef::City("/m/022pfm".into()));
        assert_eq!(job.search.legs[0].date, day(10, 7));
        assert_eq!(job.search.cabin, Cabin::Business);
        assert_eq!(job.max_pages, 7);
        assert_eq!(
            job.horizon_end,
            NaiveDate::from_ymd_opt(2027, 4, 4).unwrap_or_default()
        );
        assert!(
            job.url.starts_with(
                "https://www.google.com/travel/flights/search?tfs="
            )
        );
        assert!(job.url.ends_with("&hl=en&gl=US&curr=BRL"));

        let job = build(
            &key(Source::GoogleFlights, FamilyKind::RoundTrip { days: 7 }),
            &route(PlaceKind::Airport),
            &pos(),
            &settings,
            today,
            &JobOptions {
                max_graph_pages: Some(2),
                origin_override: Some("http://127.0.0.1:9/".into()),
                ..JobOptions::default()
            },
        );
        let Ok(Job::Calendar(job)) = job else {
            panic!("a calendar job: {job:?}");
        };
        assert_eq!(job.search.legs.len(), 2);
        assert_eq!(job.search.legs[1].date, day(10, 14));
        assert_eq!(job.max_pages, 2);
        assert!(
            job.url
                .starts_with("http://127.0.0.1:9/travel/flights/search?tfs=")
        );
    }

    #[test]
    fn unresolved_cities_and_explore_cabins() {
        let settings = Settings::default();
        let r = route(PlaceKind::City { freebase_id: None });
        let job = build(
            &key(
                Source::GoogleFlights,
                FamilyKind::Leg { leg: Leg::Outbound },
            ),
            &r,
            &pos(),
            &settings,
            day(10, 6),
            &JobOptions::default(),
        );
        assert_eq!(job, Err(JobError::Unresolved("LIS".into())));
        let mut k = key(Source::GoogleFlights, FamilyKind::Explore);
        let job = build(
            &k,
            &r,
            &pos(),
            &settings,
            day(10, 6),
            &JobOptions::default(),
        );
        let Ok(Job::Explore(job)) = job else {
            panic!("an Explore job: {job:?}");
        };
        assert!(
            job.url
                .contains("q=Flights%20from%20SAO%20business%20class")
        );
        k.cabin = Cabin::PremiumEconomy;
        assert_eq!(
            build(
                &k,
                &r,
                &pos(),
                &settings,
                day(10, 6),
                &JobOptions::default()
            ),
            Err(JobError::NoExploreCabin(Cabin::PremiumEconomy))
        );
    }

    /// Airline and miles sites other than TAP and LATAM have no reader
    /// (0059).
    #[test]
    fn other_airline_and_miles_sites_have_no_reader() {
        let settings = Settings::default();
        for (source, site) in [
            (Source::Airline { code: "G3".into() }, "airline:G3"),
            (
                Source::Miles {
                    program: "smiles".into(),
                },
                "miles:smiles",
            ),
        ] {
            assert_eq!(
                build(
                    &key(source, FamilyKind::Leg { leg: Leg::Outbound }),
                    &route(PlaceKind::Airport),
                    &pos(),
                    &settings,
                    day(10, 6),
                    &JobOptions::default(),
                ),
                Err(JobError::NoReader(site.into()))
            );
        }
    }

    fn home_pos() -> PointOfSale {
        PointOfSale {
            id: PosId(1),
            country: CountryCode::new("BR"),
            currency: Currency::brl(),
            is_home: true,
            active: true,
        }
    }

    #[test]
    fn tap_legs_are_deep_links() {
        let settings = Settings::default();
        let tap = |cabin, kind| {
            let mut k = key(Source::Airline { code: "TP".into() }, kind);
            k.cabin = cabin;
            k.pos = PosId(1);
            k
        };
        let now = NaiveDate::from_ymd_opt(2026, 10, 6)
            .and_then(|d| d.and_hms_opt(12, 0, 0))
            .unwrap_or_default()
            .and_utc();
        let options = JobOptions {
            cheapest_departure: Some(day(11, 10)),
            now: Some(now),
            ..JobOptions::default()
        };
        let job = build(
            &tap(Cabin::Economy, FamilyKind::Leg { leg: Leg::Return }),
            &route(PlaceKind::Airport),
            &home_pos(),
            &settings,
            day(10, 6),
            &options,
        );
        let Ok(Job::Tap(job)) = job else {
            panic!("a TAP job: {job:?}");
        };
        assert_eq!(job.pos, PosId(1));
        assert_eq!(
            (job.origin.as_str(), job.destination.as_str()),
            ("LIS", "GRU")
        );
        assert_eq!(job.cabin, Cabin::Economy);
        // Economy reads the calendar from tomorrow, to the horizon.
        assert_eq!(job.departure, day(10, 7));
        assert_eq!(job.first, day(10, 7));
        assert_eq!(
            job.horizon_end,
            NaiveDate::from_ymd_opt(2027, 4, 4).unwrap_or_default()
        );
        assert_eq!(job.max_months, 7);
        assert_eq!(job.fresh_since, now - chrono::TimeDelta::hours(48));
        assert_eq!(
            job.url,
            "https://booking.flytap.com/booking/flights/deeplink?market=BR\
             &language=en&origin=LIS&destination=GRU&flexibleDates=false\
             &flightType=single&adt=1&chd=0&inf=0&yth=0&depDate=07.10.2026\
             &x_tap_source=WEB"
        );

        // Business prices the cheapest date Google showed.
        let job = build(
            &tap(Cabin::Business, FamilyKind::Leg { leg: Leg::Outbound }),
            &route(PlaceKind::Airport),
            &home_pos(),
            &settings,
            day(10, 6),
            &JobOptions {
                tap_base: Some("http://127.0.0.1:9".into()),
                ..options.clone()
            },
        );
        let Ok(Job::Tap(job)) = job else {
            panic!("a TAP job: {job:?}");
        };
        assert_eq!(job.departure, day(11, 10));
        assert_eq!(
            (job.origin.as_str(), job.destination.as_str()),
            ("GRU", "LIS")
        );
        assert!(
            job.url
                .starts_with("http://127.0.0.1:9/booking/flights/deeplink?")
        );
        assert!(job.url.contains("&depDate=10.11.2026&"));

        // Round trips and larger parties are not TAP's.
        assert!(matches!(
            build(
                &tap(Cabin::Economy, FamilyKind::Explore),
                &route(PlaceKind::Airport),
                &home_pos(),
                &settings,
                day(10, 6),
                &options,
            ),
            Err(JobError::Mismatch)
        ));
        assert!(matches!(
            build(
                &tap(Cabin::Economy, FamilyKind::RoundTrip { days: 7 }),
                &route(PlaceKind::Airport),
                &home_pos(),
                &settings,
                day(10, 6),
                &options,
            ),
            Err(JobError::Unsupported(_))
        ));
        let mut two = settings.clone();
        two.passengers.adults = 2;
        assert!(matches!(
            build(
                &tap(Cabin::Economy, FamilyKind::Leg { leg: Leg::Outbound }),
                &route(PlaceKind::Airport),
                &home_pos(),
                &two,
                day(10, 6),
                &options,
            ),
            Err(JobError::Unsupported(_))
        ));
    }

    #[test]
    fn latam_checks_are_results_pages() {
        let settings = Settings::default();
        let key = |source, cabin, kind| FamilyKey {
            route: RouteId(1),
            cabin,
            source,
            pos: PosId(1),
            kind,
        };
        let pass = Source::Miles {
            program: "latam_pass".into(),
        };
        let strip = Strip {
            max_clicks: 6,
            ..Strip::none()
        };
        let options = JobOptions {
            latam_base: Some("http://127.0.0.1:9/".into()),
            latam: Some(LatamDates {
                departure: day(11, 10),
                return_date: None,
                strip: strip.clone(),
            }),
            ..JobOptions::default()
        };
        // An award leg: GRU for SAO, redemption, every cabin searched.
        let job = build(
            &key(
                pass.clone(),
                Cabin::Business,
                FamilyKind::Leg { leg: Leg::Return },
            ),
            &route(PlaceKind::Airport),
            &home_pos(),
            &settings,
            day(10, 6),
            &options,
        );
        let Ok(Job::Latam(job)) = job else {
            panic!("a LATAM job: {job:?}");
        };
        assert_eq!(
            (job.origin.as_str(), job.destination.as_str()),
            ("LIS", "GRU")
        );
        assert!(job.award);
        assert_eq!(job.cabin, Cabin::Business);
        assert_eq!(job.seller, "miles:latam_pass");
        assert_eq!(job.base, "http://127.0.0.1:9");
        assert_eq!(job.strip, strip);
        assert!(job.url.starts_with("http://127.0.0.1:9/br/pt/oferta-voos?origin=LIS&destination=GRU&outbound=2026-11-10T"));
        assert!(job.url.contains("&trip=OW&cabin=Economy&redemption=true"));

        // Round trips wait for a verified click (0090), cash or award.
        for source in [Source::Airline { code: "LA".into() }, pass.clone()] {
            let job = build(
                &key(source, Cabin::Economy, FamilyKind::RoundTrip { days: 7 }),
                &route(PlaceKind::Airport),
                &home_pos(),
                &settings,
                day(10, 6),
                &options,
            );
            assert!(
                matches!(&job, Err(JobError::Unsupported(why)) if why.contains("round trip")),
                "{job:?}"
            );
        }

        // Larger parties are not LATAM's.
        let mut two = settings.clone();
        two.passengers.adults = 2;
        assert!(matches!(
            build(
                &key(
                    pass,
                    Cabin::Economy,
                    FamilyKind::Leg { leg: Leg::Outbound }
                ),
                &route(PlaceKind::Airport),
                &home_pos(),
                &two,
                day(10, 6),
                &options,
            ),
            Err(JobError::Unsupported(_))
        ));
    }
}
