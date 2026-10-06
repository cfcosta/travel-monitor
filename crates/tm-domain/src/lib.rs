//! The travel monitor's shared vocabulary (DECISIONS.md 0048).
//!
//! Plain data and the small rules that belong to it (money arithmetic,
//! trip arithmetic, days-to-departure buckets). Nothing here performs
//! I/O; the store, the scheduler, the daemon and the app all speak in
//! these types.

pub mod family;
pub mod money;
pub mod observation;
pub mod place;
pub mod query;
pub mod settings;

pub use family::{
    FamilyKey,
    FamilyKind,
    GOOGLE_FLIGHTS,
    Leg,
    SiteLimits,
    Source,
    miles_session_domains,
    site_slug,
};
pub use money::{Currency, Money, effective_brl};
pub use observation::{
    Deal,
    DealReason,
    DealStatus,
    ExploreHint,
    Observation,
    SellerQuote,
};
pub use place::{Cabin, CountryCode, Place, PlaceKind, PointOfSale, Route};
pub use query::{QueryOutcome, QueryReason};
pub use settings::{Passengers, Settings, TripRange};

/// Identifiers are SQLite row ids. Each kind gets its own type so they
/// cannot be mixed up.
macro_rules! id {
    ($($name:ident),* $(,)?) => {$(
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    )*};
}

id!(
    PlaceId,
    RouteId,
    PosId,
    FamilyId,
    QueryId,
    ObservationId,
    DealId,
    RequestId
);
