//! Explore hints → families (DECISIONS.md 0028, 0041).
//!
//! The daily Explore sweep prices "about a week" with one date pair per
//! destination. It is not a series, but a destination whose Explore price
//! is well under what we last saw for the route *may* have dropped. Such a
//! hint adds one to `n_hints` of every family on that route and cabin,
//! which the score turns into extra effective time, weighted by how often
//! that family's past hints were confirmed.
//!
//! A hint matches a family when:
//!
//! - the origin is the same place;
//! - the cabin is the same;
//! - the destination matches: the hint's airport is the family's
//!   destination code or one of its airports, or the Freebase IDs are
//!   equal;
//! - the family's route has a known reference minimum, and the hint's
//!   price is more than `hint_drop` (10%) below it. With no reference
//!   there is nothing to compare: such families are still bootstrapping.
//!
//! Points of sale are not compared: a drop seen from one market is a
//! reason to look at the route in every market. The reference minimum
//! should be comparable to an Explore price (a round trip of about a
//! week, in the hint's currency); the caller chooses it, typically the
//! route's latest cheapest 7-day trip.

use std::collections::BTreeSet;

use tm_domain::{Cabin, ExploreHint, FamilyId, PlaceId};

use crate::{Params, state::FamilyState};

/// A family as the hint matcher sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct HintTarget {
    pub family: FamilyId,
    pub origin: PlaceId,
    pub cabin: Cabin,
    /// The destination's IATA code (`"LIS"`, or a city code such as
    /// `"LON"`).
    pub destination_code: String,
    /// For a city, the airports it groups (`["LHR", "LGW", …]`).
    pub destination_airports: Vec<String>,
    pub destination_freebase_id: Option<String>,
    /// The latest known minimum for the route and cabin, in the hint's
    /// currency's minor units.
    pub reference_min_minor: Option<i64>,
}

/// Whether `hint` names `target`'s destination from its origin and cabin.
pub fn destination_matches(hint: &ExploreHint, target: &HintTarget) -> bool {
    if hint.origin != target.origin || hint.cabin != target.cabin {
        return false;
    }
    let airport = hint.destination_airport.trim();
    let by_code = !airport.is_empty()
        && (target.destination_code.eq_ignore_ascii_case(airport)
            || target
                .destination_airports
                .iter()
                .any(|a| a.eq_ignore_ascii_case(airport)));
    let by_freebase = !hint.destination_freebase_id.is_empty()
        && target.destination_freebase_id.as_deref()
            == Some(hint.destination_freebase_id.as_str());
    by_code || by_freebase
}

/// Whether the hint's price is a drop worth looking at for `target`.
pub fn is_hint(
    hint: &ExploreHint,
    target: &HintTarget,
    params: &Params,
) -> bool {
    let Some(reference) = target.reference_min_minor else {
        return false;
    };
    reference > 0
        && destination_matches(hint, target)
        && (hint.price.minor as f64)
            < reference as f64 * (1.0 - params.hint_drop)
}

/// The families that each get `n_hints += 1`, once per matching hint, in
/// order: a family matched by two hints appears twice.
pub fn hinted_families(
    hints: &[ExploreHint],
    targets: &[HintTarget],
    params: &Params,
) -> Vec<FamilyId> {
    let mut out = Vec::new();
    for hint in hints {
        let mut seen = BTreeSet::new();
        for target in targets {
            if is_hint(hint, target, params) && seen.insert(target.family) {
                out.push(target.family);
            }
        }
    }
    out
}

/// Count one more hint for a family.
pub fn add_hint(state: &mut FamilyState) {
    state.n_hints = state.n_hints.saturating_add(1);
}

/// Learn from a check that followed hints: `confirmed` when it found the
/// drop the hints suggested.
pub fn update_hint_precision(state: &mut FamilyState, confirmed: bool) {
    if confirmed {
        state.prec_a += 1.0;
    } else {
        state.prec_b += 1.0;
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tm_domain::{Money, PosId, QueryId};

    use super::*;

    fn hint(airport: &str, freebase: &str, price: i64) -> ExploreHint {
        ExploreHint {
            query: QueryId(1),
            origin: PlaceId(1),
            cabin: Cabin::Economy,
            pos: PosId(1),
            observed_at: Utc::now(),
            destination_freebase_id: freebase.to_owned(),
            destination_airport: airport.to_owned(),
            price: Money::brl_whole(price),
            departure: None,
            return_date: None,
            stops: None,
            duration_minutes: None,
            carrier: None,
            destination_name: None,
            destination_country: None,
            destination_coordinates: None,
        }
    }

    fn target(id: i64, code: &str, reference: Option<i64>) -> HintTarget {
        HintTarget {
            family: FamilyId(id),
            origin: PlaceId(1),
            cabin: Cabin::Economy,
            destination_code: code.to_owned(),
            destination_airports: vec![],
            destination_freebase_id: None,
            reference_min_minor: reference.map(|r| r * 100),
        }
    }

    #[test]
    fn matches_by_airport_city_and_freebase() {
        let p = Params::default();
        let h = hint("LHR", "/m/04jpl", 3000);
        assert!(!is_hint(&h, &target(1, "LHR", Some(3000)), &p)); // no drop
        assert!(is_hint(&h, &target(1, "LHR", Some(3500)), &p));
        assert!(is_hint(&h, &target(1, "lhr", Some(3500)), &p));
        let mut city = target(2, "LON", Some(4000));
        assert!(!is_hint(&h, &city, &p));
        city.destination_airports = vec!["LGW".into(), "LHR".into()];
        assert!(is_hint(&h, &city, &p));
        let mut fb = target(3, "XXX", Some(4000));
        fb.destination_freebase_id = Some("/m/04jpl".into());
        assert!(is_hint(&h, &fb, &p));
    }

    #[test]
    fn requires_origin_cabin_reference_and_drop() {
        let p = Params::default();
        let h = hint("LIS", "", 3000);
        assert!(!is_hint(&h, &target(1, "LIS", None), &p));
        // 3000 is not under 3333 × 0.9 = 2999.7.
        assert!(!is_hint(&h, &target(1, "LIS", Some(3333)), &p));
        assert!(is_hint(&h, &target(1, "LIS", Some(3340)), &p));
        let mut other_cabin = target(1, "LIS", Some(5000));
        other_cabin.cabin = Cabin::Business;
        assert!(!is_hint(&h, &other_cabin, &p));
        let mut other_origin = target(1, "LIS", Some(5000));
        other_origin.origin = PlaceId(9);
        assert!(!is_hint(&h, &other_origin, &p));
        // An empty Freebase ID never matches an empty one.
        assert!(!destination_matches(
            &hint("", "", 1),
            &target(1, "", Some(5))
        ));
    }

    #[test]
    fn hinted_families_once_per_hint() {
        let p = Params::default();
        let hints = vec![hint("LIS", "", 3000), hint("LIS", "", 2900)];
        let targets = vec![
            target(1, "LIS", Some(5000)),
            target(1, "LIS", Some(5000)),
            target(2, "MAD", Some(5000)),
        ];
        assert_eq!(
            hinted_families(&hints, &targets, &p),
            vec![FamilyId(1), FamilyId(1)]
        );
    }

    #[test]
    fn precision_updates() {
        let mut s = FamilyState::default();
        add_hint(&mut s);
        assert_eq!(s.n_hints, 1);
        update_hint_precision(&mut s, true);
        update_hint_precision(&mut s, false);
        assert_eq!((s.prec_a, s.prec_b), (2.0, 3.0));
    }
}
