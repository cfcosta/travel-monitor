//! How a hand moves a mouse, as pure functions of a seeded RNG: the
//! path and timing of a movement, where in a box a click lands, how
//! long a button is held, the rhythm of wheel notches and of idle
//! wandering. [`crate::Mouse`] plays them on a pointer.
//!
//! - **Duration** follows Fitts's law, `a + b·log2(D/W + 1)` with
//!   a = 100 ms and b = 150 ms/bit, scaled by a random 0.85–1.2 and kept
//!   within 120 ms–1.6 s: far and small targets take longer.
//! - **Path**: a cubic Bézier whose control points sit 20–40% and
//!   60–80% along the line, bowed to one side by a normal amount
//!   (σ = 12% of the distance, at most 30% or 200 px). Moves over
//!   250 px overshoot by 3–8% (at most 40 px) seven times in ten and
//!   come back in a short correction of 100–220 ms.
//! - **Speed** rises and falls on a minimum-jerk profile
//!   (10t³ − 15t⁴ + 6t⁵) skewed so it peaks a little before halfway, as
//!   aimed movements do.
//! - **Samples** come at one rate per movement, 60–125 Hz, each ±12% of
//!   its interval, with a sub-pixel tremor (σ up to 0.35 px) that
//!   vanishes at both ends. Movements end exactly on their target.

use std::time::Duration;

use rand::{Rng, rngs::StdRng};
use serde::Deserialize;

/// A point on the pointer's surface, in its own pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    fn plus(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
}

/// A box, in the same pixels as [`Point`]: its top-left corner and
/// size, as `getBoundingClientRect` gives them.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }

    /// Whether `other` lies wholly inside this box.
    pub fn contains_rect(&self, other: &Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.width <= self.x + self.width
            && other.y + other.height <= self.y + self.height
    }

    pub fn centre(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// The target width for Fitts's law: the box's smaller side.
    fn size(&self) -> f64 {
        self.width.min(self.height)
    }
}

/// Where the pointer is at a moment of a movement, timed from its
/// start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waypoint {
    pub after: Duration,
    pub point: Point,
}

/// The fastest and slowest a single aimed movement takes.
pub const MOVE_DURATIONS: (Duration, Duration) =
    (Duration::from_millis(120), Duration::from_millis(1600));

/// The event rates a movement is sampled at, in Hz.
pub const SAMPLE_RATES: (f64, f64) = (60.0, 125.0);

/// The longest a button is held for a click.
pub const HOLD_DURATIONS: (Duration, Duration) =
    (Duration::from_millis(60), Duration::from_millis(140));

/// The target size assumed when a move has no box to aim at.
const DEFAULT_TARGET_SIZE: f64 = 30.0;

/// The path and timing of a movement from `from` to `to`, aimed at a
/// target `target_size` pixels across. The first waypoint is the first
/// step away from `from` (which the pointer is already at); the last is
/// `to`, exactly. Empty when there is nowhere to go.
pub fn trajectory(
    from: Point,
    to: Point,
    target_size: Option<f64>,
    rng: &mut StdRng,
) -> Vec<Waypoint> {
    let distance = from.distance(to);
    if distance < 0.5 {
        return Vec::new();
    }
    let size = target_size
        .filter(|s| *s > 0.0)
        .unwrap_or(DEFAULT_TARGET_SIZE);
    let duration = fitts_duration(distance, size, rng);
    let overshoots = distance > 250.0 && rng.random_bool(0.7);
    if !overshoots {
        return stroke(from, to, duration, rng);
    }
    // Past the target along the line, a little to one side, then back.
    let along = (distance * rng.random_range(0.03..0.08)).min(40.0);
    let (ux, uy) = ((to.x - from.x) / distance, (to.y - from.y) / distance);
    let aside = along * rng.random_range(-0.3..0.3);
    let past = to.plus(ux * along - uy * aside, uy * along + ux * aside);
    let mut path = stroke(from, past, duration.mul_f64(0.85), rng);
    let correction = Duration::from_millis(rng.random_range(100..=220));
    let pause = Duration::from_millis(rng.random_range(0..=40));
    let offset = path.last().map_or(Duration::ZERO, |w| w.after) + pause;
    path.extend(stroke(past, to, correction, rng).into_iter().map(|w| {
        Waypoint {
            after: offset + w.after,
            point: w.point,
        }
    }));
    path
}

