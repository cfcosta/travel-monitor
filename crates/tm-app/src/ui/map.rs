//! The night map (DECISIONS.md 0044, 0073): land as dots on a Robinson
//! projection, an arc from the origin to every watched destination,
//! deals in yellow, the selected route brightest. A map given a
//! [`MapState`] zooms with the wheel (or a pinch) around the cursor,
//! pans when dragged, and fits again on a double click or when the
//! origin changes.

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
    geo::{self, GeoWindow, LabelSide, Projection, Pt, Rect, View},
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
    pub deal: bool,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct MapModel {
    pub origin: Option<MapPlace>,
    pub destinations: Vec<MapDest>,
    pub window: GeoWindow,
    /// Space kept free at the bottom (for the detail card).
    pub bottom_inset: f32,
    pub labels: bool,
}

impl MapModel {
    /// The box around the origin and every destination, padded so arcs
    /// and labels clear the edges; the designs' crop when there is
    /// nothing to show. The projection fits this box to the panel and
    /// the slack around it shows more of the globe, so the map fills
    /// any window size.
    pub fn fit_window(
        origin: Option<&MapPlace>,
        dests: &[MapDest],
    ) -> GeoWindow {
        GeoWindow::around(
            origin
                .into_iter()
                .chain(dests.iter().map(|d| &d.place))
                .map(|p| (p.lon, p.lat)),
            10.0,
            60.0,
            40.0,
        )
        .unwrap_or(GeoWindow::DEFAULT)
    }
}

/// A map's zoom and pan, kept by its screen across frames (0073).
#[derive(Clone, Default)]
pub struct MapState(Rc<RefCell<Interaction>>);

#[derive(Default)]
struct Interaction {
    view: View,
    /// The origin the view was made for: another origin fits again.
    origin: Option<SharedString>,
    /// A press on the map: where it started and where it was last seen,
    /// and whether it has moved far enough to be a drag.
    press: Option<(Pt, Pt, bool)>,
}

impl MapState {
    /// Sets the view, as if the user had zoomed and panned (screenshots).
    #[cfg(test)]
    pub fn set_view(&self, view: View) {
        self.0.borrow_mut().view = view;
    }
}

/// How far a press moves before it is a drag rather than a click.
const DRAG_SLOP: f32 = 4.0;

/// Where the map was last drawn: the projection on screen, the fitted
/// one under the view, and the box the view applies in.
#[derive(Clone, Copy)]
struct Drawn {
    proj: Projection,
    base: Projection,
    area: (f32, f32, f32, f32),
}

impl Drawn {
    fn centre(&self) -> Pt {
        let (x, y, w, h) = self.area;
        Pt::new(x + w / 2.0, y + h / 2.0)
    }

    fn min_zoom(&self) -> f32 {
        self.base.min_zoom(self.area.2, self.area.3)
    }
}

fn at(position: gpui::Point<Pixels>) -> Pt {
    Pt::new(f32::from(position.x), f32::from(position.y))
}

