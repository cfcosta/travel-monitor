//! The night map's geometry (DECISIONS.md 0044): an equirectangular
//! projection fitted to a box, and the curved arcs from the origin.
//!
//! Pure functions on plain `f32` points, so they are tested without a
//! window. The map view turns them into gpui paths.

/// A point on screen (or in any 2-D plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Pt) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// A geographic window: west/east longitudes, north/south latitudes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoWindow {
    pub west: f32,
    pub east: f32,
    pub north: f32,
    pub south: f32,
}

impl GeoWindow {
    /// The designs' crop: the Americas to Japan, the Arctic circle to
    /// Patagonia (1000 × 434 in the mockups).
    pub const DEFAULT: GeoWindow = GeoWindow {
        west: -128.0,
        east: 158.0,
        north: 66.0,
        south: -58.0,
    };

    pub fn width(&self) -> f32 {
        self.east - self.west
    }

    pub fn height(&self) -> f32 {
        self.north - self.south
    }

    /// This window grown to hold every `(lon, lat)` with `margin`
    /// degrees around it, clamped to the globe.
    pub fn including(
        mut self,
        points: impl IntoIterator<Item = (f32, f32)>,
        margin: f32,
    ) -> Self {
        for (lon, lat) in points {
            self.west = self.west.min(lon - margin);
            self.east = self.east.max(lon + margin);
            self.north = self.north.max(lat + margin);
            self.south = self.south.min(lat - margin);
        }
        self.west = self.west.max(-180.0);
        self.east = self.east.min(180.0);
        self.north = self.north.min(85.0);
        self.south = self.south.max(-85.0);
        self
    }
}

/// An equirectangular projection of a [`GeoWindow`] into a box,
/// keeping degrees square and centring the slack ("meet").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    pub window: GeoWindow,
    /// Pixels per degree.
    pub scale: f32,
    /// Where (west, north) lands.
    pub origin: Pt,
}

impl Projection {
    /// Fits `window` into the box at `(x, y)` of `width × height`.
    pub fn fit(
        window: GeoWindow,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Self {
        let scale = (width / window.width())
            .min(height / window.height())
            .max(0.0);
        let used_w = window.width() * scale;
        let used_h = window.height() * scale;
        Self {
            window,
            scale,
            origin: Pt::new(
                x + (width - used_w) / 2.0,
                y + (height - used_h) / 2.0,
            ),
        }
    }

    pub fn project(&self, lon: f32, lat: f32) -> Pt {
        Pt::new(
            self.origin.x + (lon - self.window.west) * self.scale,
            self.origin.y + (self.window.north - lat) * self.scale,
        )
    }

    /// Whether `(lon, lat)` falls inside the window.
    pub fn contains(&self, lon: f32, lat: f32) -> bool {
        lon >= self.window.west
            && lon <= self.window.east
            && lat <= self.window.north
            && lat >= self.window.south
    }

    /// How large the map is drawn relative to the designs' 1000-unit
    /// width, for stroke widths and dot sizes.
    pub fn zoom(&self) -> f32 {
        self.window.width() * self.scale / 1000.0
    }
}

/// How far an arc bows out, as a share of its chord (the designs use
/// 0.22).
pub const BOW: f32 = 0.22;

/// The control point of the quadratic arc from `from` to `to`: the
/// chord's midpoint pushed `BOW × chord` along its normal, towards the
/// top of the screen (north), so every arc bows the same way.
pub fn arc_control(from: Pt, to: Pt) -> Pt {
    let mid = Pt::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    // (dy, −dx) is the chord's normal with the chord's own length, so
    // BOW × normal is BOW × chord.
    let (mut nx, mut ny) = (dy * BOW, -dx * BOW);
    if ny > 0.0 || (ny == 0.0 && nx > 0.0) {
        nx = -nx;
        ny = -ny;
    }
    Pt::new(mid.x + nx, mid.y + ny)
}

/// A point of the quadratic Bézier `from → control → to` at `t`.
#[cfg(test)]
pub fn quad_point(from: Pt, control: Pt, to: Pt, t: f32) -> Pt {
    let u = 1.0 - t;
    Pt::new(
        u * u * from.x + 2.0 * u * t * control.x + t * t * to.x,
        u * u * from.y + 2.0 * u * t * control.y + t * t * to.y,
    )
}

/// Where a destination's label goes: to the right of the dot when the
/// destination is east of the origin, else to the left (right-aligned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSide {
    Right,
    Left,
}

pub fn label_side(origin: Pt, destination: Pt) -> LabelSide {
    if destination.x >= origin.x {
        LabelSide::Right
    } else {
        LabelSide::Left
    }
}

/// A screen rectangle, for keeping labels from overlapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }
}

/// Keeps the rectangles that do not overlap any earlier kept one, in
/// order (callers put the important labels first). Returns the indices
/// kept.
pub fn place_labels(rects: &[Rect], taken: &[Rect]) -> Vec<usize> {
    let mut placed: Vec<Rect> = taken.to_vec();
    let mut kept = Vec::new();
    for (i, rect) in rects.iter().enumerate() {
        if placed.iter().all(|p| !p.overlaps(rect)) {
            placed.push(*rect);
            kept.push(i);
        }
    }
    kept
}

