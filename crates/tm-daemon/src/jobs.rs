//! What one check of a family loads (DECISIONS.md 0041, 0042): pure
//! functions from a family, its route, its point of sale and the
//! settings to a job for a [`crate::sources::Sources`].

use chrono::{Days, NaiveDate};
use tm_browser::agent::Task;
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

use crate::sources::{AgentJob, CalendarJob, ExploreJob, ResolveJob};

/// What a family's check loads.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    Calendar(CalendarJob),
    Explore(ExploreJob),
    Agent(AgentJob),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// A city whose Freebase ID is not known yet.
    #[error("{0} has no Freebase ID yet")]
    Unresolved(String),
    /// The cabin has no Explore phrasing (premium economy).
    #[error("Explore cannot search {0:?}")]
    NoExploreCabin(Cabin),
    /// No start page is known for this site.
    #[error("no start page for {0}")]
    NoStartPage(String),
    /// Explore on a real route, or a leg on a sweep.
    #[error("the family's kind does not fit its source")]
    Mismatch,
}

/// Knobs that are not settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobOptions {
    /// Cap on price-graph pages (debugging).
    pub max_graph_pages: Option<u32>,
    /// Replace `https://www.google.com` in URLs (tests).
    pub origin_override: Option<String>,
    /// The date an airline or miles search uses (the daemon picks the
    /// cheapest known Google Flights date, 0028 fan-out).
    pub agent_departure: Option<NaiveDate>,
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
        (source, kind) => {
            let site = source.site();
            let (start_url, miles, carrier) = match source {
                Source::Airline { code } => (
                    airline_start_page(code)
                        .ok_or_else(|| JobError::NoStartPage(site.clone()))?,
                    false,
                    Some(code.clone()),
                ),
                Source::Miles { program } => (
                    miles_start_page(program)
                        .ok_or_else(|| JobError::NoStartPage(site.clone()))?,
                    true,
                    None,
                ),
                Source::GoogleFlights => return Err(JobError::Mismatch),
            };
            let departure = options
                .agent_departure
                .filter(|d| *d > today)
                .unwrap_or(today + Days::new(45));
            let (from, to, return_date) = match kind {
                FamilyKind::Leg { leg: Leg::Outbound } => {
                    (&route.origin, &route.destination, None)
                }
                FamilyKind::Leg { leg: Leg::Return } => {
                    (&route.destination, &route.origin, None)
                }
                FamilyKind::RoundTrip { days } => (
                    &route.origin,
                    &route.destination,
                    Some(departure + Days::new(u64::from(days))),
                ),
                FamilyKind::Explore => return Err(JobError::Mismatch),
            };
            Ok(Job::Agent(AgentJob {
                site,
                start_url: start_url.to_owned(),
                task: agent_task(
                    from,
                    to,
                    departure,
                    return_date,
                    key.cabin,
                    settings,
                    miles,
                ),
                departure,
                return_date,
                miles,
                carrier,
            }))
        }
    }
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

/// Where Jev starts on each airline's site (0012). Best guesses: these
/// pages have not been studied (0047).
pub fn airline_start_page(code: &str) -> Option<&'static str> {
    Some(match code {
        "LA" => "https://www.latamairlines.com/br/pt",
        "AD" => "https://www.voeazul.com.br/br/pt/home",
        "G3" => "https://www.voegol.com.br/",
        "TP" => "https://www.flytap.com/pt-br",
        "IB" => "https://www.iberia.com/br/",
        "AF" => "https://wwws.airfrance.com.br/",
        "AA" => "https://www.aa.com/homePage.do?locale=pt_BR",
        "UA" => "https://www.united.com/pt-br/br",
        "EK" => "https://www.emirates.com/br/portuguese/",
        "QR" => "https://www.qatarairways.com/pt-br/homepage.html",
        "TK" => "https://www.turkishairlines.com/pt-br/",
        _ => return None,
    })
}

/// Where Jev starts on each miles program's site (0023).
pub fn miles_start_page(program: &str) -> Option<&'static str> {
    Some(match program {
        "smiles" => "https://www.smiles.com.br/home",
        "latam_pass" => "https://latampass.latam.com/pt_br/",
        "azul_fidelidade" => {
            "https://www.voeazul.com.br/br/pt/programa-fidelidade"
        }
        _ => return None,
    })
}