/// The map; `on_pick` gets the index of a destination clicked near its
/// dot. With `state`, it zooms and pans.
pub fn map(
    model: MapModel,
    fonts: Fonts,
    state: Option<MapState>,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let drawn: Rc<Cell<Option<Drawn>>> = Rc::new(Cell::new(None));
    let hits: Vec<(f32, f32)> = model
        .destinations
        .iter()
        .map(|d| (d.place.lon, d.place.lat))
        .collect();
    let pick = {
        let drawn = drawn.clone();
        move |click: Pt, window: &mut Window, cx: &mut App| {
            let Some(d) = drawn.get() else {
                return;
            };
            let nearest = hits
                .iter()
                .enumerate()
                .map(|(i, (lon, lat))| {
                    (i, d.proj.project(*lon, *lat).distance(click))
                })
                .filter(|(_, dist)| *dist <= 14.0)
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
                let view = state.as_ref().map(|s| {
                    let mut s = s.0.borrow_mut();
                    let origin = model.origin.as_ref().map(|o| o.code.clone());
                    if s.origin != origin {
                        s.origin = origin;
                        s.view = View::default();
                    }
                    s.view
                });
                let painted = window.with_content_mask(
                    Some(ContentMask { bounds }),
                    |window| {
                        paint_map(&model, &fonts, bounds, view, window, cx)
                    },
                );
                if let (Some(state), Some(kept)) = (&state, painted.view) {
                    state.0.borrow_mut().view = kept;
                }
                drawn.set(Some(painted.drawn));
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
    let (down, moving, up, out, wheel, pinch) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state,
    );
    let (wheel_drawn, pinch_drawn) = (drawn.clone(), drawn);
    let base = if dragging {
        base.cursor_grabbing()
    } else {
        base.cursor_grab()
    };
    base.on_mouse_down(MouseButton::Left, move |event, window, _| {
        let p = at(event.position);
        let mut s = down.0.borrow_mut();
        if event.click_count >= 2 {
            // A double click fits the routes again.
            s.view = View::default();
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
        let p = at(event.position);
        let mut s = moving.0.borrow_mut();
        let Some((start, last, moved)) = s.press else {
            return;
        };
        let moved = moved || start.distance(p) > DRAG_SLOP;
        if moved {
            s.view = s.view.dragged(p.x - last.x, p.y - last.y);
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
        let Some(d) = wheel_drawn.get() else {
            return;
        };
        let dy = match event.delta {
            gpui::ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
            gpui::ScrollDelta::Lines(l) => l.y,
        };
        if dy == 0.0 {
            return;
        }
        let mut s = wheel.0.borrow_mut();
        s.view = s.view.zoomed_at(
            1.2_f32.powf(dy),
            at(event.position),
            d.centre(),
            d.min_zoom(),
        );
        cx.stop_propagation();
        window.refresh();
    })
    .on_pinch(move |event, window, cx| {
        let Some(d) = pinch_drawn.get() else {
            return;
        };
        let mut s = pinch.0.borrow_mut();
        s.view = s.view.zoomed_at(
            1.0 + event.delta,
            at(event.position),
            d.centre(),
            d.min_zoom(),
        );
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

fn arc(window: &mut Window, from: Pt, to: Pt, width: f32, stroke: Hsla) {
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(pt(from));
    path.curve_to(pt(to), pt(geo::arc_control(from, to)));
    if let Ok(path) = path.build() {
        window.paint_path(path, stroke);
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

/// What painting left behind: where things were drawn, and the view as
/// it ended up once kept on the globe.
struct Painted {
    drawn: Drawn,
    view: Option<View>,
}

fn paint_map(
    model: &MapModel,
    fonts: &Fonts,
    bounds: Bounds<Pixels>,
    view: Option<View>,
    window: &mut Window,
    cx: &mut App,
) -> Painted {
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let inset = if h - model.bottom_inset > 160.0 {
        model.bottom_inset
    } else {
        0.0
    };
    let base = Projection::fit(
        model.window,
        x + 8.0,
        y + 8.0,
        w - 16.0,
        h - inset - 16.0,
    )
    .clamped_to_globe(x, y, w, h - inset);
    let area = (x, y, w, h - inset);
    let (proj, view) = match view {
        Some(v) => {
            let (p, kept) = base.viewed(v, x, y, w, h - inset);
            (p, Some(kept))
        }
        None => (base, None),
    };
    let painted = Painted {
        drawn: Drawn { proj, base, area },
        view,
    };
    // Strokes and labels follow the fitted size, not the zoom.
    let z = base.zoom().clamp(0.45, 2.5);

    // Land: a 2° grid of dots (about 1.7 plane units apart), about 44% of
    // the spacing wide, never blobs when zoomed in.
    let land = color(theme::LAND);
    let r = (proj.scale * 1.7 * 0.22).clamp(0.8, 4.0);
    // Every land dot that lands in the panel, not only those inside the
    // fitted window, so the dot field fills the slack with more globe
    // instead of stopping at a hard edge.
    for &(lon, lat) in LAND_DOTS {
        if proj.lands_in(lon, lat, x, y, w, h) {
            circle(window, proj.project(lon, lat), r, land);
        }
    }

    let Some(origin) = &model.origin else {
        return painted;
    };
    let o = proj.project(origin.lon, origin.lat);
    let dests: Vec<(Pt, &MapDest)> = model
        .destinations
        .iter()
        .map(|d| (proj.project(d.place.lon, d.place.lat), d))
        .collect();

    // Quiet arcs first, deals over them, the selected one on top.
    for (p, d) in dests.iter().filter(|(_, d)| !d.deal) {
        arc(window, o, *p, 0.9 * z, alpha(theme::ARC, 0.8));
        if !d.selected {
            circle(window, *p, 2.4 * z, color(theme::DOT));
        }
    }
    for (p, d) in dests.iter().filter(|(_, d)| d.deal) {
        arc(window, o, *p, 1.2 * z, alpha(theme::ARC_DEAL, 0.8));
        if !d.selected {
            circle(window, *p, 3.2 * z, color(theme::YELLOW));
        }
    }
    for (p, _) in dests.iter().filter(|(_, d)| d.selected) {
        let w = 2.4 * z;
        arc(window, o, *p, w + 7.0, alpha(theme::YELLOW, 0.08));
        arc(window, o, *p, w + 3.5, alpha(theme::YELLOW, 0.18));
        arc(window, o, *p, w, color(theme::YELLOW).into());
        ring(window, *p, 11.0 * z, alpha(theme::YELLOW, 0.5));
        circle(window, *p, 7.0 * z, alpha(theme::YELLOW, 0.25));
        circle(window, *p, 4.2 * z, color(theme::YELLOW));
    }

    // The origin.
    ring(window, o, 12.0 * z, alpha(theme::TEAL, 0.35));
    circle(window, o, 8.0 * z, alpha(theme::TEAL, 0.2));
    circle(window, o, 4.5 * z, color(theme::TEAL));

    if !model.labels {
        return painted;
    }
    let label_size = (10.0 * z).clamp(9.0, 14.0);
    let origin_label = Label {
        text: origin.code.clone(),
        at: Pt::new(o.x + 10.0 * z, o.y + 8.0 * z),
        side: LabelSide::Right,
        color: color(theme::TEAL).into(),
        size: (12.0 * z).clamp(10.0, 16.0),
        bold: true,
        family: fonts.sans.clone(),
    };
    // Selected and deals first, so they win a crowded spot.
    let mut order: Vec<&(Pt, &MapDest)> = dests.iter().collect();
    order.sort_by_key(|(_, d)| (!d.selected, !d.deal));
    let labels: Vec<Label> = order
        .iter()
        .map(|(p, d)| {
            let side = geo::label_side(o, *p);
            let dx = 8.0 * z;
            Label {
                text: d.place.code.clone(),
                at: match side {
                    LabelSide::Right => {
                        Pt::new(p.x + dx, p.y - label_size * 0.65)
                    }
                    LabelSide::Left => {
                        Pt::new(p.x - dx, p.y - label_size * 0.65)
                    }
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
    let shaped_origin = shape(&origin_label, window);
    let origin_rect = Rect {
        x: origin_label.at.x,
        y: origin_label.at.y,
        w: f32::from(shaped_origin.width),
        h: origin_label.size * 1.3,
    };
    let shaped: Vec<_> = labels.iter().map(|l| shape(l, window)).collect();
    let rects: Vec<Rect> = labels
        .iter()
        .zip(&shaped)
        .map(|(l, s)| {
            let w = f32::from(s.width);
            let x = match l.side {
                LabelSide::Right => l.at.x,
                LabelSide::Left => l.at.x - w,
            };
            Rect {
                x,
                y: l.at.y,
                w,
                h: l.size * 1.3,
            }
        })
        .collect();
    let kept = geo::place_labels(&rects, &[origin_rect]);
    for i in kept {
        let at = point(px(rects[i].x), px(rects[i].y));
        let _ = shaped[i].paint(
            at,
            px(labels[i].size * 1.3),
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    let _ = shaped_origin.paint(
        pt(origin_label.at),
        px(origin_label.size * 1.3),
        TextAlign::Left,
        None,
        window,
        cx,
    );
    painted
}
