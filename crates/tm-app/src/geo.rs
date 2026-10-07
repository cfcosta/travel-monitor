//! The night map's geometry (DECISIONS.md 0044, 0073): the Robinson
//! projection fitted to a box, the user's zoom and pan over it, and the
//! curved arcs from the origin.
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

    #[cfg(test)]
    pub fn width(&self) -> f32 {
        self.east - self.west
    }

    #[cfg(test)]
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

/// Robinson's table (1974), every 5° of latitude from the equator to
/// the pole: the parallel's length relative to the equator's, and its
/// distance from the equator relative to the pole's.
const ROBINSON_X: [f32; 19] = [
    1.0000, 0.9986, 0.9954, 0.9900, 0.9822, 0.9730, 0.9600, 0.9427, 0.9216,
    0.8962, 0.8679, 0.8350, 0.7986, 0.7597, 0.7186, 0.6732, 0.6213, 0.5722,
    0.5322,
];
const ROBINSON_Y: [f32; 19] = [
    0.0000, 0.0620, 0.1240, 0.1860, 0.2480, 0.3100, 0.3720, 0.4340, 0.4958,
    0.5571, 0.6176, 0.6769, 0.7346, 0.7903, 0.8435, 0.8936, 0.9394, 0.9761,
    1.0000,
];

/// The Robinson projection of `(lon, lat)` in degrees onto a plane, x
/// east and y north, in units where a degree of longitude on the
/// equator is 0.8487 (the equator is about 305.5 long, pole to pole
/// about 155). Linear between the table's rows, which is closer than a
/// 2° dot grid can show.
pub fn robinson(lon: f32, lat: f32) -> Pt {
    let a = lat.abs().min(90.0) / 5.0;
    let i = (a.floor() as usize).min(17);
    let t = a - i as f32;
    let x = ROBINSON_X[i] + (ROBINSON_X[i + 1] - ROBINSON_X[i]) * t;
    let y = ROBINSON_Y[i] + (ROBINSON_Y[i + 1] - ROBINSON_Y[i]) * t;
    Pt::new(
        0.8487 * x * lon,
        1.3523 * y * 180.0 / std::f32::consts::PI * lat.signum(),
    )
}

/// A box on the projected plane: west/east x, north/south y.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub west: f32,
    pub east: f32,
    pub north: f32,
    pub south: f32,
}

impl Plane {
    /// The smallest box holding every point of `window`: its meridians
    /// are widest at the latitude nearest the equator.
    pub fn of(window: &GeoWindow) -> Self {
        let widest = 0.0_f32.clamp(window.south, window.north);
        Self {
            west: robinson(window.west, widest).x,
            east: robinson(window.east, widest).x,
            north: robinson(0.0, window.north).y,
            south: robinson(0.0, window.south).y,
        }
    }

    /// The land data's extent: the map never shows sky past it.
    pub fn globe() -> Self {
        Self::of(&GeoWindow {
            west: -180.0,
            east: 180.0,
            north: 84.0,
            south: -88.0,
        })
    }

    pub fn width(&self) -> f32 {
        self.east - self.west
    }

    pub fn height(&self) -> f32 {
        self.north - self.south
    }
}

