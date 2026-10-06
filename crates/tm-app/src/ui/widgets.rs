//! Small pieces every screen uses: chips, buttons, tabs, stat tiles,
//! sparklines.

use gpui::{
    AnyElement,
    Bounds,
    Div,
    ElementId,
    FontWeight,
    Hsla,
    PathBuilder,
    Pixels,
    SharedString,
    Stateful,
    canvas,
    div,
    point,
    prelude::*,
    px,
};

use crate::{
    data::RowStatus,
    format,
    geo,
    theme::{self, Fonts, alpha, color},
};

/// Text in the monospaced face.
pub fn mono(fonts: &Fonts, text: impl Into<SharedString>) -> Div {
    div().font_family(fonts.mono.clone()).child(text.into())
}

/// The list's change chip: yellow for deals, blue for other drops, grey
/// for rises.
pub fn change_chip(fonts: &Fonts, change: Option<f64>, deal: bool) -> Div {
    let (bg, fg) = match change {
        _ if deal => (theme::YELLOW, theme::ON_YELLOW),
        Some(c) if c < 0.0 => (theme::DROP_BG, theme::DROP),
        _ => (theme::FLAT_BG, theme::TEXT_LABEL),
    };
    mono(fonts, change.map_or_else(|| "–".to_owned(), format::change))
        .px(px(8.))
        .py(px(2.))
        .rounded(px(10.))
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .bg(color(bg))
        .text_color(color(fg))
}

/// `Deal`, `Confirming`, `Watching`.
pub fn status_chip(status: RowStatus) -> Div {
    let chip = div()
        .px(px(10.))
        .py(px(3.))
        .rounded(px(12.))
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .child(status.label());
    match status {
        RowStatus::Deal => chip
            .bg(color(theme::YELLOW))
            .text_color(color(theme::ON_YELLOW)),
        RowStatus::Confirming => chip
            .border_1()
            .border_color(color(theme::YELLOW))
            .text_color(color(theme::YELLOW)),
        RowStatus::Watching => chip
            .border_1()
            .border_color(color(theme::BORDER_BUTTON))
            .text_color(color(theme::TEXT_MUTED)),
    }
}

/// A plain pill: `7-day round trips`, `1 adult`.
pub fn pill(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(12.))
        .bg(color(theme::CARD))
        .border_1()
        .border_color(color(theme::BORDER_CARD))
        .text_size(px(12.))
        .text_color(color(theme::TEXT_SOFT))
        .child(text.into())
}

/// The yellow call to action.
pub fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(9.))
        .rounded(px(8.))
        .bg(color(theme::YELLOW))
        .text_color(color(theme::ON_YELLOW))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(label.into())
}

/// An outlined button.
pub fn ghost_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(9.))
        .rounded(px(8.))
        .border_1()
        .border_color(color(theme::BORDER_BUTTON))
        .text_color(color(theme::TEXT))
        .cursor_pointer()
        .hover(|s| s.bg(color(theme::CARD)))
        .child(label.into())
}

/// A small outlined button for table rows.
pub fn small_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(10.))
        .py(px(4.))
        .rounded(px(7.))
        .border_1()
        .border_color(color(theme::BORDER_BUTTON))
        .text_size(px(12.))
        .text_color(color(theme::TEXT))
        .cursor_pointer()
        .hover(|s| s.bg(color(theme::CARD)))
        .child(label.into())
}

/// The track of a segmented control.
pub fn segments() -> Div {
    div()
        .flex()
        .gap(px(4.))
        .p(px(3.))
        .rounded(px(9.))
        .bg(color(theme::CARD))
        .border_1()
        .border_color(color(theme::BORDER_CARD))
}

/// One segment; the selected one is light on dark.
pub fn segment(
    id: impl Into<ElementId>,
    label: impl IntoElement,
    on: bool,
) -> Stateful<Div> {
    let seg = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(12.))
        .py(px(6.))
        .rounded(px(7.))
        .cursor_pointer()
        .child(label);
    if on {
        seg.bg(color(theme::TEXT))
            .text_color(color(theme::GROUND))
            .font_weight(FontWeight::MEDIUM)
    } else {
        seg.text_color(color(theme::TEXT_MUTED))
            .hover(|s| s.text_color(color(theme::TEXT)))
    }
}

/// A stat tile: a label, a big value and a line under it.
pub fn stat_tile(
    fonts: &Fonts,
    label: impl Into<SharedString>,
    value: impl IntoElement,
    sub: impl Into<SharedString>,
    value_color: u32,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .bg(color(theme::CARD))
        .border_1()
        .border_color(color(theme::BORDER_CARD))
        .min_w_0()
        .child(
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_LABEL))
                .child(label.into()),
        )
        .child(
            div()
                .font_family(fonts.mono.clone())
                .text_size(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(color(value_color))
                .child(value),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(color(theme::TEXT_MUTED))
                .truncate()
                .child(sub.into()),
        )
}

/// A small round dot.
pub fn dot(hex: u32, size: f32, glow: bool) -> Div {
    let d = div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(color(hex));
    if glow {
        d.shadow(vec![gpui::BoxShadow {
            color: alpha(hex, 0.8),
            offset: point(px(0.), px(0.)),
            blur_radius: px(size * 1.2),
            spread_radius: px(0.),
            inset: false,
        }])
    } else {
        d
    }
}

/// A section heading.
pub fn heading(text: impl Into<SharedString>, size: f32) -> Div {
    div()
        .text_size(px(size))
        .font_weight(FontWeight::BOLD)
        .text_color(color(theme::TEXT))
        .child(text.into())
}

/// A small grey label.
pub fn label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(color(theme::TEXT_LABEL))
        .child(text.into())
}

/// A sparkline of `values` in a `width × height` box.
pub fn sparkline(
    values: Vec<f64>,
    width: f32,
    height: f32,
    stroke: u32,
) -> AnyElement {
    let line: Hsla = color(stroke).into();
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let pts = geo::sparkline(&values, width, height, 2.0);
            if pts.len() < 2 {
                return;
            }
            let origin = bounds.origin;
            let mut path = PathBuilder::stroke(px(1.5));
            for (i, p) in pts.iter().enumerate() {
                let at = point(origin.x + px(p.x), origin.y + px(p.y));
                if i == 0 {
                    path.move_to(at);
                } else {
                    path.line_to(at);
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, line);
            }
        },
    )
    .w(px(width))
    .h(px(height))
    .flex_none()
    .into_any_element()
}

/// The yellow bar on the left edge of a selected or deal row (the
/// designs' `inset 3px 0 0` shadow); the row must be `relative`.
pub fn accent() -> Div {
    div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(px(3.))
        .bg(color(theme::YELLOW))
}
