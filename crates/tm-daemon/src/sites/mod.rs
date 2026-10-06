//! Airline and miles sites with an extraction built on a probe of the
//! site (DECISIONS.md 0050), each in its own module. Only these run;
//! every other airline and miles site stays off (0052), and their
//! Jev-driven path (0047) is kept but unreachable until a site is added
//! here.
//!
//! - [`tap`]: TAP Air Portugal, `airline:TP` (0051).

use std::collections::HashSet;

use tm_domain::{FamilyKey, Passengers, Source};

pub mod tap;

/// The rate-limit sites (`Source::site`) whose extraction has been
/// built on a probe and tested against captured responses.
pub const VERIFIED_SITES: &[&str] = &[tap::SITE];

/// [`VERIFIED_SITES`] as the candidate filter wants them.
pub fn verified_sites() -> HashSet<String> {
    VERIFIED_SITES.iter().map(|s| (*s).to_owned()).collect()
}

/// Whether `source` has its own scripted source in the daemon, rather
/// than the Jev-driven one: it then runs without Jev.
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