/// Fitts's law with a person's day-to-day variation, within
/// [`MOVE_DURATIONS`].
pub fn fitts_duration(distance: f64, size: f64, rng: &mut StdRng) -> Duration {
    let bits = (distance / size.max(1.0) + 1.0).log2();
    let seconds = (0.100 + 0.150 * bits) * rng.random_range(0.85..1.2);
    Duration::from_secs_f64(seconds.clamp(
        MOVE_DURATIONS.0.as_secs_f64(),
        MOVE_DURATIONS.1.as_secs_f64(),
    ))
}

/// One curved, eased, sampled movement that ends exactly on `to`.
fn stroke(
    from: Point,
    to: Point,
    duration: Duration,
    rng: &mut StdRng,
) -> Vec<Waypoint> {
    let distance = from.distance(to);
    if distance < 0.5 {
        return Vec::new();
    }
    let (ux, uy) = ((to.x - from.x) / distance, (to.y - from.y) / distance);
    let bow = (normal(rng) * 0.12 * distance)
        .clamp(-0.3 * distance, 0.3 * distance)
        .clamp(-200.0, 200.0);
    let control = |along: f64, rng: &mut StdRng| {
        let side = bow * rng.random_range(0.6..1.2);
        from.plus(
            ux * along * distance - uy * side,
            uy * along * distance + ux * side,
        )
    };
    let c1 = control(rng.random_range(0.2..0.4), rng);
    let c2 = control(rng.random_range(0.6..0.8), rng);
    let skew = rng.random_range(0.85..1.0);
    let total = duration.as_secs_f64();
    let rate = rng.random_range(SAMPLE_RATES.0..SAMPLE_RATES.1);
    // A whole number of intervals, so the last one is not a sliver.
    let count = (total * rate).round().max(1.0);
    let interval = total / count;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a movement of at most a few seconds has a few hundred samples"
    )]
    let samples = count as usize;
    let tremor = 0.35 * rng.random_range(0.4..1.0);
    let mut path = Vec::with_capacity(samples);
    for i in 1..=samples {
        let last = i == samples;
        #[allow(clippy::cast_precision_loss, reason = "small counts")]
        let nominal = i as f64 * interval;
        let at = if last {
            total
        } else {
            (nominal + interval * rng.random_range(-0.12..0.12)).min(total)
        };
        let t = at / total;
        let point = if last {
            to
        } else {
            let p = bezier(from, c1, c2, to, minimum_jerk(t.powf(skew)));
            let shake = tremor * (std::f64::consts::PI * t).sin();
            p.plus(normal(rng) * shake, normal(rng) * shake)
        };
        path.push(Waypoint {
            after: Duration::from_secs_f64(at),
            point,
        });
    }
    path
}

/// Where a click lands in `area`: around its centre with a spread of a
/// sixth of each side, never in its outer tenth, and never dead centre.
pub fn click_point(area: Rect, rng: &mut StdRng) -> Point {
    let pick = |start: f64, extent: f64, rng: &mut StdRng| {
        let margin = extent * 0.1;
        let offset = normal(rng) * extent / 6.0;
        (start + extent / 2.0 + offset)
            .clamp(start + margin, start + extent - margin)
    };
    let mut point = Point::new(
        pick(area.x, area.width, rng),
        pick(area.y, area.height, rng),
    );
    if point == area.centre() {
        point.x += area.width * 0.05;
    }
    point
}

