//! Airline and miles sites with an extraction built on a probe of the
//! site (DECISIONS.md 0050), each in its own module. Only the sites with
//! a reader run; every other airline and miles site stays off (0052,
//! 0059).
//!
//! - [`tap`]: TAP Air Portugal, `airline:TP` (0051). Has a reader.
//! - [`latam`]: LATAM, `airline:LA` and LATAM Pass `miles:latam_pass`
//!   (0081, 0082). Probed and parsed, no reader yet: both need the
//!   offscreen display, LATAM Pass a login.
//!
//! [`KNOWN`] says what each probed site needs.

use tm_domain::{FamilyKey, Passengers, Source};

pub mod latam;
pub mod tap;

/// What a probed site needs from the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownSite {
    /// `Source::site`: `airline:TP`, `miles:latam_pass`.
    pub site: &'static str,
    /// A headed Chromium on a display (the offscreen compositor, 0081):
    /// its bot protection denies headless Chromium.
    pub needs_display: bool,
    /// A login made by hand in the site's profile (0024, 0064).
    pub needs_login: bool,
    /// The daemon reads its prices ([`scripted`]).
    pub has_reader: bool,
    /// Where the probe is written down.
    pub research: &'static str,
}

/// The sites probed so far (0050).
pub const KNOWN: &[KnownSite] = &[
    KnownSite {
        site: tap::SITE,
        needs_display: false,
        needs_login: false,
        has_reader: true,
        research: "docs/research/sites/tap.md",
    },
    KnownSite {
        site: latam::AIRLINE_SITE,
        needs_display: true,
        needs_login: false,
        has_reader: false,
        research: "docs/research/sites/latam.md",
    },
    KnownSite {
        site: latam::MILES_SITE,
        needs_display: true,
        needs_login: true,
        has_reader: false,
        research: "docs/research/sites/latam-pass.md",
    },
];

/// What is known about `site`, if it was probed.
pub fn known(site: &str) -> Option<&'static KnownSite> {
    KNOWN.iter().find(|k| k.site == site)
}

/// Whether `site` must run headed on a display (0081).
pub fn needs_display(site: &str) -> bool {
    known(site).is_some_and(|k| k.needs_display)
}

/// Whether `site` must be logged in by hand before it can be read.
pub fn needs_login(site: &str) -> bool {
    known(site).is_some_and(|k| k.needs_login)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_are_the_scripted_sites() {
        for k in KNOWN {
            let source = Source::from_site(k.site).expect("a site");
            assert_eq!(source.site(), k.site);
            assert_eq!(scripted(&source), k.has_reader, "{}", k.site);
        }
    }

    #[test]
    fn latam_needs_a_display_and_latam_pass_a_login() {
        assert!(needs_display("airline:LA"));
        assert!(needs_display("miles:latam_pass"));
        assert!(!needs_display("airline:TP"));
        assert!(!needs_display("google_flights"));
        assert!(needs_login("miles:latam_pass"));
        assert!(!needs_login("airline:LA"));
        assert!(!scripted(&Source::Airline { code: "LA".into() }));
        assert!(!scripted(&Source::Miles {
            program: latam::PROGRAM.into()
        }));
        let program = Source::Miles {
            program: latam::PROGRAM.into(),
        };
        assert_eq!(program.site(), latam::MILES_SITE);
    }

    /// The browser source draws exactly the sites that need a display
    /// offscreen.
    #[test]
    fn the_offscreen_sites_are_the_ones_that_need_a_display() {
        let mut needing: Vec<_> = KNOWN
            .iter()
            .filter(|k| k.needs_display)
            .map(|k| k.site)
            .collect();
        let mut offscreen = crate::sources::browser::OFFSCREEN_SITES.to_vec();
        needing.sort_unstable();
        offscreen.sort_unstable();
        assert_eq!(needing, offscreen);
    }

    #[test]
    fn every_probe_is_written_down() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for k in KNOWN {
            assert!(root.join(k.research).is_file(), "{}", k.research);
        }
    }
}
