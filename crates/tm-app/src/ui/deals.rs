//! The main screen, "G · Map + list" (DECISIONS.md 0044): deals and
//! routes on the left, the night map on the right, and a card for the
//! selected row over the map.

use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    SharedString,
    Window,
    div,
    prelude::*,
    px,
    rgba,
};
use tm_domain::Cabin;

use super::{
    ListTab,
    Menu,
    Screen,
    Sort,
    TravelMonitor,
    map::{MapDest, MapModel, MapPlace, map},
    widgets::{self, mono},
};
use crate::{
    data::{RouteDetail, RouteRow},
    format,
    grid::{GridCell, TripGrid},
    theme::{self, Fonts, color},
};

const CABINS: [Option<Cabin>; 4] = [
    None,
    Some(Cabin::Economy),
    Some(Cabin::PremiumEconomy),
    Some(Cabin::Business),
];

fn cabin_label(cabin: Option<Cabin>) -> &'static str {
    cabin.map_or("All cabins", Cabin::label)
}

impl TravelMonitor {
    pub(super) fn render_deals(
        &mut self,
        fonts: &Fonts,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .size_full()
            .min_h_0()
            .child(self.render_list(fonts, cx))
            .child(self.render_map_pane(fonts, cx))
            .into_any_element()
    }