/// The Robinson projection of a [`GeoWindow`] into a box, keeping the
/// plane's units square and centring the slack ("meet").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    pub window: GeoWindow,
    /// The window on the plane.
    pub plane: Plane,
    /// Pixels per plane unit.
    pub scale: f32,
    /// Where the plane's (west, north) lands.
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
        let plane = Plane::of(&window);
        let scale = (width / plane.width())
            .min(height / plane.height())
            .max(0.0);
        let used_w = plane.width() * scale;
        let used_h = plane.height() * scale;
        Self {
            window,
            plane,
            scale,
            origin: Pt::new(
                x + (width - used_w) / 2.0,
                y + (height - used_h) / 2.0,
            ),
        }
    }

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
        let globe = Plane::globe();
        // Vertically: the top of the box shows `north + (origin.y − y)/s`.
        let span = height / s;
        let top = self.plane.north + (self.origin.y - y) / s;
        let top = if span >= globe.height() {
            globe.north + (span - globe.height()) / 2.0
        } else {
            top.min(globe.north).max(globe.south + span)
        };
        self.origin.y = y + (top - self.plane.north) * s;
        // Horizontally: the left of the box shows `west − (origin.x − x)/s`.
        let span = width / s;
        let left = self.plane.west - (self.origin.x - x) / s;
        let left = if span >= globe.width() {
            globe.west - (span - globe.width()) / 2.0
        } else {
            left.max(globe.west).min(globe.east - span)
        };
        self.origin.x = x + (self.plane.west - left) * s;
        self
    }

    /// `view` applied to this fitted map in the box `(x, y, width,
    /// height)`, kept on the globe, with the view as it ends up once
    /// kept there (a pan past the globe's edge is taken back).
    pub fn viewed(
        self,
        view: View,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> (Self, View) {
        let c = Pt::new(x + width / 2.0, y + height / 2.0);
        let mut p = self;
        p.scale *= view.zoom;
        p.origin = Pt::new(
            c.x + (self.origin.x - c.x) * view.zoom + view.pan.x,
            c.y + (self.origin.y - c.y) * view.zoom + view.pan.y,
        );
        let kept = p.clamped_to_globe(x, y, width, height);
        let pan = Pt::new(
            view.pan.x + kept.origin.x - p.origin.x,
            view.pan.y + kept.origin.y - p.origin.y,
        );
        (
            kept,
            View {
                zoom: view.zoom,
                pan,
            },
        )
    }

    /// The least zoom that still fills the box `width × height` with
    /// globe along one axis: zooming out further only adds empty sky.
    pub fn min_zoom(&self, width: f32, height: f32) -> f32 {
        let globe = Plane::globe();
        if self.scale <= 0.0 {
            return 1.0;
        }
        (width / (globe.width() * self.scale))
            .max(height / (globe.height() * self.scale))
            .min(1.0)
    }

    pub fn project(&self, lon: f32, lat: f32) -> Pt {
        let p = robinson(lon, lat);
        Pt::new(
            self.origin.x + (p.x - self.plane.west) * self.scale,
            self.origin.y + (self.plane.north - p.y) * self.scale,
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
        self.plane.width() * self.scale / 1000.0
    }
}

/// The deepest zoom over the fitted map.
pub const MAX_ZOOM: f32 = 16.0;

/// The user's zoom and pan over the fitted map (0073): `zoom` around
/// the box's centre, then `pan` in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub zoom: f32,
    pub pan: Pt,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Pt::new(0.0, 0.0),
        }
    }
}

impl View {
    /// Zoomed by `factor` (within `min..=MAX_ZOOM`) keeping the point
    /// under the screen point `at` where it is; `centre` is the centre
    /// of the box the view applies in.
    pub fn zoomed_at(self, factor: f32, at: Pt, centre: Pt, min: f32) -> Self {
        let zoom = (self.zoom * factor).clamp(min.min(1.0), MAX_ZOOM);
        // The fitted map's point under `at`, before and after.
        let bx = centre.x + (at.x - self.pan.x - centre.x) / self.zoom;
        let by = centre.y + (at.y - self.pan.y - centre.y) / self.zoom;
        Self {
            zoom,
            pan: Pt::new(
                at.x - centre.x - (bx - centre.x) * zoom,
                at.y - centre.y - (by - centre.y) * zoom,
            ),
        }
    }

    /// Moved by `dx, dy` pixels.
    pub fn dragged(self, dx: f32, dy: f32) -> Self {
        Self {
            zoom: self.zoom,
            pan: Pt::new(self.pan.x + dx, self.pan.y + dy),
        }
    }
}