/// The target size of `area` for [`trajectory`].
pub fn target_size(area: Rect) -> f64 {
    area.size()
}

/// The pause between arriving on a target and pressing: 40–180 ms.
pub fn settle_pause(rng: &mut StdRng) -> Duration {
    Duration::from_millis(rng.random_range(40..=180))
}

/// How long a click holds the button, within [`HOLD_DURATIONS`].
pub fn hold_duration(rng: &mut StdRng) -> Duration {
    let (min, max) = HOLD_DURATIONS;
    #[allow(clippy::cast_possible_truncation, reason = "milliseconds")]
    Duration::from_millis(
        rng.random_range(min.as_millis() as u64..=max.as_millis() as u64),
    )
}

/// One wheel notch and the pause before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notch {
    pub pause: Duration,
    /// 1 down (or right), -1 up (or left).
    pub direction: i32,
}

/// The notches of turning a wheel `notches` times (negative: up), as a
/// finger does: bursts of one to four quick notches 25–90 ms apart,
/// with 180–650 ms between bursts.
pub fn wheel_notches(notches: i32, rng: &mut StdRng) -> Vec<Notch> {
    let direction = notches.signum();
    let mut left = notches.unsigned_abs();
    let mut out = Vec::with_capacity(left as usize);
    let mut first = true;
    while left > 0 {
        let burst = rng.random_range(1..=4).min(left);
        for i in 0..burst {
            let pause = if i > 0 {
                Duration::from_millis(rng.random_range(25..=90))
            } else if first {
                Duration::ZERO
            } else {
                Duration::from_millis(rng.random_range(180..=650))
            };
            first = false;
            out.push(Notch { pause, direction });
        }
        left -= burst;
    }
    out
}

/// One step of idle wandering while a page loads: a pause, then maybe
/// a small drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drift {
    pub pause: Duration,
    /// Where to drift to, or `None` to stay still this time.
    pub to: Option<Point>,
}

/// The next idle step from `at`, kept inside `bounds`: a 0.4–2.5 s
/// pause, then a 15–120 px drift (six times in ten), a 1–4 px nudge
/// (two in ten) or nothing.
pub fn drift(at: Point, bounds: Rect, rng: &mut StdRng) -> Drift {
    let pause = Duration::from_millis(rng.random_range(400..=2500));
    let roll: f64 = rng.random();
    let reach = if roll < 0.6 {
        rng.random_range(15.0..120.0)
    } else if roll < 0.8 {
        rng.random_range(1.0..4.0)
    } else {
        return Drift { pause, to: None };
    };
    let angle = rng.random_range(0.0..std::f64::consts::TAU);
    let inside = |value: f64, start: f64, extent: f64| {
        let margin = (extent * 0.05).min(20.0);
        value.clamp(
            start + margin,
            (start + extent - margin).max(start + margin),
        )
    };
    let to = Point::new(
        inside(at.x + reach * angle.cos(), bounds.x, bounds.width),
        inside(at.y + reach * angle.sin(), bounds.y, bounds.height),
    );
    Drift {
        pause,
        to: Some(to),
    }
}

