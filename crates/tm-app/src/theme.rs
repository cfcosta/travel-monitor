//! The night palette and the fonts of the designs (DECISIONS.md 0044:
//! "G · Map + list" and the screens that follow its look).

use gpui::{App, Hsla, Rgba, SharedString, rgb, rgba};

/// The window's ground.
pub const GROUND: u32 = 0x060A14;
/// The top bar.
pub const BAR: u32 = 0x080D1A;
/// Side panels (the list, the route rail).
pub const PANEL: u32 = 0x070C18;
/// Cards, inputs, stat tiles.
pub const CARD: u32 = 0x0F1626;
/// A section's own ground (heatmap, tables).
pub const SECTION: u32 = 0x080D1A;
/// The selected row.
pub const SELECTED: u32 = 0x111B31;
/// The active nav tab.
pub const NAV_ACTIVE: u32 = 0x16203A;
/// A deal row's ground in tables.
pub const DEAL_ROW: u32 = 0x0B1222;

pub const BORDER: u32 = 0x1A2438;
pub const BORDER_CARD: u32 = 0x1E2A40;
pub const BORDER_INPUT: u32 = 0x24314A;
pub const BORDER_BUTTON: u32 = 0x2E3D5A;
pub const ROW_RULE: u32 = 0x121B2C;
pub const ROW_RULE_FAINT: u32 = 0x101828;

pub const TEXT: u32 = 0xE8EEF5;
pub const TEXT_SOFT: u32 = 0xC9D4E0;
pub const TEXT_MUTED: u32 = 0xA9BACB;
pub const TEXT_LABEL: u32 = 0x8DA2B8;
pub const TEXT_FAINT: u32 = 0x7F93A8;
pub const TEXT_DIM: u32 = 0x5E7590;

/// Origins and the system's own state.
pub const TEAL: u32 = 0x7CF2E0;
/// Deals.
pub const YELLOW: u32 = 0xFFD166;
/// Text on yellow.
pub const ON_YELLOW: u32 = 0x1A1406;
/// A deal's big price.
pub const PRICE_DEAL: u32 = 0xFFF3D6;
/// A drop that is not a deal.
pub const DROP: u32 = 0x9FD3F5;
pub const DROP_BG: u32 = 0x12304A;
pub const FLAT_BG: u32 = 0x1A2232;
pub const RED: u32 = 0xFF7A6B;

// Map.
/// Land dots, faded towards the globe's rim (0074).
pub const LAND: u32 = 0x3A5677;
/// The globe's sea, a little lighter than the ground.
pub const OCEAN: u32 = 0x0A1428;
/// The glow around the globe's rim.
pub const ATMOSPHERE: u32 = 0x4F8FC0;
pub const ARC_DEAL: u32 = 0x8A6F2E;
pub const LABEL_DEAL: u32 = 0xE9D9A8;
pub const LABEL: u32 = 0x7F9FB8;

/// Arc colours by region (0075); yellow stays for deals.
pub fn region(region: crate::geo::Region) -> u32 {
    use crate::geo::Region;
    match region {
        Region::Domestic => 0x4FD1C5,
        Region::SouthAmerica => 0x86D46B,
        Region::CentralAmerica => 0xE58FD8,
        Region::NorthAmerica => 0x5B9DF5,
        Region::Europe => 0xA78BFA,
        Region::AfricaMiddleEast => 0xF28C5B,
        Region::Asia => 0xF0607A,
        Region::Oceania => 0xC9A27E,
        Region::Other => 0x6B7A90,
    }
}

// Sparklines and bars.
pub const SPARK: u32 = 0x4F7EA0;
pub const BAR_DROP: u32 = 0x3E7BA6;
pub const BAR_RISE: u32 = 0x5E6B80;

// Heatmap tones (route detail).
pub const TONE_BELOW: u32 = 0x3B87A8;
pub const TONE_TYPICAL: u32 = 0x1C2A44;
pub const TONE_ABOVE: u32 = 0x4A2F2A;
/// Grid cells that are not deals.
pub const CELL: u32 = 0x142038;

pub fn color(hex: u32) -> Rgba {
    rgb(hex)
}

/// `hex` with `alpha` (0..=1).
pub fn alpha(hex: u32, alpha: f32) -> Hsla {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
    rgba((hex << 8) | a).into()
}

/// The two typefaces of the designs, or the closest the system has.
#[derive(Clone, Debug)]
pub struct Fonts {
    pub sans: SharedString,
    pub mono: SharedString,
}

impl gpui::Global for Fonts {}

const SANS: &[&str] = &[
    "Space Grotesk",
    "Inter",
    "Geist",
    "IBM Plex Sans",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "FreeSans",
];

const MONO: &[&str] = &[
    "JetBrains Mono",
    "Geist Mono",
    "IBM Plex Mono",
    "Berkeley Mono",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "FreeMono",
];

/// The first of `wanted` that `available` has, else `fallback`.
pub fn pick_font(
    wanted: &[&str],
    available: &[String],
    fallback: &str,
) -> SharedString {
    wanted
        .iter()
        .find(|name| available.iter().any(|have| have == *name))
        .map_or_else(|| fallback.to_owned().into(), |name| (*name).into())
}

impl Fonts {
    /// Space Grotesk and JetBrains Mono when installed; otherwise the
    /// closest installed family, and gpui's own fallback stack last.
    pub fn detect(cx: &App) -> Self {
        let available = cx.text_system().all_font_names();
        Self {
            sans: pick_font(SANS, &available, ".SystemUIFont"),
            mono: pick_font(MONO, &available, "monospace"),
        }
    }

    pub fn install(cx: &mut App) {
        let fonts = Self::detect(cx);
        cx.set_global(fonts);
    }
}

pub fn fonts(cx: &App) -> Fonts {
    cx.try_global::<Fonts>().cloned().unwrap_or_else(|| Fonts {
        sans: ".SystemUIFont".into(),
        mono: "monospace".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_installed_font() {
        let available = vec!["DejaVu Sans".to_owned(), "Inter".to_owned()];
        assert_eq!(pick_font(SANS, &available, "x"), "Inter");
        assert_eq!(pick_font(MONO, &available, "mono"), "mono");
    }

    #[test]
    fn alpha_keeps_the_color() {
        let a = alpha(0xFFD166, 0.5);
        let full: Hsla = rgb(0xFFD166).into();
        assert!((a.h - full.h).abs() < 1e-3);
        assert!((a.a - 0.5).abs() < 0.01);
    }
}