    fn render_list(&self, fonts: &Fonts, cx: &mut Context<Self>) -> Div {
        let Some(s) = &self.snapshot else {
            return div();
        };
        let deal_count = s.rows.iter().filter(|r| r.status.is_deal()).count();
        let tab = |id: &'static str,
                   label: &'static str,
                   count: usize,
                   which: ListTab| {
            widgets::segment(
                id,
                div()
                    .flex()
                    .gap(px(6.))
                    .child(label)
                    .child(mono(fonts, count.to_string()).text_size(px(12.))),
                self.list_tab == which,
            )
            .flex_1()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.list_tab = which;
                cx.notify();
            }))
        };
        let tabs = widgets::segments()
            .p(px(4.))
            .rounded(px(10.))
            .child(tab("tab-deals", "Deals", deal_count, ListTab::Deals))
            .child(tab("tab-all", "All routes", s.rows.len(), ListTab::All));
        let cabin_menu = self.dropdown(
            Menu::Cabin,
            "cabin",
            cabin_label(self.cabin).into(),
            CABINS
                .iter()
                .map(|c| (cabin_label(*c).into(), *c == self.cabin))
                .collect(),
            |this, i, cx| {
                this.cabin = CABINS[i];
                cx.notify();
            },
            cx,
        );
        let sort_menu = self.dropdown(
            Menu::Sort,
            "sort",
            self.sort.label().into(),
            Sort::ALL
                .iter()
                .map(|s| (s.label().into(), *s == self.sort))
                .collect(),
            |this, i, cx| {
                this.sort = Sort::ALL[i];
                cx.notify();
            },
            cx,
        );
        let field = |label: &'static str, control: Div| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(color(theme::TEXT_LABEL))
                .child(label)
                .child(control)
        };
        let rows = self.visible_rows();
        let empty = rows.is_empty().then(|| {
            div()
                .p(px(24.))
                .text_color(color(theme::TEXT_FAINT))
                .child(match self.list_tab {
                    ListTab::Deals => "No deals right now. Every watched route is listed under All routes.",
                    ListTab::All => "No routes match. Add destinations in Settings.",
                })
        });
        let items: Vec<_> = rows
            .iter()
            .map(|row| self.render_row(fonts, row, cx).into_any_element())
            .collect();
        div()
            .flex()
            .flex_col()
            .w(px(420.))
            .flex_none()
            .min_h_0()
            .border_r_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::PANEL))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(18.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .border_b_1()
                    .border_color(color(theme::BORDER))
                    .child(tabs)
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(field("Cabin", cabin_menu))
                            .child(field("Sort", sort_menu)),
                    ),
            )
            .child(
                div()
                    .id("rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(empty)
                    .children(items),
            )
    }

    fn render_row(
        &self,
        fonts: &Fonts,
        row: &RouteRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = row.key();
        let selected = self.selected == Some(key);
        let deal = row.status.is_deal();
        let dates = row
            .best
            .as_ref()
            .map(|t| format::date_range(t.departure, t.return_date))
            .unwrap_or_else(|| "no prices yet".to_owned());
        let price = row
            .best
            .as_ref()
            .map(|t| format::money(&t.price))
            .unwrap_or_else(|| "–".to_owned());
        let item = div()
            .id(SharedString::from(format!(
                "row-{}-{}",
                key.route,
                key.cabin.code()
            )))
            .debug_selector(move || {
                format!("row-{}-{}", key.route, key.cabin.code())
            })
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(18.))
            .py(px(12.))
            .border_b_1()
            .border_color(color(theme::ROW_RULE))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(row.code().to_owned()),
                            )
                            .child(
                                div()
                                    .text_color(color(theme::TEXT_MUTED))
                                    .truncate()
                                    .child(row.city().to_owned()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(color(theme::TEXT_FAINT))
                            .child(format!("{} · {dates}", row.cabin.label())),
                    ),
            )
            .child(widgets::sparkline(
                row.history.clone(),
                88.,
                26.,
                if deal { theme::YELLOW } else { theme::SPARK },
            ))
            .child(
                div()
                    .w(px(96.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(4.))
                    .child(
                        mono(fonts, price)
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(15.)),
                    )
                    .child(widgets::change_chip(fonts, row.change, deal)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(key, cx)));
        if selected {
            item.relative()
                .bg(color(theme::SELECTED))
                .child(widgets::accent())
        } else {
            item.hover(|s| s.bg(color(0x0B1324)))
        }
    }

    fn render_map_pane(&self, fonts: &Fonts, cx: &mut Context<Self>) -> Div {
        let Some(s) = &self.snapshot else {
            return div();
        };
        let origin = s.origin_place().map(|p| MapPlace {
            code: p.code.clone().into(),
            lon: p.lon as f32,
            lat: p.lat as f32,
        });
        // One dot per destination: a deal in any cabin makes it yellow.
        let mut dests: Vec<MapDest> = Vec::new();
        let mut keys = Vec::new();
        for row in &s.rows {
            let d = &row.route.destination;
            let selected = self.selected == Some(row.key());
            if let Some(i) =
                dests.iter().position(|m| m.place.code.as_ref() == d.code)
            {
                dests[i].deal |= row.status.is_deal();
                if selected {
                    dests[i].selected = true;
                    keys[i] = row.key();
                }
                continue;
            }
            dests.push(MapDest {
                place: MapPlace {
                    code: d.code.clone().into(),
                    lon: d.lon as f32,
                    lat: d.lat as f32,
                },
                deal: row.status.is_deal(),
                selected,
            });
            keys.push(row.key());
        }
        let window = MapModel::fit_window(origin.as_ref(), &dests);
        let model = MapModel {
            origin,
            destinations: dests,
            window,
            bottom_inset: 300.,
            labels: true,
        };
        let entity = cx.entity().downgrade();
        let picker = move |i: usize, _: &mut Window, cx: &mut gpui::App| {
            if let (Some(key), Some(view)) =
                (keys.get(i).copied(), entity.upgrade())
            {
                view.update(cx, |this, cx| this.select(key, cx));
            }
        };
        div()
            .flex_1()
            .min_w_0()
            .relative()
            .child(div().absolute().inset_0().child(map(
                model,
                fonts.clone(),
                picker,
            )))
            .children(self.render_card(fonts, cx))
    }

    fn render_card(
        &self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let s = self.snapshot.as_ref()?;
        let key = self.selected?;
        let row = s.row(key)?;
        let detail = s.detail.as_ref().filter(|d| d.key == key);
        let deal = row.status.is_deal();
        let trip = detail.and_then(|d| d.trip.as_ref()).or(row.best.as_ref());
        let origin = &row.route.origin.code;
        let summary = [
            Some(row.city().to_owned()),
            Some(row.cabin.label().to_owned()),
            trip.map(|t| format::date_range(t.departure, t.return_date)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let vs = row.change.map(|c| {
            format!("{} vs usual for this far out", format::change(c))
        });
        let first = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .flex_1()
            .min_w(px(220.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(26.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{origin} → {}", row.code())),
                    )
                    .child(widgets::status_chip(row.status)),
            )
            .child(div().text_color(color(theme::TEXT_MUTED)).child(summary))
            .children(
                trip.and_then(|t| t.carrier_label()).map(|c| {
                    div().text_color(color(theme::TEXT_MUTED)).child(c)
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap(px(10.))
                    .mt(px(4.))
                    .child(
                        mono(
                            fonts,
                            trip.map_or_else(
                                || "–".to_owned(),
                                |t| format::money(&t.price),
                            ),
                        )
                        .text_size(px(30.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(if deal {
                            theme::PRICE_DEAL
                        } else {
                            theme::TEXT
                        })),
                    )
                    .children(vs.map(|v| {
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(if deal {
                                theme::YELLOW
                            } else {
                                theme::TEXT_MUTED
                            }))
                            .child(v)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .mt(px(6.))
                    .child(
                        widgets::primary_button(
                            "open-gf",
                            "Open in Google Flights",
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| this.open_in_google_flights(cx),
                        )),
                    )
                    .child(
                        widgets::ghost_button("check-now", "Check now")
                            .on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.check_now(cx)
                                }),
                            ),
                    )
                    .child(
                        widgets::ghost_button("price-history", "Price history")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Screen::Detail, window, cx)
                            })),
                    ),
            );
        let grid = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .flex_1()
            .min_w(px(300.))
            .child(widgets::label("Departure × trip length"))
            .child(match detail {
                Some(d) if !d.grid.rows.is_empty() => trip_grid(fonts, &d.grid).into_any_element(),
                Some(_) => div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child("No leg prices yet.")
                    .into_any_element(),
                None => div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child("Loading…")
                    .into_any_element(),
            })
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child("Solid: round trip confirmed by a search. Dashed: estimated from the two legs."),
            );
        let sellers = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .flex_1()
            .min_w(px(260.))
            .child(widgets::label(
                "Cheapest seller by point of sale, effective BRL",
            ))
            .children(detail.map(|d| sellers_table(fonts, d, false)));
        Some(
            div()
                .absolute()
                .right(px(24.))
                .bottom(px(24.))
                .left(px(24.))
                .flex()
                .flex_wrap()
                .gap(px(20.))
                .px(px(20.))
                .py(px(18.))
                .rounded(px(16.))
                .bg(rgba(0x0C1222EB))
                .border_1()
                .border_color(color(theme::BORDER_INPUT))
                .shadow_lg()
                .debug_selector(|| "detail-card".into())
                .child(first)
                .child(grid)
                .child(sellers),
        )
    }
}