fn cabin_words(cabin: Cabin) -> &'static str {
    match cabin {
        Cabin::Economy => "economy class",
        Cabin::PremiumEconomy => "premium economy class",
        Cabin::Business => "business class",
        Cabin::First => "first class",
    }
}

/// The Jev task for an airline or miles search. It stops at the
/// results: nothing is selected, logged into or booked.
pub fn agent_task(
    from: &Place,
    to: &Place,
    departure: NaiveDate,
    return_date: Option<NaiveDate>,
    cabin: Cabin,
    settings: &Settings,
    miles: bool,
) -> Task {
    let p = settings.passengers;
    let party = if p.total() == 1 {
        "1 adult".to_owned()
    } else {
        format!(
            "{} adults, {} children and {} infants",
            p.adults,
            p.children,
            p.infants_in_seat + p.infants_on_lap
        )
    };
    let kind = if return_date.is_some() {
        "round-trip"
    } else {
        "one-way"
    };
    let award = if miles {
        " paid with miles or points (award tickets)"
    } else {
        ""
    };
    let dates = match return_date {
        Some(r) => format!(
            "departing on {} and returning on {}",
            departure.format("%d %B %Y"),
            r.format("%d %B %Y")
        ),
        None => format!("on {}", departure.format("%d %B %Y")),
    };
    let goal = format!(
        "Search {kind} flights{award} from {} ({}) to {} ({}) {dates}, \
         for {party}, in {}. Stop as soon as a list of flights with prices \
         is visible. Do not select a flight, do not log in, do not book \
         and do not pay.",
        from.name,
        from.code,
        to.name,
        to.code,
        cabin_words(cabin),
    );
    let mut task = Task::new(goal)
        .value("origin", format!("{} ({})", from.name, from.code))
        .value("origin code", from.code.clone())
        .value("destination", format!("{} ({})", to.name, to.code))
        .value("destination code", to.code.clone())
        .value("departure date", departure.format("%Y-%m-%d").to_string())
        .value(
            "departure date dd/mm/yyyy",
            departure.format("%d/%m/%Y").to_string(),
        )
        .value("adults", p.adults.to_string());
    if let Some(r) = return_date {
        task = task
            .value("return date", r.format("%Y-%m-%d").to_string())
            .value("return date dd/mm/yyyy", r.format("%d/%m/%Y").to_string());
    }
    task.max_steps = 25;
    task
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
                agent_departure: None,
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

    #[test]
    fn airline_and_miles_searches_are_jev_tasks() {
        let settings = Settings::default();
        let options = JobOptions {
            agent_departure: Some(day(11, 10)),
            ..JobOptions::default()
        };
        let job = build(
            &key(
                Source::Airline { code: "TP".into() },
                FamilyKind::Leg { leg: Leg::Outbound },
            ),
            &route(PlaceKind::Airport),
            &pos(),
            &settings,
            day(10, 6),
            &options,
        );
        let Ok(Job::Agent(job)) = job else {
            panic!("an agent job: {job:?}");
        };
        assert_eq!(job.site, "airline:TP");
        assert_eq!(job.start_url, "https://www.flytap.com/pt-br");
        assert!(!job.miles);
        assert_eq!(job.departure, day(11, 10));
        assert!(job.task.goal.contains("one-way"));
        assert!(job.task.goal.contains("do not book"));
        assert_eq!(
            job.task
                .values
                .get("departure date dd/mm/yyyy")
                .map(String::as_str),
            Some("10/11/2026")
        );

        let job = build(
            &key(
                Source::Miles {
                    program: "smiles".into(),
                },
                FamilyKind::RoundTrip { days: 7 },
            ),
            &route(PlaceKind::Airport),
            &pos(),
            &settings,
            day(10, 6),
            &JobOptions::default(),
        );
        let Ok(Job::Agent(job)) = job else {
            panic!("an agent job: {job:?}");
        };
        assert!(job.miles);
        assert_eq!(job.departure, day(11, 20));
        assert_eq!(job.return_date, Some(day(11, 27)));
        assert!(job.task.goal.contains("award"));
        assert_eq!(
            build(
                &key(
                    Source::Airline { code: "ZZ".into() },
                    FamilyKind::Leg { leg: Leg::Outbound }
                ),
                &route(PlaceKind::Airport),
                &pos(),
                &settings,
                day(10, 6),
                &JobOptions::default(),
            ),
            Err(JobError::NoStartPage("airline:ZZ".into()))
        );
    }
}
