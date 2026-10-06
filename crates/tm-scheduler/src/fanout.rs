//! Probe, then fan out (DECISIONS.md 0028; Amadeus US 9,235,620: related
//! entries re-price together).
//!
//! When a family shows a drop, the families that differ from it in exactly
//! one way, on the same route, are worth looking at sooner:
//!
//! - another cabin (same source, point of sale and kind);
//! - another point of sale (same source, cabin and kind);
//! - another source (Google Flights, an airline, a miles program; same
//!   cabin, point of sale and kind);
//! - a leg ↔ the round trip (same source, cabin and point of sale).
//!
//! The outbound and return legs are not related to each other, and
//! Explore families are never related to anything. The daemon either
//! schedules the related families with `QueryReason::FanOut` or adds
//! [`boost`] to their state.

use tm_domain::{FamilyKey, FamilyKind};

use crate::state::FamilyState;

/// How a related family differs from the one that dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    OtherCabin,
    OtherPos,
    OtherSource,
    LegRoundTrip,
}

/// How `other` relates to `dropped`, if it does.
pub fn relation(dropped: &FamilyKey, other: &FamilyKey) -> Option<Relation> {
    if dropped.route != other.route
        || matches!(dropped.kind, FamilyKind::Explore)
        || matches!(other.kind, FamilyKind::Explore)
    {
        return None;
    }
    let same_cabin = dropped.cabin == other.cabin;
    let same_pos = dropped.pos == other.pos;
    let same_source = dropped.source == other.source;
    let same_kind = dropped.kind == other.kind;
    let leg_rt = matches!(
        (dropped.kind, other.kind),
        (FamilyKind::Leg { .. }, FamilyKind::RoundTrip { .. })
            | (FamilyKind::RoundTrip { .. }, FamilyKind::Leg { .. })
    );
    match (same_cabin, same_pos, same_source, same_kind) {
        (false, true, true, true) => Some(Relation::OtherCabin),
        (true, false, true, true) => Some(Relation::OtherPos),
        (true, true, false, true) => Some(Relation::OtherSource),
        (true, true, true, false) if leg_rt => Some(Relation::LegRoundTrip),
        _ => None,
    }
}

/// The keys among `families` related to `dropped`, in input order.
pub fn related<'a>(
    dropped: &FamilyKey,
    families: impl IntoIterator<Item = &'a FamilyKey>,
) -> Vec<(FamilyKey, Relation)> {
    families
        .into_iter()
        .filter_map(|k| relation(dropped, k).map(|r| (k.clone(), r)))
        .collect()
}

/// Raise a related family's score as one pending hint would.
pub fn boost(state: &mut FamilyState) {
    crate::hints::add_hint(state);
}

#[cfg(test)]
mod tests {
    use tm_domain::{Cabin, Leg, PosId, RouteId, Source};

    use super::*;

    fn key(
        route: i64,
        cabin: Cabin,
        source: Source,
        pos: i64,
        kind: FamilyKind,
    ) -> FamilyKey {
        FamilyKey {
            route: RouteId(route),
            cabin,
            source,
            pos: PosId(pos),
            kind,
        }
    }

    #[test]
    fn finds_each_relation() {
        let out = FamilyKind::Leg { leg: Leg::Outbound };
        let ret = FamilyKind::Leg { leg: Leg::Return };
        let rt = FamilyKind::RoundTrip { days: 7 };
        let gf = Source::GoogleFlights;
        let la = Source::Airline { code: "LA".into() };
        let d = key(1, Cabin::Economy, gf.clone(), 1, out);
        let all = vec![
            d.clone(),
            key(1, Cabin::Business, gf.clone(), 1, out),
            key(1, Cabin::Economy, gf.clone(), 2, out),
            key(1, Cabin::Economy, la.clone(), 1, out),
            key(1, Cabin::Economy, gf.clone(), 1, rt),
            key(1, Cabin::Economy, gf.clone(), 1, ret),
            key(1, Cabin::Economy, gf.clone(), 1, FamilyKind::Explore),
            key(2, Cabin::Economy, gf.clone(), 1, out),
            key(1, Cabin::Business, gf.clone(), 2, out),
            key(1, Cabin::Business, la, 1, rt),
        ];
        let got: Vec<Relation> =
            related(&d, &all).into_iter().map(|r| r.1).collect();
        assert_eq!(
            got,
            vec![
                Relation::OtherCabin,
                Relation::OtherPos,
                Relation::OtherSource,
                Relation::LegRoundTrip,
            ]
        );
        // Symmetric for leg ↔ round trip.
        let rt_key = key(1, Cabin::Economy, gf, 1, rt);
        assert_eq!(relation(&rt_key, &d), Some(Relation::LegRoundTrip));
    }

    #[test]
    fn boost_adds_a_hint() {
        let mut s = FamilyState::default();
        boost(&mut s);
        assert_eq!(s.n_hints, 1);
    }
}