fn bezier(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

/// Position along a minimum-jerk movement at time fraction `t`.
fn minimum_jerk(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (10.0 - 15.0 * t + 6.0 * t * t)
}

/// A standard normal sample (Box–Muller).
fn normal(rng: &mut StdRng) -> f64 {
    let u: f64 = 1.0 - rng.random::<f64>();
    let v: f64 = rng.random();
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;

    fn rng(seed: u64) -> StdRng {
        StdRng::seed_from_u64(seed)
    }

    /// Moves of every length and direction, from a fixed set of seeds.
    fn cases() -> impl Iterator<Item = (u64, Point, Point, f64)> {
        let targets = [
            (Point::new(700.0, 500.0), Point::new(710.0, 503.0), 20.0),
            (Point::new(700.0, 500.0), Point::new(820.0, 430.0), 30.0),
            (Point::new(20.0, 980.0), Point::new(1420.0, 30.0), 12.0),
            (Point::new(1300.0, 100.0), Point::new(200.0, 900.0), 80.0),
            (Point::new(400.0, 400.0), Point::new(400.0, 800.0), 40.0),
        ];
        (0..200).flat_map(move |seed| {
            targets.into_iter().map(move |(a, b, s)| (seed, a, b, s))
        })
    }

    #[test]
    fn a_movement_ends_exactly_on_its_target() {
        for (seed, from, to, size) in cases() {
            let path = trajectory(from, to, Some(size), &mut rng(seed));
            assert_eq!(path.last().map(|w| w.point), Some(to), "seed {seed}");
        }
    }

    #[test]
    fn time_only_moves_forward_at_a_mouse_rate() {
        for (seed, from, to, size) in cases() {
            let path = trajectory(from, to, Some(size), &mut rng(seed));
            let mut last = Duration::ZERO;
            for w in &path {
                let gap = w.after.saturating_sub(last);
                assert!(w.after > last, "seed {seed}: time went back");
                // 125 Hz less its jitter, up to 60 Hz plus it (and the
                // overshoot's short pause).
                assert!(
                    gap >= Duration::from_millis(5),
                    "seed {seed}: {gap:?} between events"
                );
                assert!(
                    gap <= Duration::from_millis(65),
                    "seed {seed}: {gap:?} between events"
                );
                last = w.after;
            }
        }
    }

    #[test]
    fn the_pointer_never_jumps() {
        for (seed, from, to, size) in cases() {
            let path = trajectory(from, to, Some(size), &mut rng(seed));
            let total = path.last().map_or(Duration::ZERO, |w| w.after);
            let average = from.distance(to) / total.as_secs_f64();
            let mut previous = (Duration::ZERO, from);
            for w in &path {
                let step = previous.1.distance(w.point);
                let dt = (w.after - previous.0).as_secs_f64();
                // A bell-shaped speed peaks under twice the average; a
                // bow and an overshoot add path, the tremor a pixel.
                assert!(
                    step <= 4.5 * average * dt + 2.0,
                    "seed {seed}: {step:.1} px in {dt:.3} s (average {average:.0} px/s)"
                );
                // A 1,650 px flick peaks near 5,000 px/s, under 100 px
                // an event at 60 Hz.
                assert!(step < 120.0, "seed {seed}: a {step:.0} px jump");
                previous = (w.after, w.point);
            }
        }
    }

    #[test]
    fn a_path_bends_but_stays_near_the_line() {
        for (seed, from, to, size) in cases() {
            let path = trajectory(from, to, Some(size), &mut rng(seed));
            let distance = from.distance(to);
            for w in &path {
                // Within the bow's bound plus the overshoot's.
                let off = w.point.distance(from) + w.point.distance(to);
                assert!(
                    off <= distance * 1.9 + 90.0,
                    "seed {seed}: strays to {:?}",
                    w.point
                );
            }
        }
    }

    #[test]
    fn far_and_small_targets_take_longer() {
        let mean = |distance: f64, size: f64| {
            let total: f64 = (0..100)
                .map(|seed| {
                    fitts_duration(distance, size, &mut rng(seed)).as_secs_f64()
                })
                .sum();
            total / 100.0
        };
        assert!(mean(800.0, 20.0) > mean(100.0, 20.0));
        assert!(mean(400.0, 10.0) > mean(400.0, 100.0));
        for (seed, from, to, size) in cases() {
            let d = fitts_duration(from.distance(to), size, &mut rng(seed));
            assert!(d >= MOVE_DURATIONS.0 && d <= MOVE_DURATIONS.1, "{d:?}");
            let path = trajectory(from, to, Some(size), &mut rng(seed));
            let total = path.last().map_or(Duration::ZERO, |w| w.after);
            // An overshoot adds its correction and pause.
            assert!(
                total >= MOVE_DURATIONS.0.mul_f64(0.85)
                    && total <= MOVE_DURATIONS.1 + Duration::from_millis(260),
                "seed {seed}: {total:?}"
            );
        }
    }

    #[test]
    fn long_moves_sometimes_overshoot_and_come_back() {
        let (from, to) = (Point::new(100.0, 500.0), Point::new(1300.0, 500.0));
        let overshot = (0..100)
            .filter(|&seed| {
                trajectory(from, to, Some(30.0), &mut rng(seed))
                    .iter()
                    .any(|w| w.point.x > to.x + 10.0)
            })
            .count();
        assert!((40..=95).contains(&overshot), "{overshot} of 100");
    }

    #[test]
    fn the_same_seed_moves_the_same_way() {
        let (from, to) = (Point::new(10.0, 10.0), Point::new(900.0, 600.0));
        let a = trajectory(from, to, Some(25.0), &mut rng(7));
        let b = trajectory(from, to, Some(25.0), &mut rng(7));
        let c = trajectory(from, to, Some(25.0), &mut rng(8));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(trajectory(from, from, None, &mut rng(1)).is_empty());
    }

    #[test]
    fn clicks_land_inside_but_off_centre() {
        let area = Rect::new(100.0, 200.0, 120.0, 40.0);
        let points: Vec<Point> = (0..500)
            .map(|seed| click_point(area, &mut rng(seed)))
            .collect();
        assert!(points.iter().all(|p| area.contains(*p)));
        assert!(points.iter().all(|p| *p != area.centre()));
        let inner = Rect::new(112.0, 204.0, 96.0, 32.0);
        assert!(
            points.iter().all(|p| inner.contains(*p)),
            "in the outer tenth"
        );
        let spread = points.iter().map(|p| p.x).fold(f64::NAN, f64::max)
            - points.iter().map(|p| p.x).fold(f64::NAN, f64::min);
        assert!(spread > 40.0, "clicks cluster: {spread}");
        for seed in 0..100 {
            let hold = hold_duration(&mut rng(seed));
            assert!(hold >= HOLD_DURATIONS.0 && hold <= HOLD_DURATIONS.1);
        }
    }

    #[test]
    fn the_wheel_turns_in_bursts_and_in_full() {
        for seed in 0..100 {
            let notches = wheel_notches(-9, &mut rng(seed));
            assert_eq!(notches.len(), 9);
            assert!(notches.iter().all(|n| n.direction == -1));
            assert_eq!(notches[0].pause, Duration::ZERO);
            assert!(notches.iter().skip(1).all(|n| {
                n.pause >= Duration::from_millis(25)
                    && n.pause <= Duration::from_millis(650)
            }));
        }
        assert!(wheel_notches(0, &mut rng(1)).is_empty());
        let bursts = (0..100)
            .map(|seed| {
                wheel_notches(12, &mut rng(seed))
                    .iter()
                    .filter(|n| n.pause >= Duration::from_millis(180))
                    .count()
            })
            .sum::<usize>();
        assert!(bursts > 100, "no pauses between bursts");
    }

    #[test]
    fn wandering_is_sparse_small_and_stays_inside() {
        let bounds = Rect::new(0.0, 80.0, 1440.0, 920.0);
        let mut at = Point::new(5.0, 85.0);
        let mut moves = 0;
        let mut r = rng(3);
        for _ in 0..500 {
            let step = drift(at, bounds, &mut r);
            assert!(step.pause >= Duration::from_millis(400));
            if let Some(to) = step.to {
                assert!(bounds.contains(to), "{to:?}");
                assert!(at.distance(to) <= 120.0);
                at = to;
                moves += 1;
            }
        }
        assert!((300..=450).contains(&moves), "{moves} moves of 500");
    }
}
