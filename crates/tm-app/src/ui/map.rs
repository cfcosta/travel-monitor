//! The night map (DECISIONS.md 0044): land as dots, an arc from the
//! origin to every watched destination, deals in yellow, the selected
//! route brightest.

use std::{cell::Cell, rc::Rc};

use gpui::{
    App,
    Bounds,
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
    geo::{self, GeoWindow, LabelSide, Projection, Pt, Rect},
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
    /// The default crop, grown to hold every place.
    pub fn fit_window(
        origin: Option<&MapPlace>,
        dests: &[MapDest],
    ) -> GeoWindow {
        GeoWindow::DEFAULT.including(
            origin
                .into_iter()
                .chain(dests.iter().map(|d| &d.place))
                .map(|p| (p.lon, p.lat)),
            6.0,
        )
    }
}

/// Where the map was last drawn, for clicks.
type Drawn = Rc<Cell<Option<(Projection, Bounds<Pixels>)>>>;

/// The map; `on_pick` gets the index of a destination clicked near its
/// dot.
pub fn map(
    model: MapModel,
    fonts: Fonts,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let drawn: Drawn = Rc::new(Cell::new(None));
    let painted = drawn.clone();
    let hits: Vec<(f32, f32)> = model
        .destinations
        .iter()
        .map(|d| (d.place.lon, d.place.lat))
        .collect();
    div()
        .size_full()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            let Some((proj, _)) = drawn.get() else {
                return;
            };
            let click = Pt::new(
                f32::from(event.position.x),
                f32::from(event.position.y),
            );
            let nearest = hits
                .iter()
                .enumerate()
                .map(|(i, (lon, lat))| {
                    (i, proj.project(*lon, *lat).distance(click))
                })
                .filter(|(_, d)| *d <= 14.0)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = nearest {
                on_pick(i, window, cx);
            }
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let proj = paint_map(&model, &fonts, bounds, window, cx);
                    painted.set(Some((proj, bounds)));
                },
            )
            .size_full(),
        )
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

fn paint_map(
    model: &MapModel,
    fonts: &Fonts,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Projection {
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let inset = if h - model.bottom_inset > 160.0 {
        model.bottom_inset
    } else {
        0.0
    };
    let proj = Projection::fit(
        model.window,
        x + 8.0,
        y + 8.0,
        w - 16.0,
        h - inset - 16.0,
    );
    let z = proj.zoom().clamp(0.45, 2.5);

    // Land: a 2° grid of dots, about 44% of the spacing wide.
    let land = color(theme::LAND);
    let r = (proj.scale * 2.0 * 0.22).max(0.8);
    for &(lon, lat) in LAND_DOTS {
        if proj.contains(lon, lat) {
            circle(window, proj.project(lon, lat), r, land);
        }
    }

    let Some(origin) = &model.origin else {
        return proj;
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
        return proj;
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
    proj
}
