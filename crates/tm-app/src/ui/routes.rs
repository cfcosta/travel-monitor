//! The routes overview ("H"): today's numbers, a small map, the activity
//! feed, and every route with its change against the usual price.

use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    SharedString,
    div,
    prelude::*,
    px,
};

use super::{
    RoutesFilter,
    Screen,
    Sort,
    TravelMonitor,
    map::{MapDest, MapModel, MapPlace, map},
    sort_rows,
    widgets::{self, mono},
};
use crate::{
    data::{NewPlace, Role, RouteRow, RowStatus, Snapshot, Tone},
    format,
    geo::Region,
    theme::{self, Fonts, color},
};

/// Column widths of the routes table.
const COLS: [f32; 7] = [100., 110., 170., 140., 140., 110., 110.];

/// Column widths of "Found by Explore": cabin, price, against its own
/// Explore usual, dates, airline, sweeps, the button.
const FOUND_COLS: [f32; 7] = [100., 110., 170., 140., 190., 120., 110.];

/// How many finds are listed; the rest are counted.
const FOUND_SHOWN: usize = 10;

pub(super) fn tone_color(tone: Tone) -> u32 {
    match tone {
        Tone::Deal => theme::YELLOW,
        Tone::System => theme::TEAL,
        Tone::Bad => theme::RED,
        Tone::Neutral => 0x3E5470,
    }
}

/// The change bar: a track with a centre line, drops growing left of
/// it, rises right; a 30% change fills one half.
pub(super) fn change_bar(
    fonts: &Fonts,
    change: Option<f64>,
    deal: bool,
) -> Div {
    let half = 50.0;
    let width = change.map_or(0.0, |c| {
        ((c.abs() / 0.30) * half as f64).min(half as f64) as f32
    });
    let bar = div()
        .absolute()
        .top_0()
        .h(px(8.))
        .rounded(px(4.))
        .w(px(width));
    let bar = match change {
        Some(c) if c < 0.0 => bar.right(px(half)).bg(color(if deal {
            theme::YELLOW
        } else {
            theme::BAR_DROP
        })),
        _ => bar.left(px(half)).bg(color(theme::BAR_RISE)),
    };
    let label_color = match change {
        _ if deal => theme::YELLOW,
        Some(c) if c < 0.0 => theme::DROP,
        _ => theme::TEXT_LABEL,
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .relative()
                .w(px(100.))
                .h(px(8.))
                .rounded(px(4.))
                .bg(color(theme::ROW_RULE))
                .child(
                    div()
                        .absolute()
                        .left(px(half))
                        .top(px(-3.))
                        .w(px(1.))
                        .h(px(14.))
                        .bg(color(theme::BORDER_BUTTON)),
                )
                .child(bar),
        )
        .child(
            mono(fonts, change.map_or_else(|| "–".to_owned(), format::change))
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_color(color(label_color)),
        )
}

pub(super) fn small_map(
    s: &Snapshot,
    selected: Option<&RouteRow>,
    fonts: &Fonts,
    labels: bool,
) -> impl IntoElement {
    let home = s.origin_place().map(|p| p.country.0.as_str());
    let origin = s.origin_place().map(|p| MapPlace {
        code: p.code.clone().into(),
        lon: p.lon as f32,
        lat: p.lat as f32,
    });
    let mut dests: Vec<MapDest> = Vec::new();
    for row in &s.rows {
        let d = &row.route.destination;
        let sel = selected.is_some_and(|r| r.key() == row.key());
        if let Some(m) =
            dests.iter_mut().find(|m| m.place.code.as_ref() == d.code)
        {
            m.deal |= row.status.is_deal();
            m.selected |= sel;
            continue;
        }
        dests.push(MapDest {
            place: MapPlace {
                code: d.code.clone().into(),
                lon: d.lon as f32,
                lat: d.lat as f32,
            },
            region: Region::of(&d.country.0, home),
            deal: row.status.is_deal(),
            selected: sel,
        });
    }
    map(
        MapModel {
            origin,
            destinations: dests,
            labels,
        },
        fonts.clone(),
        None,
        |_, _, _| {},
    )
}

impl TravelMonitor {
    pub(super) fn render_routes(
        &mut self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = &self.snapshot else {
            return div().into_any_element();
        };
        div()
            .id("routes-screen")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(self.render_today(fonts, s))
            .children(self.render_found(fonts, s, cx))
            .child(self.render_routes_table(fonts, s, cx))
            .into_any_element()
    }

