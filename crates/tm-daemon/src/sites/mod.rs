//! Airline and miles sites with an extraction built on a probe of the
//! site (DECISIONS.md 0050), each in its own module. Only these run;
//! every other airline and miles site has no reader and stays off
//! (0052, 0059).
//!
//! - [`tap`]: TAP Air Portugal, `airline:TP` (0051).

use tm_domain::{FamilyKey, Passengers, Source};

pub mod tap;

/// Whether `source` is an airline or miles site with its own reader in
/// the daemon. Every other one is skipped.
pub fn scripted(source: &Source) -> bool {
    matches!(source, Source::Airline { code } if code == tap::CODE)
}

/// Why a family of a scripted site cannot be checked under these
/// settings, if it cannot. Families of other sources always pass.
pub fn unsupported(
    key: &FamilyKey,
    passengers: &Passengers,
) -> Option<&'static str> {
    match &key.source {
        Source::Airline { code } if code == tap::CODE => {
            tap::unsupported(key.kind, passengers)
        }
        _ => None,
    }
}
