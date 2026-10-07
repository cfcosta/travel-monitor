//! The route detail ("I"): six months of departures for one trip
//! length, coloured against the usual price, the selected departure's
//! sellers, and the route's recent checks.

use chrono::Duration;
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
use tm_domain::{Currency, Money};

use super::{
    Screen,
    TravelMonitor,
    deals::sellers_table,
    map::{MapDest, MapModel, MapPlace, map},
    widgets::{self, mono},
};
use crate::{
    data::{ActivityRow, RouteDetail, RouteRow, Snapshot},
    format,
    geo::GeoWindow,
    grid::{CalendarDay, Tone},
    live::Fresh,
    theme::{self, Fonts, color},
};

const CELL: f32 = 22.;
const GAP: f32 = 4.;

fn tone_bg(tone: Tone) -> Option<u32> {
    match tone {
        Tone::Deal => Some(theme::YELLOW),
        Tone::Below => Some(theme::TONE_BELOW),
        Tone::Typical => Some(theme::TONE_TYPICAL),
        Tone::Above => Some(theme::TONE_ABOVE),
        Tone::Unknown => Some(theme::CARD),
        Tone::Past => None,
    }
}

impl TravelMonitor {
    pub(super) fn render_detail(
        &mut self,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = &self.snapshot else {
            return div().into_any_element();
        };
        let row = self.selected.and_then(|k| s.row(k));
        let detail = s.detail.as_ref().filter(|d| Some(d.key) == self.selected);
        let main = match row {
            Some(row) => self
                .render_detail_main(fonts, s, row, detail, cx)
                .into_any_element(),
            None => div()
                .p(px(28.))
                .text_color(color(theme::TEXT_FAINT))
                .child("Pick a route.")
                .into_any_element(),
        };
        div()
            .flex()
            .size_full()
            .min_h_0()
            .child(self.render_rail(fonts, s, cx))
            .child(
                div()
                    .id("detail-main")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(main),
            )
            .into_any_element()
    }