    fn render_today(&self, fonts: &Fonts, s: &Snapshot) -> Div {
        let deals = s.rows.iter().filter(|r| r.status.is_deal()).count();
        let biggest = s
            .rows
            .iter()
            .filter(|r| r.status.is_deal())
            .filter_map(|r| Some((r.change?, r)))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let d = &s.daemon;
        let tiles = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Biggest drop",
                            biggest.map_or_else(
                                || "–".to_owned(),
                                |(c, _)| format::change(c),
                            ),
                            biggest
                                .and_then(|(_, r)| {
                                    Some(format!(
                                        "{} · {}",
                                        r.code(),
                                        format::money(&r.best.as_ref()?.price)
                                    ))
                                })
                                .unwrap_or_default(),
                            theme::YELLOW,
                        )
                        .flex_1(),
                    )
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Page loads",
                            div()
                                .flex()
                                .child(format::thousands(d.loads_today))
                                .child(
                                    div()
                                        .text_color(color(theme::TEXT_DIM))
                                        .child(format!(
                                            "/{}",
                                            format::thousands(d.budget)
                                        )),
                                ),
                            {
                                // Each site's own budget (0060).
                                let mut sub: Vec<String> = d
                                    .budgets()
                                    .filter_map(crate::data::SiteRow::loads)
                                    .collect();
                                sub.push(format!(
                                    "{} blocks today",
                                    d.blocks_today
                                ));
                                sub.join(" · ")
                            },
                            theme::TEXT,
                        )
                        .flex_1(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Explore sweep",
                            s.explore
                                .last_sweep
                                .map_or_else(|| "–".to_owned(), format::clock),
                            match s.explore.found.len() {
                                0 => format!(
                                    "{} destinations",
                                    s.explore.destinations
                                ),
                                n => format!(
                                    "{n} unwatched of {}",
                                    s.explore.destinations
                                ),
                            },
                            theme::TEXT,
                        )
                        .flex_1(),
                    )
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Next check",
                            d.next_check
                                .as_deref()
                                .and_then(|n| {
                                    n.rsplit_once(" in ")
                                        .map(|(_, t)| t.to_owned())
                                })
                                .unwrap_or_else(|| "–".to_owned()),
                            d.next_check
                                .as_deref()
                                .map(|n| {
                                    n.rsplit_once(" in ")
                                        .map_or(n, |(what, _)| what)
                                        .to_owned()
                                })
                                .unwrap_or_default(),
                            theme::TEXT,
                        )
                        .flex_1(),
                    ),
            );
        let headline = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .text_size(px(30.))
            .font_weight(FontWeight::BOLD)
            .line_height(px(34.))
            .child(div().text_color(color(theme::YELLOW)).child(format!(
                "{deals} deal{}",
                if deals == 1 { "" } else { "s" }
            )))
            .child(format!("across {} routes", s.rows.len()));
        let feed = s.feed.iter().take(8).map(|f| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .py(px(6.))
                .child(
                    mono(fonts, format::clock(f.at))
                        .w(px(46.))
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_FAINT)),
                )
                .child(div().mt(px(4.)).child(widgets::dot(
                    tone_color(f.tone),
                    9.,
                    false,
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_color(color(theme::TEXT_SOFT))
                        .child(f.text.clone()),
                )
        });
        div()
            .flex()
            .flex_none()
            .border_b_1()
            .border_color(color(theme::BORDER))
            .child(
                div()
                    .w(px(380.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .px(px(24.))
                    .py(px(22.))
                    .child(headline)
                    .child(tiles),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(340.))
                    .child(small_map(s, None, fonts, true)),
            )
            .child(
                div()
                    .w(px(380.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .px(px(24.))
                    .py(px(22.))
                    .child(widgets::heading("Activity", 14.))
                    .children(feed),
            )
    }

    /// Destinations the Explore sweep priced that nobody watches
    /// (0087), drops under their own Explore history first, each with a
    /// button that adds it to the watchlist.
    fn render_found(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let found = &s.explore.found;
        if found.is_empty() {
            return None;
        }
        let threshold = s.settings.deal_threshold;
        let now = self.now;
        let head =
            |text: &'static str, w: f32| div().w(px(w)).flex_none().child(text);
        let header = div()
            .flex()
            .gap(px(12.))
            .px(px(16.))
            .py(px(10.))
            .text_size(px(12.))
            .text_color(color(theme::TEXT_LABEL))
            .border_b_1()
            .border_color(color(theme::BORDER))
            .child(div().flex_1().min_w(px(180.)).child("Destination"))
            .child(head("Cabin", FOUND_COLS[0]))
            .child(head("Cheapest today", FOUND_COLS[1]).text_right())
            .child(head("vs its Explore usual", FOUND_COLS[2]))
            .child(head("Dates", FOUND_COLS[3]))
            .child(head("Airline", FOUND_COLS[4]))
            .child(head("Sweeps", FOUND_COLS[5]))
            .child(head("", FOUND_COLS[6]));
        let body = found.iter().take(FOUND_SHOWN).map(|r| {
            let drop = r.is_drop(threshold);
            div()
                .id(SharedString::from(format!(
                    "found-{}-{}",
                    r.code,
                    r.cabin.code()
                )))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(16.))
                .py(px(9.))
                .border_b_1()
                .border_color(color(theme::ROW_RULE_FAINT))
                .relative()
                .when(drop, |d| {
                    d.bg(color(theme::DEAL_ROW)).child(widgets::accent())
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(px(180.))
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(15.))
                                .child(r.code.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_color(color(theme::TEXT_MUTED))
                                .child(match &r.country {
                                    Some(c) => format!("{}, {c}", r.name),
                                    None => r.name.clone(),
                                }),
                        ),
                )
                .child(
                    div()
                        .w(px(FOUND_COLS[0]))
                        .flex_none()
                        .text_color(color(theme::TEXT_MUTED))
                        .child(r.cabin.label()),
                )
                .child(
                    mono(fonts, format::money(&r.price))
                        .w(px(FOUND_COLS[1]))
                        .flex_none()
                        .text_right()
                        .font_weight(FontWeight::BOLD),
                )
                .child(
                    div().w(px(FOUND_COLS[2])).flex_none().child(
                        match &r.usual {
                            Some(_) => change_bar(fonts, r.change, drop)
                                .into_any_element(),
                            None => div()
                                .text_size(px(12.))
                                .text_color(color(theme::TEXT_FAINT))
                                .child("new, no history yet")
                                .into_any_element(),
                        },
                    ),
                )
                .child(
                    div()
                        .w(px(FOUND_COLS[3]))
                        .flex_none()
                        .text_color(color(theme::TEXT_SOFT))
                        .child(match (r.departure, r.return_date) {
                            (Some(d), Some(ret)) => format::date_range(d, ret),
                            _ => "–".to_owned(),
                        }),
                )
                .child(
                    div()
                        .w(px(FOUND_COLS[4]))
                        .flex_none()
                        .truncate()
                        .text_color(color(theme::TEXT_SOFT))
                        .child(r.airline_label().unwrap_or_else(|| "–".into())),
                )
                .child(
                    div()
                        .w(px(FOUND_COLS[5]))
                        .flex_none()
                        .text_color(color(theme::TEXT_FAINT))
                        .child(format!(
                            "{} · {}",
                            r.sweeps,
                            format::ago(now, r.seen_at)
                        )),
                )
                .child(
                    div().w(px(FOUND_COLS[6])).flex_none().flex().child(
                        match r.watch.clone() {
                            Some(place) => widgets::ghost_button(
                                SharedString::from(format!(
                                    "watch-{}-{}",
                                    r.code,
                                    r.cabin.code()
                                )),
                                "Watch",
                            )
                            .debug_selector({
                                let selector = format!(
                                    "watch-{}-{}",
                                    r.code,
                                    r.cabin.code()
                                );
                                move || selector
                            })
                            .py(px(5.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.watch_found(place.clone(), cx)
                            }))
                            .into_any_element(),
                            None => div()
                                .text_size(px(12.))
                                .text_color(color(theme::TEXT_FAINT))
                                .child("add in Settings")
                                .into_any_element(),
                        },
                    ),
                )
        });
        let more = found.len().saturating_sub(FOUND_SHOWN);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .px(px(24.))
                .pt(px(18.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(10.))
                        .child(widgets::heading("Found by Explore", 18.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(color(theme::TEXT_MUTED))
                                .child(
                                    "Priced by the Explore sweep, not on the \
                                     watchlist",
                                ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "found-by-explore".into())
                        .flex()
                        .flex_col()
                        .border_1()
                        .border_color(color(theme::BORDER))
                        .rounded(px(12.))
                        .overflow_hidden()
                        .child(header)
                        .children(body)
                        .when(more > 0, |d| {
                            d.child(
                                div()
                                    .px(px(16.))
                                    .py(px(9.))
                                    .text_size(px(12.))
                                    .text_color(color(theme::TEXT_FAINT))
                                    .child(format!("{more} more")),
                            )
                        }),
                ),
        )
    }

    /// "Watch" on a find: adds it as a destination, as Settings would
    /// (0035), and asks the daemon to reload.
    pub(super) fn watch_found(
        &mut self,
        place: NewPlace,
        cx: &mut Context<Self>,
    ) {
        let backend = self.env.backend.clone();
        self.write(
            async move { backend.add_place(Role::Destination, place).await },
            "Watching it now",
            cx,
        );
    }

    fn render_routes_table(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let filter = self.routes_filter;
        let mut rows: Vec<&RouteRow> = s
            .rows
            .iter()
            .filter(|r| match filter {
                RoutesFilter::All => true,
                RoutesFilter::Deals => r.status.is_deal(),
                RoutesFilter::Domestic => r.route.is_domestic(),
                RoutesFilter::International => !r.route.is_domestic(),
            })
            .collect();
        sort_rows(&mut rows, Sort::BiggestDrop, super::PayWith::Cash);
        let filters = widgets::segments().children(
            RoutesFilter::ALL.into_iter().map(|f| {
                widgets::segment(
                    SharedString::from(format!("filter-{}", f.label())),
                    f.label(),
                    f == filter,
                )
                .on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.routes_filter = f;
                        cx.notify();
                    },
                ))
            }),
        );
        let head =
            |text: &'static str, w: f32| div().w(px(w)).flex_none().child(text);
        let header = div()
            .flex()
            .gap(px(12.))
            .px(px(16.))
            .py(px(10.))
            .text_size(px(12.))
            .text_color(color(theme::TEXT_LABEL))
            .border_b_1()
            .border_color(color(theme::BORDER))
            .child(div().flex_1().min_w(px(180.)).child("Route"))
            .child(head("Cabin", COLS[0]))
            .child(head("Best trip", COLS[1]).text_right())
            .child(
                head("vs usual ↓", COLS[2])
                    .text_color(color(theme::TEXT))
                    .font_weight(FontWeight::BOLD),
            )
            .child(head("History", COLS[3]))
            .child(head("Best dates", COLS[4]))
            .child(head("Checked", COLS[5]))
            .child(head("Status", COLS[6]));
        let now = self.now;
        let body = rows.into_iter().map(|r| {
            let key = r.key();
            let deal = r.status.is_deal();
            let status = match r.status {
                RowStatus::Watching => div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(theme::TEXT_FAINT))
                    .child("Watching"),
                other => widgets::status_chip(other),
            };
            div()
                .id(SharedString::from(format!(
                    "route-{}-{}",
                    key.route,
                    key.cabin.code()
                )))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(16.))
                .py(px(9.))
                .border_b_1()
                .border_color(color(theme::ROW_RULE_FAINT))
                .cursor_pointer()
                .relative()
                .when(deal, |d| {
                    d.bg(color(theme::DEAL_ROW)).child(widgets::accent())
                })
                .hover(|d| d.bg(color(theme::SELECTED)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(180.))
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(15.))
                                .child(r.code().to_owned()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_color(color(theme::TEXT_MUTED))
                                .child(r.city().to_owned()),
                        ),
                )
                .child(
                    div()
                        .w(px(COLS[0]))
                        .flex_none()
                        .text_color(color(theme::TEXT_MUTED))
                        .child(r.cabin.label()),
                )
                .child(
                    mono(
                        fonts,
                        r.best.as_ref().map_or_else(
                            || "–".to_owned(),
                            |t| format::money(&t.price),
                        ),
                    )
                    .w(px(COLS[1]))
                    .flex_none()
                    .text_right()
                    .font_weight(FontWeight::BOLD),
                )
                .child(
                    div()
                        .w(px(COLS[2]))
                        .flex_none()
                        .child(change_bar(fonts, r.change, deal)),
                )
                .child(div().w(px(COLS[3])).flex_none().child(
                    widgets::sparkline(
                        r.history.clone(),
                        120.,
                        28.,
                        if deal { theme::YELLOW } else { theme::SPARK },
                    ),
                ))
                .child(
                    div()
                        .w(px(COLS[4]))
                        .flex_none()
                        .text_color(color(theme::TEXT_SOFT))
                        .child(r.best.as_ref().map_or_else(
                            || "–".to_owned(),
                            |t| format::date_range(t.departure, t.return_date),
                        )),
                )
                .child(
                    div()
                        .w(px(COLS[5]))
                        .flex_none()
                        .text_color(color(theme::TEXT_FAINT))
                        .child(r.last_checked.map_or_else(
                            || "never".to_owned(),
                            |at| format::ago(now, at),
                        )),
                )
                .child(div().w(px(COLS[6])).flex_none().flex().child(status))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select(key, cx);
                    this.navigate(Screen::Detail, window, cx);
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(24.))
            .pt(px(18.))
            .pb(px(24.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(widgets::heading("Routes", 18.))
                    .child(filters)
                    .child(
                        widgets::ghost_button("add-route", "+ Add route")
                            .ml_auto()
                            .py(px(7.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Screen::Settings, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(color(theme::BORDER))
                    .rounded(px(12.))
                    .overflow_hidden()
                    .child(header)
                    .children(body),
            )
    }
}
