//! The night map's geometry (DECISIONS.md 0044, 0074): an orthographic
//! globe that turns and zooms, arcs along great circles lifted off its
//! surface, and the label and sparkline helpers of the screens.
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

/// A direction from the Earth's centre: x towards (0°E, 0°N), y
/// towards (90°E, 0°N), z towards the north pole.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn from_lon_lat(lon: f32, lat: f32) -> Self {
        let (lon, lat) = (lon.to_radians(), lat.to_radians());
        Self {
            x: lat.cos() * lon.cos(),
            y: lat.cos() * lon.sin(),
            z: lat.sin(),
        }
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    fn scaled(self, k: f32) -> Self {
        Self {
            x: self.x * k,
            y: self.y * k,
            z: self.z * k,
        }
    }

    fn plus(self, o: Self) -> Self {
        Self {
            x: self.x + o.x,
            y: self.y + o.y,
            z: self.z + o.z,
        }
    }

    fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// This direction at unit length; `None` for (nearly) zero.
    fn normalized(self) -> Option<Self> {
        let l = self.length();
        (l > 1e-6).then(|| self.scaled(1.0 / l))
    }

    /// Longitude and latitude in degrees.
    pub fn lon_lat(self) -> (f32, f32) {
        (
            self.y.atan2(self.x).to_degrees(),
            self.z.clamp(-1.0, 1.0).asin().to_degrees(),
        )
    }
}

/// The least and most zoom over the fitted globe.
pub const MIN_ZOOM: f32 = 0.6;
pub const MAX_ZOOM: f32 = 12.0;

/// Where the globe faces and how large it is drawn (0074).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobeView {
    /// The point at the centre of the disc.
    pub lon: f32,
    pub lat: f32,
    /// Times the fitted size.
    pub zoom: f32,
}

impl GlobeView {
    /// Facing the middle of `points` (their mean direction), at the
    /// fitted size. The latitude stays within ±60° so the globe never
    /// opens on a pole.
    pub fn facing(points: impl IntoIterator<Item = (f32, f32)>) -> Self {
        let sum = points
            .into_iter()
            .map(|(lon, lat)| Vec3::from_lon_lat(lon, lat))
            .fold(
                Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                Vec3::plus,
            );
        let (lon, lat) = sum.normalized().map_or((0.0, 0.0), Vec3::lon_lat);
        Self {
            lon,
            lat: lat.clamp(-60.0, 60.0),
            zoom: 1.0,
        }
    }

    /// Turned by a drag of `dx, dy` pixels on a globe of `radius`
    /// pixels: the surface follows the pointer.
    pub fn rotated(self, dx: f32, dy: f32, radius: f32) -> Self {
        if radius <= 0.0 {
            return self;
        }
        let lon = self.lon - (dx / radius).to_degrees();
        Self {
            lon: (lon + 540.0).rem_euclid(360.0) - 180.0,
            lat: (self.lat + (dy / radius).to_degrees()).clamp(-85.0, 85.0),
            zoom: self.zoom,
        }
    }

    /// Zoomed by `factor`, within [`MIN_ZOOM`]..=[`MAX_ZOOM`].
    pub fn zoomed(self, factor: f32) -> Self {
        Self {
            zoom: (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM),
            ..self
        }
    }
}

/// An orthographic globe in a box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Globe {
    pub view: GlobeView,
    pub centre: Pt,
    /// The disc's radius in pixels.
    pub radius: f32,
}