/// The departure × length grid of the detail card.
pub(super) fn trip_grid(fonts: &Fonts, grid: &TripGrid) -> Div {
    let header = div()
        .flex()
        .gap(px(3.))
        .text_color(color(theme::TEXT_FAINT))
        .child(div().w(px(54.)).flex_none())
        .children(
            grid.lengths
                .iter()
                .map(|n| div().flex_1().text_center().child(format!("{n}d"))),
        );
    let rows = grid.rows.iter().map(|row| {
        div()
            .flex()
            .gap(px(3.))
            .items_center()
            .child(
                div()
                    .w(px(54.))
                    .flex_none()
                    .text_color(color(theme::TEXT_MUTED))
                    .child(format::short_date(row.departure)),
            )
            .children(row.cells.iter().map(|cell| grid_cell(fonts, cell)))
    });
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .text_size(px(11.))
        .debug_selector(|| "trip-grid".into())
        .child(header)
        .children(rows)
}

fn grid_cell(fonts: &Fonts, cell: &GridCell) -> Div {
    let base = div()
        .flex_1()
        .py(px(4.))
        .rounded(px(4.))
        .text_center()
        .font_family(fonts.mono.clone())
        .text_size(px(11.));
    let Some(price) = cell.price else {
        return base.text_color(color(theme::TEXT_DIM)).child("–");
    };
    let text = format::kilo(&tm_domain::Money::new(
        price.minor,
        tm_domain::Currency::brl(),
    ));
    let mut c = if cell.best {
        base.bg(color(theme::YELLOW))
            .text_color(color(theme::ON_YELLOW))
            .font_weight(FontWeight::BOLD)
    } else {
        base.bg(color(theme::CELL))
            .text_color(color(theme::TEXT_SOFT))
    };
    if cell.selected {
        c = c.border_2().border_color(color(0xFFFFFF)).py(px(2.));
    } else if !price.confirmed {
        c = c
            .border_1()
            .border_dashed()
            .border_color(color(if cell.best {
                theme::ARC_DEAL
            } else {
                theme::BORDER_BUTTON
            }))
            .opacity(0.85)
            .py(px(3.));
    }
    c.child(text)
}