/// A sparkline's points: `values` spread over `width`, the lowest value
/// at the bottom (`height − pad`) and the highest at `pad`.
pub fn sparkline(values: &[f64], width: f32, height: f32, pad: f32) -> Vec<Pt> {
    if values.is_empty() {
        return Vec::new();
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = hi - lo;
    let step = if values.len() > 1 {
        width / (values.len() - 1) as f32
    } else {
        0.0
    };
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let t = if span > 0.0 {
                ((v - lo) / span) as f32
            } else {
                0.5
            };
            Pt::new(i as f32 * step, pad + (1.0 - t) * (height - 2.0 * pad))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Pt, b: Pt, tol: f32) -> bool {
        (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol
    }

    /// The mockup's 1000 × 434 viewBox puts SAO at (284.6, 313.2) and
    /// LIS at (415.7, 95.5).
    #[test]
    fn matches_the_mockup_projection() {
        let p = Projection::fit(GeoWindow::DEFAULT, 0.0, 0.0, 1000.0, 434.0);
        let sao = p.project(-46.63, -23.55);
        let lis = p.project(-9.14, 38.72);
        let tyo = p.project(139.69, 35.68);
        assert!(close(sao, Pt::new(284.6, 313.2), 1.5), "{sao:?}");
        assert!(close(lis, Pt::new(415.7, 95.5), 1.5), "{lis:?}");
        assert!(close(tyo, Pt::new(936.0, 106.0), 1.5), "{tyo:?}");
        assert!((p.zoom() - 1.0).abs() < 0.01);
    }

    #[test]
    fn fitting_keeps_degrees_square_and_centres() {
        let p = Projection::fit(GeoWindow::DEFAULT, 10.0, 20.0, 2000.0, 434.0);
        // Height-bound: 434 / 124 = 3.5 px per degree.
        assert!((p.scale - 3.5).abs() < 1e-3);
        let used = GeoWindow::DEFAULT.width() * p.scale;
        assert!((p.origin.x - (10.0 + (2000.0 - used) / 2.0)).abs() < 1e-3);
        assert!((p.origin.y - 20.0).abs() < 1e-3);
        let nw = p.project(-128.0, 66.0);
        assert!(close(nw, p.origin, 1e-3));
    }

    #[test]
    fn including_grows_the_window() {
        let w = GeoWindow::DEFAULT.including([(-157.9, 21.3)], 4.0);
        assert!(w.west <= -161.9 && w.west >= -180.0);
        assert_eq!(w.east, GeoWindow::DEFAULT.east);
        let clamped = GeoWindow::DEFAULT.including([(179.0, 89.0)], 10.0);
        assert_eq!(clamped.east, 180.0);
        assert_eq!(clamped.north, 85.0);
    }

    /// Control points from the mockup: SAO → LIS bows to
    /// (302.3, 175.6), SAO → MIA to (263.8, 201.1), SAO → BUE to
    /// (255.4, 323.6).
    #[test]
    fn arcs_bow_north_like_the_mockup() {
        let sao = Pt::new(284.6, 313.2);
        let cases = [
            (Pt::new(415.7, 95.5), Pt::new(302.3, 175.6)),
            (Pt::new(167.1, 140.7), Pt::new(263.8, 201.1)),
            (Pt::new(243.4, 352.1), Pt::new(255.4, 323.6)),
            (Pt::new(936.0, 106.0), Pt::new(564.7, 66.3)),
        ];
        for (to, want) in cases {
            let got = arc_control(sao, to);
            assert!(close(got, want, 0.6), "{to:?}: {got:?} vs {want:?}");
        }
    }

    #[test]
    fn arc_control_is_above_the_chord_midpoint() {
        let from = Pt::new(100.0, 100.0);
        for to in [
            Pt::new(200.0, 100.0),
            Pt::new(0.0, 100.0),
            Pt::new(100.0, 300.0),
            Pt::new(250.0, -40.0),
        ] {
            let c = arc_control(from, to);
            let mid_y = (from.y + to.y) / 2.0;
            assert!(c.y <= mid_y, "{to:?} → {c:?}");
            let chord = from.distance(to);
            let mid = Pt::new((from.x + to.x) / 2.0, mid_y);
            assert!((mid.distance(c) - BOW * chord).abs() < 1e-3);
        }
    }

    #[test]
    fn bezier_ends_at_its_points() {
        let (a, c, b) =
            (Pt::new(0.0, 0.0), Pt::new(5.0, -5.0), Pt::new(10.0, 0.0));
        assert_eq!(quad_point(a, c, b, 0.0), a);
        assert_eq!(quad_point(a, c, b, 1.0), b);
        assert!(close(quad_point(a, c, b, 0.5), Pt::new(5.0, -2.5), 1e-4));
    }

    #[test]
    fn labels_face_away_from_the_origin() {
        let o = Pt::new(100.0, 100.0);
        assert_eq!(label_side(o, Pt::new(150.0, 0.0)), LabelSide::Right);
        assert_eq!(label_side(o, Pt::new(50.0, 0.0)), LabelSide::Left);
    }

    #[test]
    fn overlapping_labels_are_dropped() {
        let r = |x: f32, y: f32| Rect {
            x,
            y,
            w: 20.0,
            h: 10.0,
        };
        let kept =
            place_labels(&[r(0.0, 0.0), r(10.0, 5.0), r(30.0, 0.0)], &[]);
        assert_eq!(kept, vec![0, 2]);
        let kept = place_labels(&[r(0.0, 0.0)], &[r(5.0, 5.0)]);
        assert!(kept.is_empty());
    }

    #[test]
    fn sparkline_spans_the_box() {
        let pts = sparkline(&[3.0, 1.0, 2.0], 88.0, 26.0, 2.0);
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0], Pt::new(0.0, 2.0));
        assert_eq!(pts[1], Pt::new(44.0, 24.0));
        assert_eq!(pts[2].x, 88.0);
        assert!(sparkline(&[], 10.0, 10.0, 0.0).is_empty());
        let flat = sparkline(&[5.0, 5.0], 10.0, 10.0, 0.0);
        assert_eq!(flat[0].y, 5.0);
    }
}
