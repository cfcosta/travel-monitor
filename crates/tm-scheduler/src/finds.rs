//! Destinations the Explore sweep prices outside the watchlist
//! (DECISIONS.md 0087).
//!
//! Explore lists about 70 destinations per sweep and the watchlist
//! holds only some of them. The rest are *finds*: one per origin, cabin
//! and destination (Explore's Freebase ID), built from the hints of
//! unwatched destinations only (the store picks those). A find shows
//! its cheapest hint of the last day and its own history: the median of
//! its older hints, once it has been priced in enough sweeps on enough
//! days.
//!
//! A find is a **drop** when its recent price is `drop` (the deal
//! threshold, 20%) or more under that median, as a deal is under its
//! route's (0022). Explore prices are one date pair of "about a week",
//! so they are noisier than a route's series: the median needs 8 prior
//! sweeps on at least 3 different days. A drop stays a drop until the
//! price comes back above half the threshold (10% under the median), so
//! a price wavering around the line does not drop again and again.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use tm_domain::{Cabin, ExploreHint, Money, PlaceId, Settings};

/// How finds are built and judged.
#[derive(Debug, Clone, PartialEq)]
pub struct FindParams {
    /// Hints this recent are what is on sale now.
    pub recent: TimeDelta,
    /// Older hints, back to this far, are the find's history.
    pub history: TimeDelta,
    /// The history needs this many sweeps …
    pub min_prior_sweeps: usize,
    /// … on this many different days (UTC) before it has a median.
    pub min_prior_days: usize,
    /// A drop is this fraction or more under the median.
    pub drop: f64,
}

impl Default for FindParams {
    fn default() -> Self {
        Self {
            recent: TimeDelta::hours(24),
            history: TimeDelta::days(60),
            min_prior_sweeps: 8,
            min_prior_days: 3,
            drop: 0.20,
        }
    }
}

impl FindParams {
    /// The defaults, with the user's deal threshold as the drop.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            drop: settings.deal_threshold,
            ..Self::default()
        }
    }

    /// The earliest hint a find needs: the start of its history.
    pub fn since(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now - self.recent - self.history
    }
}

/// One unwatched destination, as Explore priced it from one origin in
/// one cabin.
#[derive(Debug, Clone, PartialEq)]
pub struct ExploreFind {
    pub origin: PlaceId,
    pub cabin: Cabin,
    /// Explore's destination (`"/m/02j9z"`).
    pub freebase_id: String,
    /// The cheapest hint of the last `recent`: price, dates, carrier,
    /// stops and the airport priced.
    pub cheapest: ExploreHint,
    /// Every airport Explore priced for it, most often first.
    pub airports: Vec<String>,
    /// The latest name, country (in words) and coordinates Explore gave.
    pub name: Option<String>,
    pub country: Option<String>,
    pub coordinates: Option<(f64, f64)>,
    /// Sweeps that priced it, recent and older.
    pub sweeps: usize,
    /// Older sweeps, and the days they ran on.
    pub prior_sweeps: usize,
    pub prior_days: usize,
    /// The median of the older hints, once there are enough of them.
    pub usual: Option<Money>,
}

/// What a find's price says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// `drop` or more under its own median.
    Drop,
    /// Under the median, but less than a drop and more than half of one:
    /// an open drop stays open, no new one starts.
    Between,
    /// No median yet, or not much under it.
    Normal,
}

impl ExploreFind {
    /// The recent price against the usual one (`-0.25` is 25% under).
    pub fn change(&self) -> Option<f64> {
        let usual = self.usual.as_ref()?;
        (usual.minor > 0).then(|| {
            self.cheapest.price.minor as f64 / usual.minor as f64 - 1.0
        })
    }

    pub fn verdict(&self, params: &FindParams) -> Verdict {
        // Exactly 20% under is a drop, whatever the float rounding.
        const EPS: f64 = 1e-9;
        match self.change() {
            Some(c) if c <= -params.drop + EPS => Verdict::Drop,
            Some(c) if c <= -params.drop / 2.0 + EPS => Verdict::Between,
            _ => Verdict::Normal,
        }
    }

