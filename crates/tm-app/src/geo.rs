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

impl GeoWindow {
    /// The smallest window holding every `(lon, lat)`, padded by
    /// `margin` degrees and grown around its centre to at least
    /// `min_width` × `min_height` degrees, clamped to the globe. `None`
    /// when there are no points.
    ///
    /// The map fits this box rather than a fixed crop, so the routes
    /// fill the panel at any window size and aspect ratio.
    pub fn around(
        points: impl IntoIterator<Item = (f32, f32)>,
        margin: f32,
        min_width: f32,
        min_height: f32,
    ) -> Option<Self> {
        let mut it = points.into_iter();
        let (lon, lat) = it.next()?;
        let mut w = GeoWindow {
            west: lon,
            east: lon,
            north: lat,
            south: lat,
        };
        for (lon, lat) in it {
            w.west = w.west.min(lon);
            w.east = w.east.max(lon);
            w.north = w.north.max(lat);
            w.south = w.south.min(lat);
        }
        w.west -= margin;
        w.east += margin;
        w.north += margin;
        w.south -= margin;
        let grow = |lo: &mut f32, hi: &mut f32, min: f32| {
            let extra = (min - (*hi - *lo)).max(0.0) / 2.0;
            *lo -= extra;
            *hi += extra;
        };
        grow(&mut w.west, &mut w.east, min_width);
        grow(&mut w.south, &mut w.north, min_height);
        w.west = w.west.max(-180.0);
        w.east = w.east.min(180.0);
        w.north = w.north.min(85.0);
        w.south = w.south.max(-85.0);
        Some(w)
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

    /// The land data's extent: the map never shows sky past these.
    pub const GLOBE_NORTH: f32 = 84.0;
    pub const GLOBE_SOUTH: f32 = -88.0;
    pub const GLOBE_WEST: f32 = -180.0;
    pub const GLOBE_EAST: f32 = 180.0;

    /// Slides the map inside the box `(x, y, width, height)` so its
    /// slack never shows beyond the globe: when the box shows more than
    /// the whole globe along an axis, the globe is centred on that axis;
    /// otherwise an edge past a pole (or the date line) is pulled back to
    /// it. The scale is kept, so the fitted window stays fully visible
    /// whenever it can.
    pub fn clamped_to_globe(
        mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Self {
        if self.scale <= 0.0 {
            return self;
        }
        let s = self.scale;
        // Latitude: the top of the box shows `north + (origin.y − y)/s`.
        let span = height / s;
        let globe = Self::GLOBE_NORTH - Self::GLOBE_SOUTH;
        let top = self.window.north + (self.origin.y - y) / s;
        let top = if span >= globe {
            Self::GLOBE_NORTH + (span - globe) / 2.0
        } else {
            top.min(Self::GLOBE_NORTH).max(Self::GLOBE_SOUTH + span)
        };
        self.origin.y = y + (top - self.window.north) * s;
        // Longitude: the left of the box shows `west − (origin.x − x)/s`.
        let span = width / s;
        let globe = Self::GLOBE_EAST - Self::GLOBE_WEST;
        let left = self.window.west - (self.origin.x - x) / s;
        let left = if span >= globe {
            Self::GLOBE_WEST - (span - globe) / 2.0
        } else {
            left.max(Self::GLOBE_WEST).min(Self::GLOBE_EAST - span)
        };
        self.origin.x = x + (self.window.west - left) * s;
        self
    }

    pub fn project(&self, lon: f32, lat: f32) -> Pt {
        Pt::new(
            self.origin.x + (lon - self.window.west) * self.scale,
            self.origin.y + (self.window.north - lat) * self.scale,
        )
    }

    /// Whether `(lon, lat)` lands inside the screen box
    /// `(x, y, width, height)`: the window is only what must fit; the
    /// slack around it shows more of the globe.
    pub fn lands_in(
        &self,
        lon: f32,
        lat: f32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> bool {
        let p = self.project(lon, lat);
        p.x >= x && p.x <= x + width && p.y >= y && p.y <= y + height
    }

    /// Whether `(lon, lat)` falls inside the window.
    #[cfg(test)]
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
    fn around_fits_the_points_with_a_minimum_size() {
        assert_eq!(GeoWindow::around(std::iter::empty(), 5.0, 1.0, 1.0), None);
        // SAO to Lisbon and Tokyo.
        let w = GeoWindow::around(
            [(-46.6, -23.5), (-9.1, 38.7), (139.7, 35.7)],
            8.0,
            60.0,
            40.0,
        )
        .expect("points");
        assert!((w.west - -54.6).abs() < 1e-3 && (w.east - 147.7).abs() < 1e-3);
        assert!(
            (w.north - 46.7).abs() < 1e-3 && (w.south - -31.5).abs() < 1e-3
        );
        // A single domestic route still gets a sensible box.
        let w = GeoWindow::around(
            [(-46.6, -23.5), (-37.1, -11.0)],
            4.0,
            60.0,
            40.0,
        )
        .expect("points");
        assert!((w.width() - 60.0).abs() < 1e-3);
        assert!((w.height() - 40.0).abs() < 1e-3);
    }

    #[test]
    fn a_tall_box_centres_the_globe_and_a_short_one_stops_at_the_poles() {
        let w = GeoWindow {
            west: -60.0,
            east: 20.0,
            north: 40.0,
            south: -30.0,
        };
        // 10 px per degree, 2000 px tall: 200° shown, more than the
        // globe's 172°, so it is centred: 14° of slack at each end.
        let p = Projection::fit(w, 0.0, 0.0, 800.0, 2000.0)
            .clamped_to_globe(0.0, 0.0, 800.0, 2000.0);
        let top = p.project(0.0, Projection::GLOBE_NORTH).y;
        let bottom = p.project(0.0, Projection::GLOBE_SOUTH).y;
        assert!((top - 140.0).abs() < 1e-3, "{top}");
        assert!((2000.0 - bottom - 140.0).abs() < 1e-3, "{bottom}");
        // A window near the pole in a box taller than it: the top edge
        // is pulled back to the data's north.
        let arctic = GeoWindow {
            west: -60.0,
            east: 20.0,
            north: 80.0,
            south: 40.0,
        };
        let p = Projection::fit(arctic, 0.0, 0.0, 800.0, 900.0)
            .clamped_to_globe(0.0, 0.0, 800.0, 900.0);
        assert!(p.project(0.0, Projection::GLOBE_NORTH).y.abs() < 1e-3);
        // And the window itself is still on screen.
        assert!(p.lands_in(-20.0, 60.0, 0.0, 0.0, 800.0, 900.0));
    }

    #[test]
    fn slack_shows_more_globe() {
        // A tall box: the window fits the width, and land above and
        // below it still lands on screen.
        let w = GeoWindow {
            west: -60.0,
            east: 20.0,
            north: 40.0,
            south: -30.0,
        };
        let p = Projection::fit(w, 0.0, 0.0, 800.0, 2000.0);
        assert!(!p.contains(-40.0, 60.0));
        assert!(p.lands_in(-40.0, 60.0, 0.0, 0.0, 800.0, 2000.0));
        assert!(!p.lands_in(-100.0, 0.0, 0.0, 0.0, 800.0, 2000.0));
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
