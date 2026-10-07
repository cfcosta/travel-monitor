//! The night map (DECISIONS.md 0044, 0074): a globe of land dots, an
//! arc along the great circle from the origin to every watched
//! destination, lifted off the surface and coloured by its region
//! (0075, with a legend), deals brighter with a yellow dot, the
//! selected route in yellow. The globe faces the routes, or the selected one;
//! given a [`MapState`] it turns when dragged, zooms with the wheel or a
//! pinch, and faces the routes again on a double click.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{
    App,
    Bounds,
    ContentMask,
    FontWeight,
    Hsla,
    MouseButton,
    PathBuilder,
    Pixels,
    SharedString,
    TextAlign,
    TextRun,
    Window,
    canvas,
    div,
    fill,
    font,
    point,
    prelude::*,
    px,
    size,
};

use crate::{
    geo::{self, Globe, GlobeView, LabelSide, Pt, Rect, Region, Vec3},
    land_dots::LAND_DOTS,
    theme::{self, Fonts, alpha, color},
};

#[derive(Debug, Clone, PartialEq)]
pub struct MapPlace {
    pub code: SharedString,
    pub lon: f32,
    pub lat: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapDest {
    pub place: MapPlace,
    /// Which colour its arc takes (0075).
    pub region: Region,
    pub deal: bool,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct MapModel {
    pub origin: Option<MapPlace>,
    pub destinations: Vec<MapDest>,
    pub labels: bool,
}

impl MapModel {
    /// What the globe faces: the selected route's middle, else the
    /// middle of the origin and every destination.
    fn facing(&self) -> GlobeView {
        let middle = self.middle();
        GlobeView {
            lat: (middle.lat - TILT).max(-60.0),
            ..middle
        }
    }

    fn middle(&self) -> GlobeView {
        let origin = self.origin.iter().map(|o| (o.lon, o.lat));
        match self.destinations.iter().find(|d| d.selected) {
            Some(d) => {
                GlobeView::facing(origin.chain([(d.place.lon, d.place.lat)]))
            }
            None => GlobeView::facing(origin.chain(
                self.destinations.iter().map(|d| (d.place.lon, d.place.lat)),
            )),
        }
    }

    /// The origin and the selected destination, which decide where the
    /// globe faces.
    fn focus(&self) -> (Option<SharedString>, Option<SharedString>) {
        (
            self.origin.as_ref().map(|o| o.code.clone()),
            self.destinations
                .iter()
                .find(|d| d.selected)
                .map(|d| d.place.code.clone()),
        )
    }
}

/// A map's view, kept by its screen across frames (0074).
#[derive(Clone, Default)]
pub struct MapState(Rc<RefCell<Interaction>>);

#[derive(Default)]
struct Interaction {
    /// `None` until first drawn: the globe then faces the routes.
    view: Option<GlobeView>,
    /// The origin and selection the view was turned for: another one
    /// turns the globe to face it, keeping the zoom.
    focus: (Option<SharedString>, Option<SharedString>),
    /// A press on the map: where it started and was last seen, and
    /// whether it has moved far enough to be a drag.
    press: Option<(Pt, Pt, bool)>,
}

impl MapState {
    /// Sets the view, as if the user had turned and zoomed (screenshots).
    #[cfg(test)]
    pub fn set_view(&self, view: GlobeView) {
        self.0.borrow_mut().view = Some(view);
    }
}

/// How far a press moves before it is a drag rather than a click.
const DRAG_SLOP: f32 = 4.0;

/// How fast a drag turns the globe: at 1 the surface under the pointer
/// follows it; twice that crosses the world in fewer strokes.
const DRAG_SPEED: f32 = 2.0;

/// Points per arc.
const ARC_STEPS: usize = 48;

/// How high the longest arcs rise, in radii.
const ARC_LIFT: f32 = 0.22;

/// How far south of the routes' middle the globe faces: seen slightly
/// from below, the lifted arcs bow north instead of pointing straight
/// at the viewer.
const TILT: f32 = 18.0;

fn at(position: gpui::Point<Pixels>) -> Pt {
    Pt::new(f32::from(position.x), f32::from(position.y))
}

/// The map; `on_pick` gets the index of a destination clicked near its
/// dot. With `state`, it turns and zooms.
pub fn map(
    model: MapModel,
    fonts: Fonts,
    state: Option<MapState>,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let drawn: Rc<Cell<Option<Globe>>> = Rc::new(Cell::new(None));
    let hits: Vec<(f32, f32)> = model
        .destinations
        .iter()
        .map(|d| (d.place.lon, d.place.lat))
        .collect();
    let pick = {
        let drawn = drawn.clone();
        move |click: Pt, window: &mut Window, cx: &mut App| {
            let Some(globe) = drawn.get() else {
                return;
            };
            let nearest = hits
                .iter()
                .enumerate()
                .filter_map(|(i, (lon, lat))| {
                    Some((i, globe.project(*lon, *lat)?.distance(click)))
                })
                .filter(|(_, d)| *d <= 14.0)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = nearest {
                on_pick(i, window, cx);
            }
        }
    };
    let canvas = {
        let drawn = drawn.clone();
        let state = state.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let facing = model.facing();
                let view = match &state {
                    Some(state) => {
                        let mut s = state.0.borrow_mut();
                        let focus = model.focus();
                        let view = match s.view {
                            Some(v) if s.focus == focus => v,
                            Some(v) => GlobeView {
                                zoom: v.zoom,
                                ..facing
                            },
                            None => facing,
                        };
                        s.focus = focus;
                        s.view = Some(view);
                        view
                    }
                    None => facing,
                };
                let globe = window.with_content_mask(
                    Some(ContentMask { bounds }),
                    |window| {
                        paint_map(&model, &fonts, bounds, view, window, cx)
                    },
                );
                drawn.set(Some(globe));
            },
        )
        .size_full()
    };
    let base = div().size_full().overflow_hidden().child(canvas);
    let Some(state) = state else {
        return base
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                pick(at(event.position), window, cx);
            })
            .into_any_element();
    };
    let dragging = state.0.borrow().press.is_some_and(|(_, _, moved)| moved);
    let base = if dragging {
        base.cursor_grabbing()
    } else {
        base.cursor_grab()
    };
    let (down, moving, up, out, wheel, pinch) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state,
    );
    base.on_mouse_down(MouseButton::Left, move |event, window, _| {
        let p = at(event.position);
        let mut s = down.0.borrow_mut();
        if event.click_count >= 2 {
            // A double click faces the routes again, at the fitted size.
            s.view = None;
            s.press = None;
            window.refresh();
        } else {
            s.press = Some((p, p, false));
        }
    })
    .on_mouse_move(move |event, window, _| {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(globe) = drawn.get() else {
            return;
        };
        let p = at(event.position);
        let mut s = moving.0.borrow_mut();
        let Some((start, last, moved)) = s.press else {
            return;
        };
        if moved || start.distance(p) > DRAG_SLOP {
            s.view = Some(globe.view.rotated(
                (p.x - last.x) * DRAG_SPEED,
                (p.y - last.y) * DRAG_SPEED,
                globe.radius,
            ));
            s.press = Some((start, p, true));
            window.refresh();
        }
    })
    .on_mouse_up(MouseButton::Left, move |event, window, cx| {
        let press = up.0.borrow_mut().press.take();
        if let Some((_, _, false)) = press {
            pick(at(event.position), window, cx);
        }
        window.refresh();
    })
    .on_mouse_up_out(MouseButton::Left, move |_, window, _| {
        if out.0.borrow_mut().press.take().is_some() {
            window.refresh();
        }
    })
    .on_scroll_wheel(move |event, window, cx| {
        let dy = match event.delta {
            gpui::ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
            gpui::ScrollDelta::Lines(l) => l.y,
        };
        if dy == 0.0 {
            return;
        }
        let mut s = wheel.0.borrow_mut();
        if let Some(v) = s.view {
            s.view = Some(v.zoomed(1.2_f32.powf(dy)));
        }
        cx.stop_propagation();
        window.refresh();
    })
    .on_pinch(move |event, window, cx| {
        let mut s = pinch.0.borrow_mut();
        if let Some(v) = s.view {
            s.view = Some(v.zoomed(1.0 + event.delta));
        }
        cx.stop_propagation();
        window.refresh();
    })
    .into_any_element()
}