impl Globe {
    /// The fitted globe fills 90% of the box's shorter side; the view's
    /// zoom scales that.
    pub fn fit(
        view: GlobeView,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Self {
        Self {
            view,
            centre: Pt::new(x + width / 2.0, y + height / 2.0),
            radius: (0.45 * width.min(height) * view.zoom).max(0.0),
        }
    }

    /// Where the direction `v`, raised `altitude` radii above the
    /// surface, lands on screen, and its depth: the cosine of its angle
    /// from the facing point, positive on the near side.
    pub fn place(&self, v: Vec3, altitude: f32) -> (Pt, f32) {
        let (l0, p0) = (self.view.lon.to_radians(), self.view.lat.to_radians());
        let facing = Vec3::from_lon_lat(self.view.lon, self.view.lat);
        let east = Vec3 {
            x: -l0.sin(),
            y: l0.cos(),
            z: 0.0,
        };
        let north = Vec3 {
            x: -p0.sin() * l0.cos(),
            y: -p0.sin() * l0.sin(),
            z: p0.cos(),
        };
        let k = self.radius * (1.0 + altitude);
        (
            Pt::new(
                self.centre.x + k * v.dot(east),
                self.centre.y - k * v.dot(north),
            ),
            v.dot(facing),
        )
    }

    /// Where `(lon, lat)` on the surface lands, or `None` on the far
    /// side.
    pub fn project(&self, lon: f32, lat: f32) -> Option<Pt> {
        let (p, depth) = self.place(Vec3::from_lon_lat(lon, lat), 0.0);
        (depth >= 0.0).then_some(p)
    }

    /// Whether a point placed at `p` with `depth` is in sight: on the
    /// near side, or raised beyond the disc's rim.
    pub fn in_sight(&self, p: Pt, depth: f32) -> bool {
        depth >= 0.0 || p.distance(self.centre) > self.radius
    }
}

/// `n + 1` points along the great circle from `a` to `b` (each
/// `(lon, lat)`), each with its altitude: lifted in the middle by
/// `lift` radii times the share of a half-turn the arc spans, so long
/// flights rise higher. Opposite points take any of their circles.
pub fn great_arc(
    a: (f32, f32),
    b: (f32, f32),
    n: usize,
    lift: f32,
) -> Vec<(Vec3, f32)> {
    let n = n.max(1);
    let va = Vec3::from_lon_lat(a.0, a.1);
    let vb = Vec3::from_lon_lat(b.0, b.1);
    let cos = va.dot(vb).clamp(-1.0, 1.0);
    let angle = cos.acos();
    // The unit direction from `a` towards `b` along the circle.
    let towards = vb
        .plus(va.scaled(-cos))
        .normalized()
        .or_else(|| {
            Vec3 {
                x: -va.y,
                y: va.x,
                z: 0.0,
            }
            .normalized()
            .or(Some(Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            }))
        })
        .unwrap_or(va);
    let height = lift * angle / std::f32::consts::PI;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let v = va
                .scaled((t * angle).cos())
                .plus(towards.scaled((t * angle).sin()));
            (v, height * (std::f32::consts::PI * t).sin())
        })
        .collect()
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
    fn directions_round_trip() {
        for (lon, lat) in [
            (0.0, 0.0),
            (-46.63, -23.55),
            (139.69, 35.68),
            (-150.0, 61.2),
        ] {
            let v = Vec3::from_lon_lat(lon, lat);
            assert!((v.length() - 1.0).abs() < 1e-5);
            let (lo, la) = v.lon_lat();
            assert!(
                (lo - lon).abs() < 1e-3 && (la - lat).abs() < 1e-3,
                "{lon} {lat}"
            );
        }
    }

    /// The facing point lands at the centre, north is up, east is
    /// right, and the far side is hidden.
    #[test]
    fn the_globe_faces_its_view() {
        let view = GlobeView {
            lon: -46.63,
            lat: -23.55,
            zoom: 1.0,
        };
        let g = Globe::fit(view, 0.0, 0.0, 1000.0, 600.0);
        assert!((g.radius - 270.0).abs() < 1e-3);
        let sao = g.project(-46.63, -23.55).expect("in front");
        assert!(close(sao, Pt::new(500.0, 300.0), 1e-2), "{sao:?}");
        let north = g.project(-46.63, 0.0).expect("in front");
        assert!(north.y < sao.y && (north.x - sao.x).abs() < 1e-2);
        let east = g.project(-30.0, -23.55).expect("in front");
        assert!(east.x > sao.x);
        // São Paulo's antipode, near Taiwan, is behind; Tokyo too.
        assert_eq!(g.project(133.37, 23.55), None);
        assert_eq!(g.project(139.69, 35.68), None);
        // Every point in front lands inside the disc.
        for (lon, lat) in [(-9.14, 38.72), (-80.0, 25.8), (18.4, -33.9)] {
            let p = g.project(lon, lat).expect("in front");
            assert!(p.distance(g.centre) <= g.radius + 1e-3);
        }
        // Raised past the rim, a point behind is still in sight.
        let (p, depth) = g.place(Vec3::from_lon_lat(133.37 - 95.0, 23.55), 0.3);
        assert!(depth < 0.0);
        assert!(g.in_sight(p, depth));
        // Zoom scales the disc.
        let near = Globe::fit(view.zoomed(2.0), 0.0, 0.0, 1000.0, 600.0);
        assert!((near.radius - 540.0).abs() < 1e-3);
    }

    #[test]
    fn views_face_the_middle_and_turn_with_a_drag() {
        let v = GlobeView::facing([(-10.0, 0.0), (10.0, 0.0)]);
        assert!(v.lon.abs() < 1e-3 && v.lat.abs() < 1e-3);
        assert_eq!(v.zoom, 1.0);
        // Never a pole, and something sensible with nothing to face.
        assert_eq!(GlobeView::facing([(0.0, 89.0)]).lat, 60.0);
        let none = GlobeView::facing(std::iter::empty());
        assert_eq!((none.lon, none.lat), (0.0, 0.0));
        // A drag right brings the west into view; down, the north.
        let r = 300.0;
        let turned = v.rotated(r * std::f32::consts::FRAC_PI_2, 0.0, r);
        assert!((turned.lon - -90.0).abs() < 1e-3, "{turned:?}");
        let up = v.rotated(0.0, 1000.0 * r, r);
        assert_eq!(up.lat, 85.0);
        // Longitude wraps.
        let wrapped = GlobeView { lon: 170.0, ..v }.rotated(-r * 0.5, 0.0, r);
        assert!(wrapped.lon < -150.0, "{wrapped:?}");
        // Zoom is bounded.
        assert_eq!(v.zoomed(1000.0).zoom, MAX_ZOOM);
        assert_eq!(v.zoomed(0.0).zoom, MIN_ZOOM);
    }

    #[test]
    fn arcs_follow_great_circles_and_rise_in_the_middle() {
        let sao = (-46.63, -23.55);
        let lis = (-9.14, 38.72);
        let arc = great_arc(sao, lis, 32, 0.4);
        assert_eq!(arc.len(), 33);
        let (first, h0) = arc[0];
        let (last, h1) = arc[32];
        assert!(first.dot(Vec3::from_lon_lat(sao.0, sao.1)) > 0.99999);
        assert!(last.dot(Vec3::from_lon_lat(lis.0, lis.1)) > 0.99999);
        assert!(h0.abs() < 1e-6 && h1.abs() < 1e-6);
        // On the sphere, evenly spaced, highest in the middle.
        let step = arc[0].0.dot(arc[1].0);
        for w in arc.windows(2) {
            assert!((w[0].0.length() - 1.0).abs() < 1e-4);
            assert!((w[0].0.dot(w[1].0) - step).abs() < 1e-4);
        }
        let top = arc.iter().map(|(_, h)| *h).fold(0.0, f32::max);
        assert!((arc[16].1 - top).abs() < 1e-6);
        // A longer flight rises higher.
        let tyo = great_arc(sao, (139.69, 35.68), 32, 0.4);
        assert!(tyo[16].1 > top);
        // Opposite points still make an arc of unit points.
        let opposite = great_arc((0.0, 0.0), (180.0, 0.0), 8, 0.4);
        assert!(
            opposite
                .iter()
                .all(|(v, _)| (v.length() - 1.0).abs() < 1e-4)
        );
        assert!(opposite[8].0.dot(Vec3::from_lon_lat(180.0, 0.0)) > 0.9999);
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