/// Sellers per point of sale, the cheapest in yellow, then the
/// airline's own price. `quoted` adds the seller's own-currency column
/// (the route detail).
pub(super) fn sellers_table(
    fonts: &Fonts,
    d: &RouteDetail,
    quoted: bool,
) -> Div {
    let cheapest = d.sellers.iter().map(|s| s.effective.minor).min();
    let line = || {
        div()
            .flex()
            .gap(px(10.))
            .py(px(7.))
            .border_t_1()
            .border_color(color(theme::BORDER))
            .text_size(px(13.))
    };
    let rows = d.sellers.iter().map(|s| {
        let best = Some(s.effective.minor) == cheapest;
        line()
            .child(
                mono(fonts, s.pos.as_str().to_owned())
                    .w(px(36.))
                    .flex_none()
                    .text_color(color(theme::TEXT_MUTED)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(theme::TEXT_SOFT))
                    .child(s.seller.clone()),
            )
            .when(quoted, |l| {
                l.child(
                    mono(
                        fonts,
                        s.quoted.as_ref().map_or_else(
                            || format::money(&s.effective),
                            format::money,
                        ),
                    )
                    .w(px(84.))
                    .flex_none()
                    .text_right()
                    .text_color(color(theme::TEXT_FAINT)),
                )
            })
            .child(
                mono(fonts, format::money(&s.effective))
                    .w(px(96.))
                    .flex_none()
                    .text_right()
                    .when(best, |p| {
                        p.text_color(color(theme::YELLOW))
                            .font_weight(FontWeight::BOLD)
                    }),
            )
    });
    let direct = d.airline_direct.as_ref().map(|(name, price)| {
        line()
            .text_color(color(theme::TEXT_LABEL))
            .child(div().w(px(36.)).flex_none())
            .child(div().flex_1().child(if quoted {
                format!("{name} direct")
            } else {
                "Airline direct".to_owned()
            }))
            .when(quoted, |l| l.child(div().w(px(84.)).flex_none()))
            .child(
                mono(fonts, format::money(price))
                    .w(px(96.))
                    .flex_none()
                    .text_right(),
            )
    });
    let empty = d.sellers.is_empty().then(|| {
        div()
            .text_size(px(12.))
            .text_color(color(theme::TEXT_FAINT))
            .child("Sellers are read when a deal is confirmed.")
    });
    div()
        .flex()
        .flex_col()
        .debug_selector(|| "sellers".into())
        .children(empty)
        .children(rows)
        .children(direct)
}