fn pt(p: Pt) -> gpui::Point<Pixels> {
    point(px(p.x), px(p.y))
}

fn circle(window: &mut Window, c: Pt, r: f32, background: impl Into<Hsla>) {
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(c.x - r), px(c.y - r)),
                size(px(2. * r), px(2. * r)),
            ),
            background.into(),
        )
        .corner_radii(px(r)),
    );
}

fn ring(window: &mut Window, c: Pt, r: f32, border: Hsla) {
    window.paint_quad(gpui::quad(
        Bounds::new(
            point(px(c.x - r), px(c.y - r)),
            size(px(2. * r), px(2. * r)),
        ),
        px(r),
        gpui::transparent_black(),
        px(1.),
        border,
        gpui::BorderStyle::Solid,
    ));
}

/// The arc from `from` to `to` along their great circle, lifted off the
/// surface, as the runs of screen points in sight (it may pass behind
/// the globe and come back).
fn arc_runs(globe: &Globe, from: &MapPlace, to: &MapPlace) -> Vec<Vec<Pt>> {
    let mut runs: Vec<Vec<Pt>> = Vec::new();
    let mut run: Vec<Pt> = Vec::new();
    for (v, altitude) in geo::great_arc(
        (from.lon, from.lat),
        (to.lon, to.lat),
        ARC_STEPS,
        ARC_LIFT,
    ) {
        let (p, depth) = globe.place(v, altitude);
        if globe.in_sight(p, depth) {
            run.push(p);
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    runs.push(run);
    runs.retain(|r| r.len() > 1);
    runs
}

fn stroke(window: &mut Window, runs: &[Vec<Pt>], width: f32, color: Hsla) {
    for run in runs {
        let mut path = PathBuilder::stroke(px(width));
        path.move_to(pt(run[0]));
        for p in &run[1..] {
            path.line_to(pt(*p));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

struct Label {
    text: SharedString,
    at: Pt,
    side: LabelSide,
    color: Hsla,
    size: f32,
    bold: bool,
    family: SharedString,
}

fn shape(label: &Label, window: &mut Window) -> gpui::ShapedLine {
    let mut f = font(label.family.clone());
    if label.bold {
        f.weight = FontWeight::BOLD;
    }
    let run = TextRun {
        len: label.text.len(),
        font: f,
        color: label.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(
        label.text.clone(),
        px(label.size),
        &[run],
        None,
    )
}

fn label_rect(label: &Label, shaped: &gpui::ShapedLine) -> Rect {
    let w = f32::from(shaped.width);
    Rect {
        x: match label.side {
            LabelSide::Right => label.at.x,
            LabelSide::Left => label.at.x - w,
        },
        y: label.at.y,
        w,
        h: label.size * 1.3,
    }
}

fn paint_map(
    model: &MapModel,
    fonts: &Fonts,
    bounds: Bounds<Pixels>,
    view: GlobeView,
    window: &mut Window,
    cx: &mut App,
) -> Globe {
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let globe = Globe::fit(view, x, y, w, h);
    let (c, r) = (globe.centre, globe.radius);
    // Strokes and labels follow the fitted size, not the zoom.
    let fitted = Globe::fit(GlobeView { zoom: 1.0, ..view }, x, y, w, h);
    let z = (fitted.radius / 300.0).clamp(0.45, 2.0);

    // The atmosphere: a soft glow around the rim, then the sea.
    for (grow, a) in [(16.0, 0.025), (10.0, 0.04), (5.0, 0.07), (2.0, 0.12)] {
        circle(window, c, r + grow * z, alpha(theme::ATMOSPHERE, a));
    }
    circle(window, c, r, color(theme::OCEAN));

    // Land: the 2° dots on the near side, fading towards the rim.
    let dot = (r * 2.0_f32.to_radians() * 0.24).clamp(0.5, 3.5);
    let (left, top) = (x - dot, y - dot);
    let (right, bottom) = (x + w + dot, y + h + dot);
    for &(lon, lat) in LAND_DOTS {
        // Meridians close in towards the poles: keep every n-th dot of
        // a row there, so the field stays even instead of ringed.
        let every = (1.0 / lat.to_radians().cos().max(0.05)).round() as i32;
        if ((lon + 180.0) / 2.0) as i32 % every.max(1) != 0 {
            continue;
        }
        let (p, depth) = globe.place(Vec3::from_lon_lat(lon, lat), 0.0);
        if depth <= 0.0
            || p.x < left
            || p.x > right
            || p.y < top
            || p.y > bottom
        {
            continue;
        }
        let shade = 0.2 + 0.8 * depth.sqrt();
        circle(window, p, dot, alpha(theme::LAND, shade));
    }
    ring(window, c, r, alpha(theme::ATMOSPHERE, 0.35));

    let Some(origin) = &model.origin else {
        return globe;
    };
    let o = globe.project(origin.lon, origin.lat);
    let dests: Vec<(Option<Pt>, &MapDest)> = model
        .destinations
        .iter()
        .map(|d| (globe.project(d.place.lon, d.place.lat), d))
        .collect();

    // Quiet arcs first, deals over them, the selected one on top. Each
    // takes its region's colour (0075); a deal is brighter and thicker,
    // with a yellow dot.
    for (p, d) in dests.iter().filter(|(_, d)| !d.deal && !d.selected) {
        let hue = theme::region(d.region);
        let runs = arc_runs(&globe, origin, &d.place);
        stroke(window, &runs, 0.9 * z, alpha(hue, 0.55));
        if let Some(p) = p {
            circle(window, *p, 2.4 * z, alpha(hue, 0.95));
        }
    }
    for (p, d) in dests.iter().filter(|(_, d)| d.deal && !d.selected) {
        let hue = theme::region(d.region);
        let runs = arc_runs(&globe, origin, &d.place);
        stroke(window, &runs, 1.5 * z, alpha(hue, 0.95));
        if let Some(p) = p {
            circle(window, *p, 3.2 * z, color(theme::YELLOW));
        }
    }
    for (p, d) in dests.iter().filter(|(_, d)| d.selected) {
        let runs = arc_runs(&globe, origin, &d.place);
        let w = 2.4 * z;
        stroke(window, &runs, w + 7.0, alpha(theme::YELLOW, 0.08));
        stroke(window, &runs, w + 3.5, alpha(theme::YELLOW, 0.18));
        stroke(window, &runs, w, color(theme::YELLOW).into());
        if let Some(p) = p {
            ring(window, *p, 11.0 * z, alpha(theme::YELLOW, 0.5));
            circle(window, *p, 7.0 * z, alpha(theme::YELLOW, 0.25));
            circle(window, *p, 4.2 * z, color(theme::YELLOW));
        }
    }

    // The origin.
    if let Some(o) = o {
        ring(window, o, 12.0 * z, alpha(theme::TEAL, 0.35));
        circle(window, o, 8.0 * z, alpha(theme::TEAL, 0.2));
        circle(window, o, 4.5 * z, color(theme::TEAL));
    }

    if !model.labels {
        return globe;
    }
    let label_size = (10.0 * z).clamp(9.0, 14.0);
    let origin_label = o.map(|o| Label {
        text: origin.code.clone(),
        at: Pt::new(o.x + 10.0 * z, o.y + 8.0 * z),
        side: LabelSide::Right,
        color: color(theme::TEAL).into(),
        size: (12.0 * z).clamp(10.0, 16.0),
        bold: true,
        family: fonts.sans.clone(),
    });
    // Labels face away from the origin, or from the globe's centre when
    // the origin is behind it. Selected and deals first, so they win a
    // crowded spot; only destinations on the near side get one.
    let from = o.unwrap_or(c);
    let mut order: Vec<(Pt, &MapDest)> = dests
        .iter()
        .filter_map(|(p, d)| Some(((*p)?, *d)))
        .collect();
    order.sort_by_key(|(_, d)| (!d.selected, !d.deal));
    let labels: Vec<Label> = order
        .iter()
        .map(|(p, d)| {
            let side = geo::label_side(from, *p);
            let dx = 8.0 * z;
            let ly = p.y - label_size * 0.65;
            Label {
                text: d.place.code.clone(),
                at: match side {
                    LabelSide::Right => Pt::new(p.x + dx, ly),
                    LabelSide::Left => Pt::new(p.x - dx, ly),
                },
                side,
                color: color(if d.selected {
                    theme::PRICE_DEAL
                } else if d.deal {
                    theme::LABEL_DEAL
                } else {
                    theme::LABEL
                })
                .into(),
                size: label_size,
                bold: d.selected,
                family: fonts.mono.clone(),
            }
        })
        .collect();
    let shaped_origin = origin_label.map(|l| {
        let s = shape(&l, window);
        (l, s)
    });
    let taken: Vec<Rect> = shaped_origin
        .iter()
        .map(|(l, s)| label_rect(l, s))
        .collect();
    let shaped: Vec<_> = labels.iter().map(|l| shape(l, window)).collect();
    let rects: Vec<Rect> = labels
        .iter()
        .zip(&shaped)
        .map(|(l, s)| label_rect(l, s))
        .collect();
    for i in geo::place_labels(&rects, &taken) {
        let at = point(px(rects[i].x), px(rects[i].y));
        let line = px(labels[i].size * 1.3);
        let _ = shaped[i].paint(at, line, TextAlign::Left, None, window, cx);
    }
    if let Some((l, s)) = shaped_origin {
        let line = px(l.size * 1.3);
        let _ = s.paint(pt(l.at), line, TextAlign::Left, None, window, cx);
    }
    paint_legend(model, fonts, (x, y, h), z, window, cx);
    globe
}

/// The regions on the map and their colours, in the bottom-left corner
/// of the box at `(x, y)` of height `h` (0075).
fn paint_legend(
    model: &MapModel,
    fonts: &Fonts,
    (x, y, h): (f32, f32, f32),
    z: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let mut regions: Vec<Region> =
        model.destinations.iter().map(|d| d.region).collect();
    regions.sort();
    regions.dedup();
    if regions.len() < 2 {
        return;
    }
    let size = (11.0 * z).clamp(10.0, 13.0);
    let line = size * 1.6;
    let left = x + 18.0;
    let mut row = y + h - 18.0 - line * regions.len() as f32;
    for region in regions {
        let hue = theme::region(region);
        let mid = row + line / 2.0;
        let mut path = PathBuilder::stroke(px(2.0));
        path.move_to(point(px(left), px(mid)));
        path.line_to(point(px(left + 16.0), px(mid)));
        if let Ok(path) = path.build() {
            window.paint_path(path, alpha(hue, 0.95));
        }
        let label = Label {
            text: region.label().into(),
            at: Pt::new(left + 24.0, mid - size * 0.65),
            side: LabelSide::Right,
            color: color(theme::TEXT_LABEL).into(),
            size,
            bold: false,
            family: fonts.sans.clone(),
        };
        let shaped = shape(&label, window);
        let _ = shaped.paint(
            pt(label.at),
            px(size * 1.3),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        row += line;
    }
}
