//! The top bar: logo, sections, origin picker and the daemon's status.

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    SharedString,
    anchored,
    deferred,
    div,
    prelude::*,
    px,
};

use super::{Menu, Screen, TravelMonitor, widgets};
use crate::{
    data::{DaemonStatus, Live},
    format,
    theme::{self, Fonts, color},
};

/// The status chip: its dot's colour, its text, and whether a check is
/// running (the dot pulses). `checking SAO→LIS · economy · outbound ·
/// BR · 12 s`, `idle · next in 3 min · 212/1200 loads`, `paused for
/// login: smiles`, `stale · seen 5 min ago` (0057).
pub(crate) fn status_line(
    d: &DaemonStatus,
    now: DateTime<Utc>,
) -> (u32, String, bool) {
    let loads = format!("{}/{} loads", d.loads_today, d.budget);
    let (dot, mut parts, checking) = match d.live(now) {
        Live::Missing => {
            (theme::RED, vec!["daemon not seen".to_owned()], false)
        }
        Live::Stale(at) => (
            theme::YELLOW,
            vec![format!("stale · seen {}", format::ago(now, at)), loads],
            false,
        ),
        Live::Paused(state) => {
            (theme::YELLOW, vec![state.to_owned(), loads], false)
        }
        Live::Checking(c) => {
            let mut parts = vec![
                format!("checking {}", c.route),
                c.cabin.label().to_lowercase(),
                c.kind.clone(),
                c.pos.clone(),
            ];
            parts.extend(c.site.clone());
            parts.push(format::elapsed(now, c.started_at));
            (theme::TEAL, parts, true)
        }
        Live::Idle(next) => (
            theme::TEAL,
            vec![
                match next {
                    Some(at) => {
                        format!("idle · next in {}", format::countdown(now, at))
                    }
                    None => "idle".to_owned(),
                },
                loads,
            ],
            false,
        ),
    };
    if d.blocks_today > 0 {
        parts.push(format!(
            "{} block{}",
            d.blocks_today,
            if d.blocks_today == 1 { "" } else { "s" }
        ));
    }
    (dot, parts.join(" · "), checking)
}

impl TravelMonitor {
    pub(super) fn render_topbar(
        &self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> Div {
        let logo = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(widgets::dot(theme::TEAL, 11., true))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(17.))
                    .child("travel monitor"),
            );
        let nav =
            div()
                .flex()
                .gap(px(2.))
                .children(Screen::TABS.into_iter().map(|tab| {
                    let on = tab.label() == self.screen.label();
                    let item = div()
                        .id(SharedString::from(format!("nav-{}", tab.label())))
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .child(tab.label())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.navigate(tab, window, cx)
                        }));
                    if on {
                        item.bg(color(theme::NAV_ACTIVE))
                            .text_color(color(theme::TEXT))
                            .font_weight(FontWeight::MEDIUM)
                    } else {
                        item.text_color(color(theme::TEXT_MUTED))
                            .hover(|s| s.text_color(color(theme::TEXT)))
                    }
                }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(22.))
            .px(px(24.))
            .py(px(12.))
            .border_b_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::BAR))
            .child(logo)
            .child(nav)
            .child(
                div()
                    .ml_auto()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(self.render_origin_picker(cx))
                    .child(self.render_status(fonts)),
            )
    }

    fn render_origin_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = &self.snapshot else {
            return div().into_any_element();
        };
        let current = s
            .origin_place()
            .map(|p| format!("{} · {}", p.code, p.name))
            .unwrap_or_else(|| "No origin".to_owned());
        let options: Vec<(SharedString, bool)> = s
            .origins
            .iter()
            .map(|p| {
                (
                    format!("{} · {}", p.code, p.name).into(),
                    Some(p.id) == s.origin,
                )
            })
            .collect();
        let ids: Vec<_> = s.origins.iter().map(|p| p.id).collect();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(color(theme::TEXT_LABEL))
            .child("From")
            .child(self.dropdown(
                Menu::Origin,
                "origin",
                current.into(),
                options,
                move |this, i, cx| {
                    if let Some(id) = ids.get(i) {
                        this.set_origin(*id, cx);
                    }
                },
                cx,
            ))
            .into_any_element()
    }

    fn render_status(&self, fonts: &Fonts) -> Div {
        let Some(s) = &self.snapshot else {
            return div();
        };
        let (dot, text, checking) = status_line(&s.daemon, self.now);
        let dot = widgets::dot(dot, 7., true);
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .max_w(px(520.))
            .px(px(12.))
            .py(px(6.))
            .rounded(px(16.))
            .bg(color(theme::CARD))
            .border_1()
            .border_color(color(if checking {
                theme::BORDER_INPUT
            } else {
                theme::BORDER_CARD
            }))
            .font_family(fonts.mono.clone())
            .text_size(px(12.))
            .text_color(color(theme::TEXT_MUTED))
            .debug_selector(|| "daemon-status".into())
            .child(if checking {
                widgets::pulse("status-pulse", dot).into_any_element()
            } else {
                dot.into_any_element()
            })
            .child(div().min_w_0().truncate().child(text))
    }

    /// A select: a trigger showing `current`, and a menu of `options`
    /// while open.
    pub(super) fn dropdown(
        &self,
        menu: Menu,
        id: &'static str,
        current: SharedString,
        options: Vec<(SharedString, bool)>,
        on_pick: impl Fn(&mut Self, usize, &mut Context<Self>) + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.menu == Some(menu);
        let trigger = div()
            .id(SharedString::from(format!("{id}-trigger")))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(color(theme::CARD))
            .border_1()
            .border_color(color(if open {
                theme::TEAL
            } else {
                theme::BORDER_INPUT
            }))
            .text_color(color(theme::TEXT))
            .text_size(px(13.))
            .cursor_pointer()
            .child(div().truncate().child(current))
            .child(div().text_color(color(theme::TEXT_FAINT)).child("▾"))
            .on_click(
                cx.listener(move |this, _, _, cx| this.toggle_menu(menu, cx)),
            );
        let mut wrapper = div().relative().flex().flex_col().child(trigger);
        if open {
            let items =
                options.into_iter().enumerate().map(|(i, (label, on))| {
                    let pick = on_pick.clone();
                    div()
                        .id(SharedString::from(format!("{id}-option-{i}")))
                        .px(px(12.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .text_color(color(if on {
                            theme::TEXT
                        } else {
                            theme::TEXT_MUTED
                        }))
                        .when(on, |d| d.bg(color(theme::NAV_ACTIVE)))
                        .hover(|s| {
                            s.bg(color(theme::SELECTED))
                                .text_color(color(theme::TEXT))
                        })
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            pick(this, i, cx);
                            cx.notify();
                        }))
                });
            let list = div()
                .id(SharedString::from(format!("{id}-menu")))
                .occlude()
                .flex()
                .flex_col()
                .min_w(px(180.))
                .p(px(4.))
                .mt(px(4.))
                .rounded(px(10.))
                .bg(color(theme::CARD))
                .border_1()
                .border_color(color(theme::BORDER_INPUT))
                .shadow_lg()
                .children(items)
                .on_mouse_down_out(
                    cx.listener(|this, _, _, cx| this.close_menu(cx)),
                );
            wrapper = wrapper.child(
                div().absolute().top(px(34.)).left_0().child(
                    deferred(
                        anchored()
                            .snap_to_window_with_margin(px(8.))
                            .child(list),
                    )
                    .with_priority(1),
                ),
            );
        }
        wrapper
    }
}