    fn render_rail(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut rows: Vec<&RouteRow> = s.rows.iter().collect();
        super::sort_rows(&mut rows, super::Sort::BiggestDrop);
        div()
            .id("rail")
            .w(px(220.))
            .flex_none()
            .overflow_y_scroll()
            .py(px(10.))
            .border_r_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::PANEL))
            .children(rows.into_iter().map(|r| {
                let key = r.key();
                let on = self.selected == Some(key);
                div()
                    .id(SharedString::from(format!(
                        "rail-{}-{}",
                        key.route,
                        key.cabin.code()
                    )))
                    .flex()
                    .justify_between()
                    .items_center()
                    .px(px(16.))
                    .py(px(9.))
                    .cursor_pointer()
                    .relative()
                    .when(on, |d| {
                        d.bg(color(theme::SELECTED)).child(widgets::accent())
                    })
                    .hover(|d| d.bg(color(theme::SELECTED)))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_baseline()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child(r.code().to_owned()),
                            )
                            .when(r.cabin != tm_domain::Cabin::Economy, |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(color(theme::TEXT_FAINT))
                                        .child(r.cabin.label()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_baseline()
                            .child(
                                mono(
                                    fonts,
                                    r.best.as_ref().map_or_else(
                                        || "–".to_owned(),
                                        |t| format::money(&t.price),
                                    ),
                                )
                                .text_size(px(12.))
                                .text_color(color(theme::TEXT_SOFT)),
                            )
                            .child(
                                mono(
                                    fonts,
                                    r.change.map_or_else(
                                        || "–".to_owned(),
                                        format::change,
                                    ),
                                )
                                .text_size(px(11.))
                                .text_color(
                                    color(if r.status.is_deal() {
                                        theme::YELLOW
                                    } else {
                                        theme::TEXT_FAINT
                                    }),
                                ),
                            ),
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select(key, cx)),
                    )
            }))
    }

    fn render_detail_main(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        row: &RouteRow,
        detail: Option<&RouteDetail>,
        cx: &mut Context<Self>,
    ) -> Div {
        let settings = &s.settings;
        let days = self.calendar_days.unwrap_or(settings.trips.round_trip_days);
        let adults = settings.passengers.adults;
        let pos: Vec<&str> = s
            .points_of_sale
            .iter()
            .filter(|p| p.active)
            .map(|p| p.country.as_str())
            .collect();
        let region = if row.route.is_domestic() {
            "Domestic"
        } else {
            "International"
        };
        let head = div()
            .flex_1()
            .min_w(px(380.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .justify_center()
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child(
                        div()
                            .id("crumb-routes")
                            .cursor_pointer()
                            .hover(|d| d.text_color(color(theme::TEAL)))
                            .child("Routes")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Screen::Routes, window, cx)
                            })),
                    )
                    .child(format!("/ {region}")),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(34.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!(
                                "{} → {}",
                                row.route.origin.code,
                                row.code()
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(34.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(theme::TEXT_LABEL))
                            .child(row.city().to_owned()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(widgets::pill(format!("{days}-day round trips")))
                    .child(widgets::pill(format!(
                        "{adults} adult{}",
                        if adults == 1 { "" } else { "s" }
                    )))
                    .child(widgets::pill(format!("POS {}", pos.join(" · "))))
                    .children(detail.and_then(|d| d.change_rate).map(|r| {
                        widgets::pill(format!("Changes ≈ {r:.1}/day"))
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .mt(px(4.))
                    .child(
                        widgets::ghost_button("detail-check", "Check now")
                            .py(px(8.))
                            .on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.check_now(cx)
                                }),
                            ),
                    )
                    .child(
                        widgets::ghost_button("detail-edit", "Edit route")
                            .py(px(8.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.navigate(Screen::Settings, window, cx)
                            })),
                    ),
            );
        let cheapest = detail.and_then(|d| d.calendar.cheapest());
        let money =
            |minor: i64| format::money(&Money::new(minor, Currency::brl()));
        let tiles = div()
            .relative()
            .w(px(360.))
            .flex_none()
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
                            "Cheapest now",
                            cheapest.and_then(|c| c.price).map_or_else(
                                || "–".to_owned(),
                                |p| money(p.minor),
                            ),
                            cheapest
                                .map(|c| {
                                    format::date_range(
                                        c.date,
                                        c.date
                                            + Duration::days(i64::from(days)),
                                    )
                                })
                                .unwrap_or_default(),
                            theme::YELLOW,
                        )
                        .flex_1(),
                    )
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Lowest ever",
                            detail
                                .and_then(|d| d.lowest_ever.as_ref())
                                .map_or_else(
                                    || "–".to_owned(),
                                    |(m, _, _)| format::money(m),
                                ),
                            detail
                                .and_then(|d| d.lowest_ever.as_ref())
                                .map(|(_, at, pos)| {
                                    format!(
                                        "{pos} · {}",
                                        format::short_date(*at)
                                    )
                                })
                                .unwrap_or_default(),
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
                            detail.and_then(|d| d.usual.as_ref()).map_or_else(
                                || "Usual".to_owned(),
                                |(_, b)| format!("Usual, {b}"),
                            ),
                            detail.and_then(|d| d.usual.as_ref()).map_or_else(
                                || "–".to_owned(),
                                |(m, _)| format::money(m),
                            ),
                            "60-day median",
                            theme::TEXT,
                        )
                        .flex_1(),
                    )
                    .child(
                        widgets::stat_tile(
                            fonts,
                            "Last checked",
                            row.last_checked.map_or_else(
                                || "never".to_owned(),
                                format::clock,
                            ),
                            detail
                                .and_then(|d| d.next_check)
                                .map(|n| {
                                    format!(
                                        "next in {}",
                                        format::until(self.now, n)
                                    )
                                })
                                .unwrap_or_default(),
                            theme::TEXT,
                        )
                        .flex_1(),
                    ),
            )
            // New observations for this route just arrived (0057).
            .children(widgets::fresh_tint(self.glow(Fresh::Route(row.key()))));
        let route_map = {
            let o = &row.route.origin;
            let d = &row.route.destination;
            let window = GeoWindow {
                west: f32::INFINITY,
                east: f32::NEG_INFINITY,
                north: f32::NEG_INFINITY,
                south: f32::INFINITY,
            }
            .including(
                [(o.lon as f32, o.lat as f32), (d.lon as f32, d.lat as f32)],
                12.0,
            );
            div()
                .w(px(240.))
                .h(px(190.))
                .flex_none()
                .rounded(px(14.))
                .overflow_hidden()
                .border_1()
                .border_color(color(theme::BORDER_CARD))
                .bg(color(theme::GROUND))
                .child(map(
                    MapModel {
                        origin: Some(MapPlace {
                            code: o.code.clone().into(),
                            lon: o.lon as f32,
                            lat: o.lat as f32,
                        }),
                        destinations: vec![MapDest {
                            place: MapPlace {
                                code: d.code.clone().into(),
                                lon: d.lon as f32,
                                lat: d.lat as f32,
                            },
                            deal: row.status.is_deal(),
                            selected: true,
                        }],
                        window,
                        bottom_inset: 0.,
                        labels: false,
                    },
                    fonts.clone(),
                    None,
                    |_, _, _| {},
                ))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(28.))
            .pt(px(20.))
            .pb(px(28.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.))
                    .child(head)
                    .child(tiles)
                    .child(route_map),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.))
                    .items_start()
                    .child(self.render_heatmap(s, row, detail, days, cx))
                    .child(
                        self.render_departure(fonts, s, row, detail, days, cx),
                    ),
            )
            .child(render_checks(
                fonts,
                detail.map(|d| d.recent_checks.as_slice()).unwrap_or(&[]),
                |id| self.glow(Fresh::Query(id)),
            ))
    }

    fn render_heatmap(
        &self,
        s: &Snapshot,
        row: &RouteRow,
        detail: Option<&RouteDetail>,
        days: u16,
        cx: &mut Context<Self>,
    ) -> Div {
        // The cabins this route is tracked in, as rows of the snapshot.
        let cabins: Vec<_> = s
            .rows
            .iter()
            .filter(|r| r.route.id == row.route.id)
            .map(|r| r.key())
            .collect();
        let cabin_tabs =
            widgets::segments().children(cabins.into_iter().map(|key| {
                widgets::segment(
                    SharedString::from(format!("cabin-{}", key.cabin.code())),
                    key.cabin.label(),
                    key == row.key(),
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select(key, cx)),
                )
            }));
        let t = &s.settings.trips;
        let length_tabs =
            widgets::segments().children((t.min_days..=t.max_days).map(|n| {
                widgets::segment(
                    SharedString::from(format!("len-{n}")),
                    format!("{n}d"),
                    n == days,
                )
                .px(px(8.))
                .on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.calendar_days = Some(n);
                        this.refresh(cx);
                        cx.notify();
                    },
                ))
            }));
        let body: AnyElement = match detail {
            None => div()
                .text_color(color(theme::TEXT_FAINT))
                .child("Loading…")
                .into_any_element(),
            Some(d) => {
                let months = div()
                    .flex()
                    .gap(px(GAP))
                    .pl(px(40.))
                    .text_size(px(11.))
                    .text_color(color(theme::TEXT_FAINT))
                    .children(d.calendar.weeks.iter().map(|w| {
                        div()
                            .w(px(CELL))
                            .flex_none()
                            .whitespace_nowrap()
                            .child(w.month.unwrap_or(""))
                    }));
                let weekdays = div()
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .w(px(36.))
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(color(theme::TEXT_FAINT))
                    .children(
                        ["Mon", "", "Wed", "", "Fri", "", "Sun"]
                            .map(|l| div().h(px(CELL)).child(l)),
                    );
                let mut weeks = Vec::with_capacity(d.calendar.weeks.len());
                for (wi, w) in d.calendar.weeks.iter().enumerate() {
                    let mut column = div().flex().flex_col().gap(px(GAP));
                    for (di, day) in w.days.iter().enumerate() {
                        column = column.child(self.day_cell(wi, di, day, cx));
                    }
                    weeks.push(column);
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .debug_selector(|| "calendar".into())
                    .child(months)
                    .child(
                        div()
                            .flex()
                            .gap(px(GAP))
                            .child(weekdays)
                            .children(weeks),
                    )
                    .into_any_element()
            }
        };
        let legend_item = |bg: Option<u32>, text: &'static str| {
            let swatch = div().size(px(12.)).rounded(px(3.));
            let swatch = match bg {
                Some(bg) => swatch.bg(color(bg)),
                None => swatch
                    .border_1()
                    .border_dashed()
                    .border_color(color(theme::BORDER_BUTTON)),
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(swatch)
                .child(text)
        };
        div()
            .flex_1()
            .min_w(px(700.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(18.))
            .py(px(16.))
            .rounded(px(14.))
            .border_1()
            .border_color(color(theme::BORDER))
            .bg(color(theme::SECTION))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_between()
                    .items_center()
                    .gap(px(10.))
                    .child(widgets::heading(format!("{days}-day trips by departure date"), 15.))
                    .child(div().flex().gap(px(8.)).child(length_tabs).child(cabin_tabs)),
            )
            .child(div().id("calendar-scroll").overflow_x_scroll().child(body))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(16.))
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_LABEL))
                    .child("Compared with the usual price for the same days to departure:")
                    .child(legend_item(Some(theme::YELLOW), "deal"))
                    .child(legend_item(Some(theme::TONE_BELOW), "below"))
                    .child(legend_item(Some(theme::TONE_TYPICAL), "typical"))
                    .child(legend_item(Some(theme::TONE_ABOVE), "above"))
                    .child(legend_item(None, "past")),
            )
    }

    fn day_cell(
        &self,
        wi: usize,
        di: usize,
        day: &CalendarDay,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let date = day.date;
        let cell = div()
            .id(SharedString::from(format!("day-{wi}-{di}")))
            .size(px(CELL))
            .rounded(px(5.));
        let cell = match tone_bg(day.tone) {
            Some(bg) => cell.bg(color(bg)).cursor_pointer().on_click(
                cx.listener(move |this, _, _, cx| {
                    this.departure = Some(date);
                    this.refresh(cx);
                    cx.notify();
                }),
            ),
            None => cell
                .border_1()
                .border_dashed()
                .border_color(color(theme::BORDER_BUTTON)),
        };
        if day.selected {
            cell.border_2().border_color(color(0xFFFFFF))
        } else {
            cell.hover(|d| d.border_1().border_color(color(theme::TEXT_MUTED)))
        }
    }

    fn render_departure(
        &self,
        fonts: &Fonts,
        s: &Snapshot,
        row: &RouteRow,
        detail: Option<&RouteDetail>,
        days: u16,
        cx: &mut Context<Self>,
    ) -> Div {
        let panel = div()
            .flex_1()
            .min_w(px(300.))
            .max_w(px(380.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(18.))
            .py(px(16.))
            .rounded(px(14.))
            .bg(color(0x0D1220));
        let Some(d) = detail else {
            return panel.border_1().border_color(color(theme::BORDER));
        };
        let trip = d.trip.as_ref();
        let departure = self.departure.or(trip.map(|t| t.departure));
        let Some(departure) = departure else {
            return panel.border_1().border_color(color(theme::BORDER)).child(
                div()
                    .text_color(color(theme::TEXT_FAINT))
                    .child("No prices yet."),
            );
        };
        let ret = departure + Duration::days(i64::from(days));
        let day = d.calendar.day(departure);
        let is_trip =
            trip.is_some_and(|t| t.departure == departure && t.days() == days);
        let deal = day.is_some_and(|d| d.tone == Tone::Deal);
        let change = day.and_then(|d| {
            let p = d.price?;
            let b = d.baseline?;
            (b > 0).then(|| p.minor as f64 / b as f64 - 1.0)
        });
        let chip = change.map(|c| {
            let text = if deal {
                format!("Deal · {}", format::change(c))
            } else {
                format::change(c)
            };
            let chip = div()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(12.))
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .child(text);
            if deal {
                chip.bg(color(theme::YELLOW))
                    .text_color(color(theme::ON_YELLOW))
            } else {
                chip.border_1()
                    .border_color(color(theme::BORDER_BUTTON))
                    .text_color(color(theme::TEXT_MUTED))
            }
        });
        let price_line = day.and_then(|d| d.price).map(|p| {
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .child(
                    mono(
                        fonts,
                        format::money(&Money::new(p.minor, Currency::brl())),
                    )
                    .text_size(px(24.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(if deal {
                        theme::PRICE_DEAL
                    } else {
                        theme::TEXT
                    })),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_FAINT))
                        .child(if p.confirmed {
                            "round trip, confirmed"
                        } else {
                            "estimated from the two legs"
                        }),
                )
        });
        let carrier = trip.filter(|_| is_trip).and_then(|t| {
            let label = t.carrier_label()?;
            Some(match t.duration_minutes {
                Some(m) => format!(
                    "{label} {} → {} · {}",
                    row.route.origin.code,
                    row.code(),
                    format::duration(m)
                ),
                None => label,
            })
        });
        let settings = &s.settings;
        panel
            .border_1()
            .border_color(color(if deal { theme::YELLOW } else { theme::BORDER }))
            .debug_selector(|| "departure".into())
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap(px(8.))
                    .child(widgets::heading(
                        format!("{} → {}", format::day_date(departure), format::day_date(ret)),
                        15.,
                    ))
                    .children(chip),
            )
            .children(carrier.map(|c| div().text_color(color(theme::TEXT_MUTED)).child(c)))
            .children(price_line)
            .when(is_trip, |p| p.child(sellers_table(fonts, d, true)))
            .when(!is_trip, |p| {
                p.child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(theme::TEXT_FAINT))
                        .child("Sellers are read for confirmed deals; Check now asks for a search of these dates."),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(theme::TEXT_FAINT))
                    .child(format!(
                        "Effective price includes IOF {} and a {} card spread.",
                        format::percent(settings.iof),
                        format::percent(settings.card_spread)
                    )),
            )
            .child(
                widgets::primary_button("detail-open-gf", "Open in Google Flights")
                    .flex()
                    .justify_center()
                    .on_click(cx.listener(|this, _, _, cx| this.open_in_google_flights(cx))),
            )
    }
}

fn render_checks(
    fonts: &Fonts,
    checks: &[ActivityRow],
    glow: impl Fn(i64) -> f32,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(widgets::heading("Recent checks", 15.))
        .child(super::activity::activity_table(
            fonts, checks, false, None, glow,
        ))
}