    /// The airport named most often.
    pub fn main_airport(&self) -> &str {
        self.airports
            .first()
            .map_or(self.cheapest.destination_airport.as_str(), |a| a)
    }
}

/// Whose hint is whose: origin, cabin and destination.
type Key = (PlaceId, Cabin, String);

fn key_of(hint: &ExploreHint) -> Key {
    let destination = if hint.destination_freebase_id.is_empty() {
        format!("airport:{}", hint.destination_airport)
    } else {
        hint.destination_freebase_id.clone()
    };
    (hint.origin, hint.cabin, destination)
}

/// The median of integer prices, rounded to the nearest minor unit.
fn median(mut xs: Vec<i64>) -> Option<i64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_unstable();
    let n = xs.len();
    Some(if n % 2 == 1 {
        xs[n / 2]
    } else {
        // Rounds half away from zero; prices are positive.
        (xs[n / 2 - 1] + xs[n / 2] + 1) / 2
    })
}

/// The finds among `hints` (hints of unwatched destinations): one per
/// origin, cabin and destination priced in the last `recent`. Drops
/// first, biggest first, then the rest by price.
pub fn explore_finds(
    hints: &[ExploreHint],
    now: DateTime<Utc>,
    params: &FindParams,
) -> Vec<ExploreFind> {
    let recent_from = now - params.recent;
    let history_from = recent_from - params.history;
    let mut groups: BTreeMap<Key, Vec<&ExploreHint>> = BTreeMap::new();
    for h in hints {
        if h.observed_at >= history_from && h.observed_at <= now {
            groups.entry(key_of(h)).or_default().push(h);
        }
    }
    let mut out: Vec<ExploreFind> = groups
        .into_iter()
        .filter_map(|((origin, cabin, freebase_id), group)| {
            let cheapest = group
                .iter()
                .filter(|h| h.observed_at > recent_from)
                .min_by_key(|h| {
                    (h.price.minor, std::cmp::Reverse(h.observed_at))
                })
                .copied()?;
            let currency = &cheapest.price.currency;
            let prior: Vec<&&ExploreHint> = group
                .iter()
                .filter(|h| {
                    h.observed_at <= recent_from
                        && &h.price.currency == currency
                })
                .collect();
            let prior_sweeps =
                prior.iter().map(|h| h.query).collect::<BTreeSet<_>>().len();
            let prior_days = prior
                .iter()
                .map(|h| h.observed_at.date_naive())
                .collect::<BTreeSet<NaiveDate>>()
                .len();
            let usual = (prior_sweeps >= params.min_prior_sweeps
                && prior_days >= params.min_prior_days)
                .then(|| median(prior.iter().map(|h| h.price.minor).collect()))
                .flatten()
                .map(|m| Money::new(m, currency.clone()));
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for h in &group {
                if !h.destination_airport.is_empty() {
                    *counts
                        .entry(h.destination_airport.as_str())
                        .or_default() += 1;
                }
            }
            let mut airports: Vec<(&str, usize)> = counts.into_iter().collect();
            airports.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let latest = |f: &dyn Fn(&ExploreHint) -> bool| {
                group
                    .iter()
                    .filter(|h| f(h))
                    .max_by_key(|h| h.observed_at)
                    .copied()
            };
            Some(ExploreFind {
                origin,
                cabin,
                freebase_id,
                airports: airports
                    .into_iter()
                    .map(|(a, _)| a.to_owned())
                    .collect(),
                name: latest(&|h| h.destination_name.is_some())
                    .and_then(|h| h.destination_name.clone()),
                country: latest(&|h| h.destination_country.is_some())
                    .and_then(|h| h.destination_country.clone()),
                coordinates: latest(&|h| h.destination_coordinates.is_some())
                    .and_then(|h| h.destination_coordinates),
                sweeps: group
                    .iter()
                    .map(|h| h.query)
                    .collect::<BTreeSet<_>>()
                    .len(),
                prior_sweeps,
                prior_days,
                usual,
                cheapest: cheapest.clone(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let change = |f: &ExploreFind| f.change().unwrap_or(f64::INFINITY);
        change(a)
            .total_cmp(&change(b))
            .then(a.cheapest.price.minor.cmp(&b.cheapest.price.minor))
            .then(a.freebase_id.cmp(&b.freebase_id))
            .then(a.cabin.cmp(&b.cabin))
    });
    out
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use hegel::{TestCase, generators as gs};
    use tm_domain::{PosId, QueryId};

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0)
            .single()
            .expect("valid time")
    }

    fn hint(
        query: i64,
        hours_ago: i64,
        freebase: &str,
        airport: &str,
        price: i64,
    ) -> ExploreHint {
        ExploreHint {
            query: QueryId(query),
            origin: PlaceId(1),
            cabin: Cabin::Economy,
            pos: PosId(1),
            observed_at: now() - TimeDelta::hours(hours_ago),
            destination_freebase_id: freebase.to_owned(),
            destination_airport: airport.to_owned(),
            price: Money::brl_whole(price),
            departure: None,
            return_date: None,
            stops: Some(1),
            duration_minutes: None,
            carrier: Some("Garuda".into()),
            destination_name: Some("Jakarta".into()),
            destination_country: Some("Indonesia".into()),
            destination_coordinates: Some((-6.2, 106.8)),
        }
    }

    /// Ten sweeps, one every 12 hours from 2 to 6.5 days ago, at `price`.
    fn history(freebase: &str, airport: &str, price: i64) -> Vec<ExploreHint> {
        (0..10)
            .map(|i| hint(100 + i, 48 + 12 * i, freebase, airport, price))
            .collect()
    }

    #[test]
    fn the_cheapest_recent_hint_against_older_ones() {
        let mut hints = history("/m/044rv", "CGK", 7_000);
        hints.push(hint(1, 3, "/m/044rv", "CGK", 5_400));
        hints.push(hint(2, 2, "/m/044rv", "HLP", 5_200));
        // Older than the history: ignored.
        hints.push(hint(3, 24 * 70, "/m/044rv", "CGK", 1_000));
        let finds = explore_finds(&hints, now(), &FindParams::default());
        assert_eq!(finds.len(), 1);
        let f = &finds[0];
        assert_eq!(f.cheapest.price, Money::brl_whole(5_200));
        assert_eq!(f.cheapest.destination_airport, "HLP");
        assert_eq!(f.usual, Some(Money::brl_whole(7_000)));
        assert_eq!(f.sweeps, 12);
        assert_eq!(f.prior_sweeps, 10);
        assert_eq!(f.airports, ["CGK", "HLP"]);
        assert_eq!(f.main_airport(), "CGK");
        assert_eq!(f.name.as_deref(), Some("Jakarta"));
        let change = f.change().expect("a median");
        assert!((change - (5_200.0 / 7_000.0 - 1.0)).abs() < 1e-9);
        assert_eq!(f.verdict(&FindParams::default()), Verdict::Drop);
    }

    #[test]
    fn only_destinations_priced_lately_are_finds() {
        let finds = explore_finds(
            &history("/m/x", "XXX", 900),
            now(),
            &FindParams::default(),
        );
        assert!(finds.is_empty());
    }

    #[test]
    fn a_median_needs_enough_sweeps_on_enough_days() {
        let p = FindParams::default();
        // Seven sweeps: not enough.
        let mut hints: Vec<ExploreHint> =
            history("/m/x", "XXX", 7_000).into_iter().take(7).collect();
        hints.push(hint(1, 1, "/m/x", "XXX", 3_000));
        let f = &explore_finds(&hints, now(), &p)[0];
        assert_eq!(f.usual, None);
        assert_eq!(f.verdict(&p), Verdict::Normal);
        // Ten sweeps, all on one day: still not enough.
        let mut hints: Vec<ExploreHint> = (0..10)
            .map(|i| hint(100 + i, 26 + i, "/m/x", "XXX", 7_000))
            .collect();
        hints.push(hint(1, 1, "/m/x", "XXX", 3_000));
        let f = &explore_finds(&hints, now(), &p)[0];
        assert_eq!(f.prior_sweeps, 10);
        assert_eq!(f.prior_days, 1);
        assert_eq!(f.usual, None);
    }

    #[test]
    fn verdicts_have_a_band_between_drop_and_normal() {
        let p = FindParams::default();
        let at = |price| {
            let mut hints = history("/m/x", "XXX", 10_000);
            hints.push(hint(1, 1, "/m/x", "XXX", price));
            explore_finds(&hints, now(), &p)[0].verdict(&p)
        };
        assert_eq!(at(8_000), Verdict::Drop);
        assert_eq!(at(8_500), Verdict::Between);
        assert_eq!(at(9_000), Verdict::Between);
        assert_eq!(at(9_100), Verdict::Normal);
        assert_eq!(at(12_000), Verdict::Normal);
    }

    #[test]
    fn finds_are_per_cabin_and_drops_come_first() {
        let mut hints = history("/m/a", "AAA", 4_000);
        hints.push(hint(1, 1, "/m/a", "AAA", 3_000));
        hints.push(hint(1, 1, "/m/b", "BBB", 2_000));
        let mut business = hint(2, 1, "/m/a", "AAA", 9_000);
        business.cabin = Cabin::Business;
        hints.push(business);
        let finds = explore_finds(&hints, now(), &FindParams::default());
        let order: Vec<(&str, Cabin)> = finds
            .iter()
            .map(|f| (f.freebase_id.as_str(), f.cabin))
            .collect();
        assert_eq!(
            order,
            [
                ("/m/a", Cabin::Economy),
                ("/m/b", Cabin::Economy),
                ("/m/a", Cabin::Business)
            ]
        );
    }

    #[test]
    fn medians_round_to_the_nearest_unit() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3, 1, 2]), Some(2));
        assert_eq!(median(vec![1, 2]), Some(2));
        assert_eq!(median(vec![10, 20, 30, 41]), Some(25));
    }

    #[hegel::test(test_cases = 200)]
    fn finds_are_bounded_by_their_hints(tc: TestCase) {
        let prices: Vec<i64> = tc.draw(
            gs::vecs(gs::integers::<i64>().min_value(1).max_value(1_000_000))
                .min_size(1)
                .max_size(40),
        );
        let hours: Vec<i64> = tc.draw(
            gs::vecs(gs::integers::<i64>().min_value(0).max_value(24 * 30))
                .min_size(prices.len())
                .max_size(prices.len()),
        );
        let hints: Vec<ExploreHint> = prices
            .iter()
            .zip(&hours)
            .enumerate()
            .map(|(i, (&p, &h))| hint(i as i64, h, "/m/x", "XXX", p))
            .collect();
        let p = FindParams::default();
        let finds = explore_finds(&hints, now(), &p);
        let recent: Vec<i64> = hints
            .iter()
            .filter(|h| h.observed_at > now() - p.recent)
            .map(|h| h.price.minor)
            .collect();
        match recent.iter().min() {
            None => assert!(finds.is_empty()),
            Some(&lo) => {
                assert_eq!(finds.len(), 1);
                let f = &finds[0];
                assert_eq!(f.cheapest.price.minor, lo);
                assert_eq!(f.sweeps, hints.len());
                assert_eq!(f.sweeps, f.prior_sweeps + recent.len());
                if let Some(usual) = &f.usual {
                    let prior = hints
                        .iter()
                        .filter(|h| h.observed_at <= now() - p.recent)
                        .map(|h| h.price.minor);
                    let (min, max) = prior
                        .fold((i64::MAX, i64::MIN), |(a, b), x| {
                            (a.min(x), b.max(x))
                        });
                    assert!(usual.minor >= min && usual.minor <= max);
                    assert!(f.prior_sweeps >= p.min_prior_sweeps);
                }
                if f.verdict(&p) == Verdict::Drop {
                    assert!(f.change().is_some_and(|c| c <= -p.drop + 1e-9));
                }
            }
        }
    }
}