/// The great-circle distance between two points, in kilometres
/// (haversine on a sphere of the Earth's mean radius). Used to rank
/// trips by price per kilometre flown.
pub fn great_circle_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0088;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let h = (dp / 2.0).sin().powi(2)
        + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().asin()
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

    #[test]
    fn robinson_follows_its_table() {
        assert_eq!(robinson(0.0, 0.0), Pt::new(0.0, 0.0));
        // The equator 0.8487 × 360 long, the pole 1.3523 radians up.
        assert!((robinson(180.0, 0.0).x - 152.766).abs() < 1e-2);
        assert!((robinson(0.0, 90.0).y - 77.481).abs() < 1e-2);
        assert!((robinson(0.0, -90.0).y + 77.481).abs() < 1e-2);
        // Row 45°: X 0.8962, Y 0.5571.
        let p = robinson(100.0, 45.0);
        assert!((p.x - 0.8487 * 0.8962 * 100.0).abs() < 1e-3, "{p:?}");
        assert!((p.y - 1.3523 * 0.5571 * 57.29578).abs() < 1e-2, "{p:?}");
        // Halfway between rows is halfway between their values.
        let mid = robinson(10.0, 47.5).x;
        let want = 0.8487 * (0.8962 + 0.8679) / 2.0 * 10.0;
        assert!((mid - want).abs() < 1e-3);
        // Parallels shorten towards the poles; the map is about twice as
        // wide as it is tall.
        assert!(robinson(180.0, 60.0).x < robinson(180.0, 30.0).x);
        let globe = Plane::globe();
        let whole =
            robinson(180.0, 0.0).x * 2.0 / (robinson(0.0, 90.0).y * 2.0);
        assert!((whole - 1.97).abs() < 0.01, "{whole}");
        assert!(globe.width() > 0.0 && globe.height() > 0.0);
    }

    #[test]
    fn fitting_keeps_units_square_and_centres() {
        let w = GeoWindow::DEFAULT;
        let plane = Plane::of(&w);
        let p = Projection::fit(w, 10.0, 20.0, 2000.0, 434.0);
        // Height-bound.
        assert!((p.scale - 434.0 / plane.height()).abs() < 1e-3);
        let used = plane.width() * p.scale;
        assert!((p.origin.x - (10.0 + (2000.0 - used) / 2.0)).abs() < 1e-3);
        assert!((p.origin.y - 20.0).abs() < 1e-3);
        // The window's corners and places inside it land in the box.
        for (lon, lat) in [
            (w.west, w.north),
            (w.east, w.south),
            (-46.63, -23.55),
            (139.69, 35.68),
        ] {
            assert!(p.lands_in(lon, lat, 10.0, 20.0, 2000.0, 434.0));
        }
        // North is up and east is right.
        let sao = p.project(-46.63, -23.55);
        let lis = p.project(-9.14, 38.72);
        assert!(lis.x > sao.x && lis.y < sao.y);
    }

    /// Zooming keeps the point under the cursor in place, and a view
    /// pans no further than the globe's edge.
    #[test]
    fn views_zoom_at_the_cursor_and_stay_on_the_globe() {
        let w = GeoWindow {
            west: -90.0,
            east: 30.0,
            north: 50.0,
            south: -60.0,
        };
        let (x, y, bw, bh) = (0.0, 0.0, 1200.0, 800.0);
        let base =
            Projection::fit(w, x, y, bw, bh).clamped_to_globe(x, y, bw, bh);
        let centre = Pt::new(600.0, 400.0);
        let lis = base.project(-9.14, 38.72);
        let view = View::default().zoomed_at(3.0, lis, centre, 1.0);
        assert!((view.zoom - 3.0).abs() < 1e-6);
        let (zoomed, kept) = base.viewed(view, x, y, bw, bh);
        assert!(close(zoomed.project(-9.14, 38.72), lis, 1e-2));
        assert_eq!(kept, view, "inside the globe: nothing taken back");
        assert!((zoomed.scale - base.scale * 3.0).abs() < 1e-3);
        // Zoom is bounded both ways.
        let deep = View::default().zoomed_at(1000.0, lis, centre, 1.0);
        assert_eq!(deep.zoom, MAX_ZOOM);
        let out = View::default().zoomed_at(0.01, lis, centre, 0.5);
        assert_eq!(out.zoom, 0.5);
        // Dragged far east, the date line stops at the box's right edge.
        let (far, kept) = base.viewed(view.dragged(-1.0e6, 0.0), x, y, bw, bh);
        assert!((far.project(180.0, 0.0).x - bw).abs() < 1e-1);
        assert!(kept.pan.x > -1.0e6);
        // And the fitted view is the plain projection.
        let (same, _) = base.viewed(View::default(), x, y, bw, bh);
        assert_eq!(same, base);
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
        // Width-bound in a box 2000 px tall: more than the whole globe
        // shows vertically, so it is centred, with as much sky above as
        // below.
        let p = Projection::fit(w, 0.0, 0.0, 800.0, 2000.0)
            .clamped_to_globe(0.0, 0.0, 800.0, 2000.0);
        let top = p.project(0.0, 84.0).y;
        let bottom = p.project(0.0, -88.0).y;
        assert!(top > 0.0, "{top}");
        assert!((top - (2000.0 - bottom)).abs() < 1e-2, "{top} {bottom}");
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
        assert!(p.project(0.0, 84.0).y.abs() < 1e-2);
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

    #[test]
    fn great_circle_distances() {
        // São Paulo to Lisbon and to Salvador.
        let lis = great_circle_km(-23.55, -46.63, 38.72, -9.14);
        assert!((lis - 7948.59).abs() < 0.1, "{lis}");
        let ssa = great_circle_km(-23.55, -46.63, -12.97, -38.50);
        assert!((ssa - 1455.38).abs() < 0.1, "{ssa}");
        assert_eq!(great_circle_km(10.0, 20.0, 10.0, 20.0), 0.0);
    }
}
